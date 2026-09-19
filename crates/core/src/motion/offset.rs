//! Lateral offset: injection, effective curvature and feasibility.
//!
//! A runner does not follow the geometric centre line. They hold a habitual
//! offset, drift slowly around it, and cut the inside of a bend — but only as far
//! as physics and the environment allow. Two corrections are mandatory and are
//! applied here rather than assumed away:
//!
//! * **effective curvature** — shifting by `d` along the normal changes the
//!   turn radius, `kappa_eff = kappa / (1 - d kappa)`, which feeds the lateral
//!   acceleration check, the roll attitude and the gyroscope. Using the centre
//!   line curvature downstream would make the IMU disagree with the trajectory;
//! * **environmental feasibility** — the shifted point must stay on passable
//!   ground and outside the safety radius of obstacles, and any correction must
//!   ramp over a few metres instead of stepping, or the lateral velocity would
//!   spike.

use glam::DVec2;

use crate::error::{CoreError, Result};
use crate::field::HardMask;
use crate::math::sampling::low_pass;
use crate::path::Path;
use crate::rng::{Rng, Stream};
use crate::terrain::{DistanceField, Terrain};

use super::limits::{allowed_inner_offset, effective_curvature};
use super::profile::SpeedProfile;

/// Minimum magnitude of the parallel-curve denominator.
pub const CURVATURE_DENOMINATOR_FLOOR: f64 = 0.3;

/// Lateral offset configuration.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OffsetConfig {
    /// Mean offset, metres; positive is to the left of the direction of travel.
    pub mean_m: f64,
    /// Steady-state standard deviation of the drift, metres.
    pub std_m: f64,
    /// Drift time constant, seconds.
    pub tau_s: f64,
    /// Safety radius kept from obstacles, metres.
    pub safe_radius_m: f64,
    /// Arc length over which corrections ramp, metres.
    pub transition_m: f64,
    /// Smoothing time applied to the applied offset, seconds.
    ///
    /// An Ornstein-Uhlenbeck process is continuous but not differentiable: its
    /// velocity is white, so the raw draws give the trajectory a lateral
    /// acceleration that no runner produces and that the gyroscope would report
    /// as heading jitter. Low-passing the applied value keeps the drift
    /// statistics while band-limiting the motion.
    pub smoothing_s: f64,
    /// Lateral acceleration budget, m/s^2.
    pub a_lat_max: f64,
}

impl Default for OffsetConfig {
    fn default() -> Self {
        Self {
            mean_m: 0.6,
            std_m: 0.35,
            tau_s: 45.0,
            safe_radius_m: 0.75,
            transition_m: 3.0,
            smoothing_s: 0.5,
            a_lat_max: 2.5,
        }
    }
}

/// Offset and curvature at one trajectory sample.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OffsetSample {
    /// Arc length, metres.
    pub s: f64,
    /// Time, seconds.
    pub t: f64,
    /// Applied offset after all corrections, metres.
    pub offset_m: f64,
    /// Requested offset before corrections, metres.
    pub requested_m: f64,
    /// Centre-line curvature.
    pub kappa: f64,
    /// Effective curvature of the offset trajectory.
    pub kappa_eff: f64,
    /// True when the curvature or lateral-acceleration bound changed the offset.
    pub clamped_by_curvature: bool,
    /// True when the environment (obstacle or hard constraint) changed it.
    pub clamped_by_environment: bool,
}

