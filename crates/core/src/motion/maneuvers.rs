//! Low-speed maneuvers: standing start, final stop and on-the-spot turns.
//!
//! These are the three behaviours a continuous speed profile cannot express, and
//! they are exactly the places where synthetic trajectories look synthetic: no
//! standing start means no GNSS cloud at the beginning, no turn maneuver means a
//! gyroscope that reads zero while the heading reverses.
//!
//! The turn follows a trapezoidal angular-velocity profile solved from the
//! required total rotation, so the runner ends exactly facing the new direction
//! in a physically plausible 1–2 seconds.

use glam::DVec2;

use crate::error::{CoreError, Result};
use crate::math::angle_of;

/// Angular acceleration used to solve a turn profile, rad/s^2.
pub const TURN_ALPHA: f64 = 8.0;

/// Minimum turn angle that counts as a reversal worth a maneuver, degrees.
///
/// A runner leans through a 120 degree bend without stopping; it is the near
/// reversal that has to be walked round on the spot.
pub const TURN_DETECTION_ANGLE_DEG: f64 = 150.0;

/// Angular velocity profile of an on-the-spot turn.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TurnProfile {
    /// Duration of the acceleration and deceleration phases, seconds.
    pub ramp_s: f64,
    /// Duration of the constant-rate phase, seconds.
    pub cruise_s: f64,
    /// Peak angular velocity, rad/s.
    pub peak_omega: f64,
    /// Total rotation, radians, signed.
    pub total_angle: f64,
}

impl TurnProfile {
    /// Solves a trapezoidal profile for a required rotation.
    ///
    /// The peak rate is reduced when the requested rotation is too small to
    /// reach it, so short turns stay short instead of taking a fixed time.
    pub fn solve(total_angle: f64, omega_max: f64, alpha: f64) -> Result<Self> {
        let magnitude = total_angle.abs();
        if magnitude < 1e-6 {
            return Err(CoreError::config("turn angle is too small to schedule"));
        }
        let alpha = alpha.max(0.1);
        let omega_max = omega_max.max(0.1);
        // Rotation needed to reach and leave the peak rate.
        let ramp_rotation = omega_max * omega_max / alpha;
        if magnitude <= ramp_rotation {
            // Triangular profile: never reaches the peak rate.
            let peak = (magnitude * alpha).sqrt();
            let ramp = peak / alpha;
            return Ok(Self {
                ramp_s: ramp,
                cruise_s: 0.0,
                peak_omega: peak,
                total_angle,
            });
        }
        let ramp = omega_max / alpha;
        let cruise_rotation = magnitude - ramp_rotation;
        Ok(Self {
            ramp_s: ramp,
            cruise_s: cruise_rotation / omega_max,
            peak_omega: omega_max,
            total_angle,
        })
    }

    /// Total duration, seconds.
    pub fn duration_s(&self) -> f64 {
        2.0 * self.ramp_s + self.cruise_s
    }

    /// Angular velocity at a time inside the maneuver, rad/s.
    pub fn omega_at(&self, t: f64) -> f64 {
        if t <= 0.0 || t >= self.duration_s() {
            return 0.0;
        }
        let sign = if self.total_angle < 0.0 { -1.0 } else { 1.0 };
        if t < self.ramp_s {
            // The ramp rate `angle_at` integrates. It is fixed by the peak and the
            // ramp length, so reading a constant here would report a rate that
            // does not add up to the rotation the same profile produced.
            sign * self.peak_omega / self.ramp_s.max(1e-6) * t
        } else if t < self.ramp_s + self.cruise_s {
            sign * self.peak_omega
        } else {
            let remaining = self.duration_s() - t;
            sign * self
                .peak_omega
                .min(remaining * self.peak_omega / self.ramp_s.max(1e-6))
        }
    }

    /// Accumulated rotation at a time inside the maneuver, radians.
    pub fn angle_at(&self, t: f64) -> f64 {
        let sign = if self.total_angle < 0.0 { -1.0 } else { 1.0 };
        let alpha = self.peak_omega / self.ramp_s.max(1e-6);
        if t <= 0.0 {
            return 0.0;
        }
        if t < self.ramp_s {
            return sign * 0.5 * alpha * t * t;
        }
        let ramp_angle = 0.5 * alpha * self.ramp_s * self.ramp_s;
        if t < self.ramp_s + self.cruise_s {
            return sign * (ramp_angle + self.peak_omega * (t - self.ramp_s));
        }
        // Area still to be swept before the turn ends: the deceleration ramp
        // integrates to 0.5 * alpha * remaining^2, so the rotated angle is the
        // total minus that remainder.
        let remaining = (self.duration_s() - t).max(0.0);
        let remaining_angle = 0.5 * alpha * remaining * remaining;
        sign * (self.total_angle.abs() - remaining_angle).max(0.0)
    }
}

/// A place where the path reverses direction.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TurnCandidate {
    /// Arc length of the reversal.
    pub s: f64,
    /// Turn angle in radians, signed.
    pub angle: f64,
    /// Position of the reversal.
    pub position: DVec2,
}

