//! Closed-loop planning.
//!
//! Searching from a point to itself degenerates: the shortest closed path is the
//! empty one. The design's reference-point split avoids that entirely — plan
//! `p0 -> pm -> p0` with `pm` far from `p0` — and the result is a closed circuit
//! whose two halves are planned by the ordinary route planner.
//!
//! Lap continuity is a property of the *noise*, not of the geometry: the motion
//! stage keeps the same Ornstein–Uhlenbeck streams across laps, so the drift of
//! lap `r+1` continues where lap `r` left off instead of restarting.

use glam::DVec2;

use crate::environment::Environment;
use crate::error::{CoreError, Result};
use crate::graph::MixedGraph;
use crate::motion::profile::StopHold;
use crate::person::PersonParams;

use super::request::LoopRequest;
use super::standard::{PlannedRoute, RouteConfig};

/// Tunables of loop planning.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LoopConfig {
    /// Route configuration used for both halves.
    pub route: RouteConfig,
    /// Minimum distance between the start and the reference point, metres.
    pub min_reference_distance_m: f64,
    /// Whether the loop is smoothed as a closed curve.
    pub smooth_as_closed: bool,
}

impl Default for LoopConfig {
    fn default() -> Self {
        Self {
            route: RouteConfig::default(),
            min_reference_distance_m: 40.0,
            smooth_as_closed: true,
        }
    }
}

/// Plans a closed loop.
///
/// The reference point is taken from the request when given; otherwise the map's
/// farthest passable point in the direction of greatest extent is used, which for
/// a track or a stadium produces a sensible circuit without configuration.
pub fn plan_loop(
    environment: &Environment,
    graph: &mut MixedGraph<'_>,
    request: &LoopRequest,
    person: &PersonParams,
    config: &LoopConfig,
    seed: u64,
    individual: u32,
) -> Result<PlannedRoute> {
    let start = environment.snap_to_passable(request.start, 25.0)?;
    let reference = match request.reference {
        Some(reference) => environment.snap_to_passable(reference, 25.0)?,
        None => choose_reference(environment, &start, config.min_reference_distance_m)?,
    };

    let outbound = super::request::StandardRequest::new(start, reference);
    let inbound = super::request::StandardRequest::new(reference, start);

    // A lap has no standstill at the junction: a runner crossing the line keeps
    // going, and a zero-speed boundary there would look like a stop. The library
    // is never consulted here (implementation doc 6.6): the halves come from the
    // reference-point split, and a stored OD set does not describe their
    // candidate semantics. Each half is a two-point request, so it is planned
    // with the single-leg heuristic inflation of the route configuration.
    let mut first = super::standard::plan_with_library(
        environment,
        graph,
        &outbound,
        person,
        &config.route,
        seed,
        individual,
        false,
    )?;
    let second = super::standard::plan_with_library(
        environment,
        graph,
        &inbound,
        person,
        &config.route,
        seed,
        individual ^ 0x51ED,
        false,
    )?;

    // Join the halves, dropping the duplicated reference point.
    let mut points: Vec<DVec2> = first.path.points().to_vec();
    points.extend(second.path.points().iter().skip(1).copied());
    // Close the circuit exactly.
    if let Some(last) = points.last_mut() {
        *last = start;
    }

    let joined = crate::path::Path::new(points)?;
    // Smoothing the closed band: run it on the open polyline and then re-close,
    // which avoids the endpoints being pinned to a shared point that is not a real
    // corner. It uses the caller's smoothing configuration, like both halves. The
    // result is a candidate, not the answer — the resampling below can still
    // introduce a segment that clips a forbidden cell, so the finished path is
    // validated against the exact cell traversal like the standard planner's.
    let smoothed: Option<Vec<DVec2>> = if config.smooth_as_closed {
        // The junction of the two halves is the reference point the circuit was
        // split at, so it is anchored like a waypoint: the band would otherwise
        // straighten the far end of the circuit away.
        let outcome = crate::smooth::smooth_path_anchored(
            joined.points(),
            &environment.cost,
            &environment.distance,
            &environment.hard,
            &config.route.smoothing,
            &[first.path.points().len() - 1],
        )?;
        outcome.feasible.then(|| outcome.path.points().to_vec())
    } else {
        None
    };
    let base: &[DVec2] = smoothed.as_deref().unwrap_or(joined.points());
    let mut closed: Vec<DVec2> = base.to_vec();
    if let Some(last) = closed.last_mut() {
        *last = start;
    }
    let resampled = crate::path::Path::resampled(closed.clone(), config.route.sample_spacing_m)?;
    let smoothed = crate::plan::clearance::first_clear_path(
        &[resampled.points(), closed.as_slice(), joined.points()],
        &environment.hard,
        "loop route",
    )?;

    // As in the standard planner, the cost describes the circuit that will be
    // run: joining, smoothing and resampling all change the geometry, so the sum
    // of the two halves' search costs no longer prices it.
    first.cost_equiv_m = crate::graph::polyline_cost(
        &crate::field::sampler::CostSampler::new(&environment.cost),
        &environment.connectors,
        person.target_speed,
        smoothed.points(),
    )
    .unwrap_or(first.cost_equiv_m + second.cost_equiv_m);
    first.path = smoothed;
    first.length_m = first.path.total_length();
    first.stops = Vec::<StopHold>::new();
    first.modifiers.clear();
    Ok(first)
}

/// Picks a reference point far from the start but still on the map.
fn choose_reference(
    environment: &Environment,
    start: &DVec2,
    min_distance_m: f64,
) -> Result<DVec2> {
    let bounds = environment.bounds;
    let center = DVec2::new(
        (bounds.min_x + bounds.max_x) * 0.5,
        (bounds.min_y + bounds.max_y) * 0.5,
    );
    // Opposite side of the map from the start: a stable choice that needs no
    // search and gives the two halves comparable length.
    let direction = (*start - center).normalize_or_zero();
    let extent = (bounds.width().min(bounds.height())) * 0.4;
    let candidate = center - direction * extent;
    let candidate = if (candidate - *start).length() >= min_distance_m {
        candidate
    } else {
        center
    };
    let snapped = environment
        .hard
        .nearest_passable(candidate, (extent / environment.grid.resolution) as usize)
        .ok_or_else(|| {
            CoreError::unusable(
                candidate.x,
                candidate.y,
                "no passable reference point found",
            )
        })?;
    if (snapped - *start).length() < min_distance_m {
        return Err(CoreError::config(
            "loop reference point is too close to the start; provide one explicitly",
        ));
    }
    Ok(snapped)
}

/// Number of laps that fit in a duration at the individual's pace.
pub fn laps_for_duration(loop_length_m: f64, target_speed: f64, duration_s: f64) -> usize {
    if loop_length_m <= 1e-6 || target_speed <= 1e-6 {
        return 1;
    }
    ((duration_s * target_speed / loop_length_m).floor() as usize).max(1)
}