/// Generates the lateral offset along a profiled path.
pub fn generate(
    path: &Path,
    profile: &SpeedProfile,
    _terrain: &Terrain,
    hard: &HardMask,
    distance_field: &DistanceField,
    config: &OffsetConfig,
    rng: &mut Rng,
) -> Result<Vec<OffsetSample>> {
    if profile.s.len() < 2 {
        return Err(CoreError::config("speed profile has too few samples"));
    }
    let theta = 1.0 / config.tau_s.max(1e-3);
    let mut out = Vec::with_capacity(profile.s.len());
    let mut offset = config.mean_m;
    // Ramped limit of the legal offset, in metres.
    let mut offset_limit = config.mean_m.abs();
    let mut applied_smoothed = config.mean_m;
    // The value actually applied on the previous sample. The climb back towards
    // the requested offset is ramped, so a limit that opens does not become a
    // step in the lateral position.
    let mut applied_previous = config.mean_m;

    for index in 0..profile.s.len() {
        let s = profile.s[index];
        let t = profile.t[index];
        let dt = if index == 0 {
            0.0
        } else {
            (profile.t[index] - profile.t[index - 1]).max(0.0)
        };

        // Ornstein-Uhlenbeck step parameterised by the steady-state standard
        // deviation, so the continuous diffusion coefficient never appears at a
        // call site. The exact discrete update is used, matching
        // `noise::OuProcess`, so the drift statistics do not depend on the step
        // size chosen here.
        if dt > 0.0 {
            let decay = (-theta * dt).exp();
            let noise = config.std_m * (1.0 - decay * decay).max(0.0).sqrt() * rng.gaussian();
            offset = config.mean_m + (offset - config.mean_m) * decay + noise;
        }
        let requested = offset;

        let kappa = path.curvature_at(s);
        let speed = profile.speed_at(s);
        let normal = path.normal_at(s);
        let center = path.position_at(s);

        // Environment: find the largest offset that keeps the runner on passable
        // ground and outside the safety radius, then ramp towards it. The ramp is
        // clamped from above, so a shrinking limit takes effect immediately while
        // the recovery is smooth: safety first, comfort second.
        let target_limit = environment_limit(
            center,
            normal,
            offset,
            hard,
            distance_field,
            config.safe_radius_m,
        );
        if dt > 0.0 {
            let tau = config.transition_m / speed.max(0.5);
            // The limit may open up immediately — there is nothing to be safe
            // about — but closes through the ramp, never above the legal value.
            // Closing is allowed to be abrupt: when the environment forbids the
            // current offset, an immediate correction is the only safe answer,
            // and the alternative would be to leave the runner inside an
            // obstacle.
            offset_limit = if target_limit >= offset_limit {
                target_limit
            } else {
                low_pass(offset_limit, target_limit, dt, tau).min(target_limit)
            };
        } else {
            offset_limit = target_limit;
        }
        // Band-limit the applied offset, so the trajectory carries the drift the
        // Ornstein-Uhlenbeck process models without its white velocity.
        applied_smoothed = if dt > 0.0 {
            low_pass(applied_smoothed, offset, dt, config.smoothing_s)
        } else {
            offset
        };
        let mut applied = applied_smoothed.clamp(-offset_limit, offset_limit);
        // The flag reports the *limit* binding, not the ramp applied below:
        // comparing the ramped value against the smoothed request would call every
        // sample "clamped" while the offset is recovering.
        let clamped_by_environment = (applied - applied_smoothed).abs() > 1e-6;

        // Curvature and lateral acceleration: a hard bound, never low-passed, so
        // the limit is respected at every sample.
        let mut clamped_by_curvature = false;
        if applied * kappa > 0.0 {
            // Inside of the bend: the offset reduces the radius.
            let margin = 1.0 - speed * speed * kappa.abs() / config.a_lat_max.max(1e-6);
            if margin <= 0.0 {
                applied = 0.0;
                clamped_by_curvature = true;
            } else if let Some(allowed) = allowed_inner_offset(kappa, speed, config.a_lat_max) {
                let limit = allowed * applied.signum();
                if applied.abs() > limit.abs() {
                    applied = limit;
                    clamped_by_curvature = true;
                }
            }
        }
        // Protect the parallel-curve denominator from below: an offset larger
        // than the radius of curvature would place the runner past the centre of
        // the arc and reverse the turn.
        let denominator = 1.0 - applied * kappa;
        if denominator < CURVATURE_DENOMINATOR_FLOOR {
            let max_offset = (1.0 - CURVATURE_DENOMINATOR_FLOOR) / kappa.abs().max(1e-9);
            applied = applied.signum() * max_offset.min(applied.abs());
            clamped_by_curvature = true;
        }

        // The recovery ramp, after every bound has had its say. Both the
        // environment limit and the curvature bound are evaluated per sample, and
        // both release as abruptly as they bind — a limit that opens, or a bend
        // whose curvature estimate eases — so following the released value at once
        // is a lateral step of the whole recovered amount, which the gyroscope and
        // the jerk metric both see. Shrinking stays immediate: an immediate
        // correction is the only safe answer where a bound tightens, and the ramp
        // is only ever applied on the way back up. Both bounds define an interval
        // of legal offsets along the normal, so the ramp runs between two legal
        // values and stays inside it.
        if dt > 0.0 && applied.abs() > applied_previous.abs() {
            let tau = config.transition_m / speed.max(0.5);
            applied = low_pass(applied_previous, applied, dt, tau);
            applied = applied.clamp(-offset_limit, offset_limit);
        }
        applied_previous = applied;

        out.push(OffsetSample {
            s,
            t,
            offset_m: applied,
            requested_m: requested,
            kappa,
            kappa_eff: effective_curvature(kappa, applied),
            clamped_by_curvature,
            clamped_by_environment,
        });
    }

    Ok(out)
}

/// Largest magnitude of the offset that keeps the shifted point legal.
///
/// Binary search rather than a fixed shrink step, so the correction is as small
/// as the environment allows and the trajectory drifts towards the centre line no
/// more than necessary. A point outside the map is never legal.
fn environment_limit(
    center: DVec2,
    normal: DVec2,
    offset: f64,
    hard: &HardMask,
    distance_field: &DistanceField,
    safe_radius_m: f64,
) -> f64 {
    let is_legal = |limit: f64| -> bool {
        let point = center + normal * limit;
        !hard.is_forbidden(point) && distance_field.distance_at(point) >= safe_radius_m
    };
    let magnitude = offset.abs();
    if magnitude <= 1e-9 {
        return 0.0;
    }
    if is_legal(offset) {
        return magnitude;
    }
    if !is_legal(0.0) {
        // Even the centre line is unusable; the smoothing stage's projection owns
        // that problem, so no offset is permitted here.
        return 0.0;
    }
    let sign = offset.signum();
    let (mut low, mut high) = (0.0f64, magnitude);
    for _ in 0..12 {
        let mid = 0.5 * (low + high);
        if is_legal(mid * sign) {
            low = mid;
        } else {
            high = mid;
        }
    }
    low
}

/// Builds the offset stream for one individual.
pub fn stream_rng(seed: u64, individual: u32) -> Rng {
    Rng::stream(seed, Stream::LateralOffset, individual, 0)
}
