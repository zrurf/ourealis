//! Online re-planning: a new checkpoint arrives mid-run.
//!
//! Two rules make the redirect look like a runner changing their mind rather
//! than a trajectory being cut and spliced:
//!
//! 1. **re-plan from the full state** — position, speed *and* heading. Starting
//!    from the nearest point on the old path would silently discard velocity and
//!    produce a discontinuity in both speed and direction;
//! 2. **blend over a window** — the old and the new trajectory are mixed with a
//!    smoothstep weight, whose derivative vanishes at both ends, so position and
//!    velocity are continuous across the switch. After the window the runner
//!    follows the new route exactly.

use crate::environment::Environment;
use crate::error::{CoreError, Result};
use crate::graph::MixedGraph;
use crate::math::sampling::smoothstep;
use crate::motion::Trajectory;
use crate::person::PersonParams;

use super::request::{Checkpoint, StandardRequest};
use super::standard::{PlannedRoute, RouteConfig, plan};

/// Tunables of online re-planning.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DynamicConfig {
    /// Route configuration of each re-plan.
    pub route: RouteConfig,
    /// Length of the blend window, seconds.
    pub blend_window_s: f64,
}

impl Default for DynamicConfig {
    fn default() -> Self {
        Self {
            route: RouteConfig::default(),
            blend_window_s: 6.0,
        }
    }
}

/// A re-planning event and its outcome.
#[derive(Debug, Clone, PartialEq)]
pub struct ReplanRecord {
    /// Checkpoint that triggered it.
    pub checkpoint: Checkpoint,
    /// True when a new route was produced.
    pub accepted: bool,
    /// Reason a checkpoint was rejected.
    pub note: String,
}

/// Blends a re-planned trajectory into the running one.
///
/// The replacement is built from the runner's current state, so its own timeline
/// starts at zero; it is therefore aligned so that its start coincides with
/// `switch_s` of the existing trajectory. From there the two are mixed with a
/// smoothstep weight, whose derivative vanishes at both ends, which is what keeps
/// position *and* velocity continuous across the switch. Beyond the old
/// trajectory's end the new one is followed as is.
pub fn blend_trajectories(
    old: &Trajectory,
    new: &Trajectory,
    switch_s: f64,
    window_s: f64,
) -> Result<Trajectory> {
    if new.samples.is_empty() {
        return Ok(old.clone());
    }
    let window_s = window_s.max(0.0);
    let end = switch_s + window_s;
    // The replacement was built from the runner's state, so its own clock starts
    // at its first sample; shifting by that keeps its internal spacing whatever
    // the sampling turns out to be.
    let new_start = new.samples[0].time_s;

    let mut blended = new.clone();
    // Keep the part of the old run that precedes the redirect. A zero-length
    // window is an instantaneous switch, not a discard of the recording so far.
    let mut samples: Vec<crate::motion::TrajectorySample> = old
        .samples
        .iter()
        .filter(|sample| sample.time_s < switch_s)
        .copied()
        .collect();

    for sample in blended.samples.iter_mut() {
        let time_s = switch_s + (sample.time_s - new_start).max(0.0);
        sample.time_s = time_s;
        let alpha = if window_s <= 0.0 || time_s >= end {
            1.0
        } else {
            smoothstep((time_s - switch_s) / window_s)
        };
        if alpha < 1.0
            && let Some(old_sample) = old.sample_at(time_s)
        {
            let position = old_sample.position * (1.0 - alpha) + sample.position * alpha;
            let center = old_sample.center * (1.0 - alpha) + sample.center * alpha;
            let speed = old_sample.speed * (1.0 - alpha) + sample.speed * alpha;
            let z = old_sample.z * (1.0 - alpha) + sample.z * alpha;
            let terrain_z = old_sample.terrain_z * (1.0 - alpha) + sample.terrain_z * alpha;
            sample.position = position;
            sample.center = center;
            sample.z = z;
            sample.terrain_z = terrain_z;
            sample.bounce_z = z - terrain_z;
            sample.speed = speed;
            sample.heading = blend_angle(old_sample.heading, sample.heading, alpha);
            sample.head_heading = blend_angle(old_sample.head_heading, sample.head_heading, alpha);
            sample.pitch = old_sample.pitch * (1.0 - alpha) + sample.pitch * alpha;
            sample.roll = old_sample.roll * (1.0 - alpha) + sample.roll * alpha;
            // Below half weight the old run still describes what the body is
            // doing; above it the replacement's own flags are the ones that say
            // whether the runner is standing or turning.
            if alpha < 0.5 {
                sample.turning = old_sample.turning;
                sample.standing = old_sample.standing;
            }
        }
        samples.push(*sample);
    }
    blended.samples = samples;
    Ok(blended)
}

/// Interpolates two angles the short way round.
fn blend_angle(a: f64, b: f64, alpha: f64) -> f64 {
    a + alpha * crate::math::angle_difference(b, a)
}

/// Re-plans a run for a new checkpoint.
///
/// `current` is the trajectory generated so far, `switch_s` the time the
/// checkpoint takes effect, and `goal` where the runner must now go.
#[allow(clippy::too_many_arguments)]
pub fn replan(
    environment: &Environment,
    graph: &mut MixedGraph<'_>,
    current: &Trajectory,
    checkpoint: &Checkpoint,
    person: &PersonParams,
    config: &DynamicConfig,
    seed: u64,
    individual: u32,
) -> Result<(PlannedRoute, ReplanRecord)> {
    let switch_s = checkpoint.issued_at_s.clamp(0.0, current.duration_s());
    let Some(state) = current.sample_at(switch_s) else {
        return Err(CoreError::config(
            "checkpoint time lies outside the generated trajectory",
        ));
    };
    if (checkpoint.position - state.position).length() < 1.0 {
        return Ok((
            PlannedRoute {
                path: current.path.clone(),
                legs: Vec::new(),
                modifiers: Vec::new(),
                stops: Vec::new(),
                cost_equiv_m: 0.0,
                length_m: current.length_m(),
            },
            ReplanRecord {
                checkpoint: *checkpoint,
                accepted: false,
                note: "checkpoint coincides with the current position".into(),
            },
        ));
    }

    // Re-plan from the current position towards the checkpoint. The path starts
    // where the runner is, so the position is continuous; the speed the runner
    // carries is handed to the motion stage by the caller
    // (`MotionConfig::profile.initial_speed`), and the heading is carried by the
    // blend window, which mixes the two trajectories' headings over it.
    let request = StandardRequest::new(state.position, checkpoint.position);
    let route = plan(
        environment,
        graph,
        &request,
        person,
        &config.route,
        seed,
        individual,
    )?;

    Ok((
        route,
        ReplanRecord {
            checkpoint: *checkpoint,
            accepted: true,
            note: format!("re-planned at t = {switch_s:.1} s"),
        },
    ))
}