/// Finds reversals along a path.
///
/// Two criteria, because a runner reverses in two different shapes:
///
/// * a **corner**: the heading change across a window of arc length exceeds the
///   threshold. A gradual 180-degree bend does not qualify, which is correct — it
///   is a curve, not a maneuver;
/// * a **fold**: the path returns to within a short distance of where it was, some
///   way further along. This is the shape of a dead end — an alcove, a driveway, a
///   viewpoint — and it is shorter than the corner window: the tangents either side
///   of a two-metre alcove point the same way, so the heading test looks straight
///   past it and the runner is left to reverse without a maneuver. The body then
///   swings through most of a half turn in a few tenths of a second, and since the
///   lateral offset rides on the body normal, that swing alone is over a metre per
///   second of sideways motion at what is supposed to be a standstill.
pub fn detect_turns(
    path: &crate::path::Path,
    window_m: f64,
    min_angle_deg: f64,
) -> Vec<TurnCandidate> {
    let mut out = Vec::new();
    let total = path.total_length();
    if total <= window_m * 2.0 {
        return out;
    }
    let threshold = min_angle_deg.to_radians();
    // The fold test needs the scan to land on the fold's mouth, which can be
    // anywhere; the corner test is insensitive to it because it looks a whole
    // window either side. A step of a fraction of the window serves both.
    let step = (window_m / 8.0).max(0.25);
    let mut s = step;
    while s < total - window_m {
        let before = path.tangent_at(s - window_m);
        let after = path.tangent_at(s + window_m);
        let angle = crate::math::angle_difference(angle_of(after), angle_of(before));
        if angle.abs() >= threshold {
            // The two tangents say a reversal happens somewhere in the window;
            // where inside it is what the runner has to turn. Taking the scan
            // position would put the pivot up to a window length away from the
            // fold — in an alcove narrower than the window the runner would turn
            // around first and then walk the rest of the way in facing backwards,
            // which the offset, carried on the body normal, turns into a metre of
            // sideways sweep at what is supposed to be a standstill.
            let pivot = sharpest_change(path, s, window_m);
            out.push(TurnCandidate {
                s: pivot,
                angle,
                position: path.position_at(pivot),
            });
            // Skip past this turn so one reversal is not reported repeatedly.
            s = pivot + window_m * 4.0;
            continue;
        }
        if let Some(fold) = fold_at(path, s) {
            out.push(fold);
            s = fold.s + window_m;
            continue;
        }
        s += step;
    }
    out
}

/// Arc of the sharpest direction change inside `[s - window, s + window]`.
///
/// The corner test compares the tangents a whole window apart, which finds the
/// reversal but not its position. This walks the window at a fraction of a metre
/// and keeps the arc where the direction changes fastest: a hairpin's tip for an
/// out-and-back, the middle of the bend for a corner.
fn sharpest_change(path: &crate::path::Path, s: f64, window_m: f64) -> f64 {
    // A tenth of a metre: the pivot has to sit on the fold, because everything
    // between it and the fold is still travelled facing the old direction, and the
    // offset rides on the body normal while the attitude turns.
    const PROBE_M: f64 = 0.1;
    let total = path.total_length();
    let mut best = (f64::NEG_INFINITY, s);
    let mut arc = (s - window_m).max(0.0);
    let end = (s + window_m).min(total);
    while arc <= end {
        let before = crate::math::angle_of(path.tangent_at((arc - PROBE_M).max(0.0)));
        let after = crate::math::angle_of(path.tangent_at((arc + PROBE_M).min(total)));
        let change = crate::math::angle_difference(after, before).abs();
        if change > best.0 {
            best = (change, arc);
        }
        arc += PROBE_M;
    }
    best.1
}

/// Longest fold looked for, in arc length, metres.
///
/// The distance a dead end can reach and still be shorter than the corner window.
const FOLD_REACH_M: f64 = 3.0;

/// How close a path has to return to where it was for a fold, metres.
const FOLD_RETURN_M: f64 = 0.5;

/// Recognises an out-and-back around `s`, if one starts there.
///
/// The deepest point of the fold is the turn: the runner walks in, turns on the
/// spot, and walks out, which is what the maneuver produces.
fn fold_at(path: &crate::path::Path, s: f64) -> Option<TurnCandidate> {
    let total = path.total_length();
    let start = path.position_at(s);
    let mut reach = 0.5;
    while reach <= FOLD_REACH_M {
        if s + reach <= total {
            let tip = path.position_at(s + reach);
            if tip.distance(start) < FOLD_RETURN_M {
                let angle = crate::math::angle_difference(
                    angle_of(path.tangent_at(s + reach)),
                    angle_of(path.tangent_at(s)),
                );
                if angle.abs() > std::f64::consts::FRAC_PI_2 {
                    return Some(TurnCandidate {
                        s: s + reach * 0.5,
                        angle,
                        position: path.position_at(s + reach * 0.5),
                    });
                }
            }
        }
        reach += 0.5;
    }
    None
}

/// A scheduled maneuver on the timeline.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Maneuver {
    /// Standing still before the run starts.
    StandStart {
        /// Duration, seconds.
        duration_s: f64,
        /// Where the runner waits.
        position: DVec2,
        /// Facing, radians.
        heading: f64,
    },
    /// Standing still after the run ends.
    StandEnd {
        /// Duration, seconds.
        duration_s: f64,
        /// Where the runner waits.
        position: DVec2,
        /// Facing, radians.
        heading: f64,
    },
    /// On-the-spot turn.
    Turn {
        /// Arc length of the reversal along the path, metres.
        arc_s: f64,
        /// Position of the turn.
        position: DVec2,
        /// Heading the runner faces once the turn is complete, radians. The turn
        /// rotates from the heading actually held on arrival to this one, which
        /// is what keeps the entry and the exit continuous.
        exit_heading: f64,
        /// Turn profile solved from the full reversal angle.
        ///
        /// The executed rotation is the remainder between the arrival heading and
        /// [`Maneuver::Turn::exit_heading`], so the profile is re-solved against
        /// that remainder when the timeline is assembled.
        profile: TurnProfile,
    },
}

impl Maneuver {
    /// Duration of the maneuver, seconds.
    pub fn duration_s(&self) -> f64 {
        match self {
            Maneuver::StandStart { duration_s, .. } | Maneuver::StandEnd { duration_s, .. } => {
                *duration_s
            }
            Maneuver::Turn { profile, .. } => profile.duration_s(),
        }
    }

    /// Arc length the maneuver is attached to, when it has one.
    pub fn arc_s(&self) -> Option<f64> {
        match self {
            Maneuver::Turn { arc_s, .. } => Some(*arc_s),
            _ => None,
        }
    }

    /// Fixed position of the maneuver, if it has one.
    pub fn position(&self) -> DVec2 {
        match self {
            Maneuver::StandStart { position, .. }
            | Maneuver::StandEnd { position, .. }
            | Maneuver::Turn { position, .. } => *position,
        }
    }
}

/// Configuration of the low-speed maneuvers.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ManeuverConfig {
    /// Standing time before the run, seconds.
    pub start_stand_s: f64,
    /// Standing time after the run, seconds.
    pub end_stand_s: f64,
    /// Peak angular velocity of a turn, rad/s.
    pub turn_omega_max: f64,
    /// Angular acceleration of a turn, rad/s^2.
    pub turn_alpha: f64,
    /// Detection window for reversals, metres.
    pub turn_window_m: f64,
    /// Detection threshold for reversals, degrees.
    pub turn_angle_deg: f64,
    /// Whether to schedule maneuvers at all.
    pub enabled: bool,
}

impl Default for ManeuverConfig {
    fn default() -> Self {
        Self {
            start_stand_s: 12.0,
            end_stand_s: 8.0,
            turn_omega_max: 2.4,
            turn_alpha: TURN_ALPHA,
            turn_window_m: 6.0,
            turn_angle_deg: TURN_DETECTION_ANGLE_DEG,
            enabled: true,
        }
    }
}

/// Schedules the maneuvers of a run.
///
/// Standing times are drawn per individual, so a population does not all start
/// moving on the same second.
pub fn schedule(
    path: &crate::path::Path,
    config: &ManeuverConfig,
    rng: &mut crate::rng::Rng,
) -> Vec<Maneuver> {
    if !config.enabled {
        return Vec::new();
    }
    let mut out = Vec::new();
    if config.start_stand_s > 0.0 {
        // Drawn around the configured mean so a population does not all start
        // moving on the same second; a zero configuration means "already moving",
        // which is what a re-planned trajectory needs.
        let start_stand = rng.uniform_range(
            (config.start_stand_s * 0.5).max(1.0),
            config.start_stand_s * 1.5,
        );
        out.push(Maneuver::StandStart {
            duration_s: start_stand,
            position: path.start(),
            heading: angle_of(path.tangent_at(0.0)),
        });
    }
    if config.end_stand_s > 0.0 {
        out.push(Maneuver::StandEnd {
            duration_s: config.end_stand_s,
            position: path.end(),
            heading: angle_of(path.tangent_at(path.total_length())),
        });
    }

    for candidate in detect_turns(path, config.turn_window_m, config.turn_angle_deg) {
        if let Ok(profile) =
            TurnProfile::solve(candidate.angle, config.turn_omega_max, config.turn_alpha)
        {
            // The heading the path has once the reversal is behind the runner.
            // The maneuver is measured across `+-window`, but the runner is
            // already part-way through that swing when it arrives at `s`, so the
            // turn only has to supply the remainder: rotating by the full window
            // angle would overshoot and snap back on the next sample.
            let exit_s = (candidate.s + config.turn_window_m).min(path.total_length());
            out.push(Maneuver::Turn {
                arc_s: candidate.s,
                position: candidate.position,
                exit_heading: angle_of(path.tangent_at(exit_s)),
                profile,
            });
        }
    }
    out
}
