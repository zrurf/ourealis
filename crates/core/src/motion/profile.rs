//! Speed profile: forward–backward propagation and time parameterisation.
//!
//! The limit array alone is not a speed profile: a runner cannot jump to a
//! limit, and must begin braking *before* the constraint arrives. Two sweeps fix
//! that — one forward that limits acceleration, one backward that makes every
//! deceleration start early enough. Both use `+2 a d`, the sign difference being
//! only in the direction of propagation.
//!
//! The result is a speed profile in which no point exceeds any limit, no
//! longitudinal acceleration exceeds `a_max`, and the terminal condition
//! `v(L) = 0` produces a natural approach to a stop without extra logic.

use glam::DVec2;

use crate::error::{CoreError, Result};
use crate::math::sampling::lerp;
use crate::path::Path;
use crate::terrain::Terrain;

use super::super::person::PersonParams;
use super::limits::{SpeedLimitParams, components_at};
use super::pace::{
    ConvergenceTracker, FatigueModel, IterationControl, IterationOutcome, PacingModel,
};

/// A forced stop at an arc length.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StopHold {
    /// Arc length of the stop.
    pub s: f64,
    /// How long the runner stays there, seconds.
    pub duration_s: f64,
}

/// A local speed-limit modifier, used for waypoint semantics.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LimitModifier {
    /// First arc length of the affected window.
    pub from_s: f64,
    /// Last arc length of the affected window.
    pub to_s: f64,
    /// Multiplier applied to the limit inside the window.
    pub factor: f64,
}

impl LimitModifier {
    /// Creates a modifier.
    pub fn new(from_s: f64, to_s: f64, factor: f64) -> Self {
        Self {
            from_s: from_s.min(to_s),
            to_s: from_s.max(to_s),
            factor: factor.clamp(0.0, 1.0),
        }
    }

    /// True when the arc length lies inside the window.
    pub fn contains(&self, s: f64) -> bool {
        s >= self.from_s && s <= self.to_s
    }
}

/// Configuration of the profile builder.
#[derive(Debug, Clone, PartialEq)]
pub struct ProfileConfig {
    /// Arc-length spacing of the profile samples, metres.
    pub sample_spacing_m: f64,
    /// Whether the run ends at a standstill.
    pub stop_at_end: bool,
    /// Whether the path is a closed loop, which replaces the zero-speed
    /// boundary with a periodic one.
    pub periodic: bool,
    /// Waypoint speed modifiers.
    pub modifiers: Vec<LimitModifier>,
    /// Forced stops.
    pub stops: Vec<StopHold>,
    /// Speed at the first sample of the path, m/s. `None` means the runner
    /// starts from rest, which is what a standing start needs; a redirect
    /// mid-run sets it to the speed the runner already carries, so the rebuilt
    /// profile does not brake to a halt and accelerate again.
    pub initial_speed: Option<f64>,
    /// Convergence controls of the fatigue iteration.
    pub iteration: IterationControl,
    /// Maximum iterations of the periodic boundary search.
    pub periodic_iterations: usize,
}

impl Default for ProfileConfig {
    fn default() -> Self {
        Self {
            sample_spacing_m: 0.25,
            stop_at_end: true,
            periodic: false,
            modifiers: Vec::new(),
            stops: Vec::new(),
            initial_speed: None,
            iteration: IterationControl::default(),
            periodic_iterations: 3,
        }
    }
}

/// The speed profile of a path.
#[derive(Debug, Clone, PartialEq)]
pub struct SpeedProfile {
    /// Arc length of each sample.
    pub s: Vec<f64>,
    /// Speed at each sample, m/s.
    pub v: Vec<f64>,
    /// Elapsed time at each sample, seconds.
    pub t: Vec<f64>,
    /// Combined speed limit at each sample, m/s.
    pub limits: Vec<f64>,
    /// Total duration including stops, seconds.
    pub total_time_s: f64,
    /// Forced stops along the path.
    pub stops: Vec<StopHold>,
    /// Fixed-point convergence report.
    pub iteration: IterationOutcome,
    /// Median target speed used by the pacing model, m/s.
    pub mean_speed: f64,
    /// Acceleration budget of the individual, m/s^2. Retained so the curve near a
    /// standstill can be reconstructed analytically.
    pub a_max: f64,
}

impl SpeedProfile {
    /// Builds the profile of a path.
    ///
    /// `pace_drift` is a per-sample multiplicative factor on the intended pace,
    /// one entry per profile sample (or empty). It is applied to the *intended*
    /// speed, so the physiological and curvature limits still bound the result:
    /// a runner whose target wanders does not thereby exceed what a hill or a
    /// bend allows.
    pub fn build(
        path: &Path,
        terrain: &Terrain,
        limits: &SpeedLimitParams,
        person: &PersonParams,
        config: &ProfileConfig,
        pace_drift: &[f64],
    ) -> Result<Self> {
        person.validate()?;
        let spacing = config.sample_spacing_m.max(0.05);
        let total_length = path.total_length();
        if total_length <= spacing {
            return Err(CoreError::config("path is shorter than one profile sample"));
        }
        let count = (total_length / spacing).ceil() as usize + 1;
        let s: Vec<f64> = (0..count)
            .map(|index| (index as f64 * spacing).min(total_length))
            .collect();

        let fatigue = FatigueModel::for_person(person);
        let mean_speed = person.target_speed;
        let pacing = PacingModel::for_person(person, mean_speed);
        // A stop is enforced at the profile sample nearest its arc, so that is the
        // arc it actually happens at. The timeline has to agree: it decides when to
        // hold by comparing the runner's arc against the stop, and with a metre of
        // profile spacing the difference between "where the caller asked" and
        // "where the sample grid can place it" is up to half a metre — enough for a
        // 0.2 m window to miss the stop entirely.
        let stops = snap_stops(&config.stops, &s);
        let stop_duration: f64 = stops.iter().map(|stop| stop.duration_s).sum();
        let _ = stop_duration;

        let initial_total = IterationOutcome::initial(total_length, mean_speed) + stop_duration;
        let mut tracker = ConvergenceTracker::new(config.iteration, initial_total);

        let mut best_speeds: Vec<f64> = Vec::new();
        let mut best_times: Vec<f64> = Vec::new();
        let mut best_limits: Vec<f64> = Vec::new();
        let mut outcome = IterationOutcome {
            total_time_s: initial_total,
            iterations: 0,
            converged: false,
            fell_back: false,
            last_change_s: f64::INFINITY,
        };

        for iteration in 1..=config.iteration.max_iterations.max(1) {
            let total_estimate = tracker.previous_total();
            // Time mapping from the previous iteration, scaled to the current
            // total-time estimate; this is what makes the fatigue term track the
            // actual pace distribution instead of the arc-length proportion.
            let time_map: Vec<f64> = if best_times.len() == s.len() {
                let scale = if outcome.total_time_s > 1e-6 {
                    total_estimate / outcome.total_time_s
                } else {
                    1.0
                };
                best_times.iter().map(|t| t * scale).collect()
            } else {
                s.iter()
                    .map(|value| total_estimate * value / total_length.max(1e-6))
                    .collect()
            };

            let mut limit_values = vec![0.0f64; s.len()];
            for (index, arc) in s.iter().enumerate() {
                let fatigue_speed = fatigue.speed_at(time_map[index]);
                let pace_target = pacing.speed_at(if total_estimate > 1e-6 {
                    (time_map[index] / total_estimate).clamp(0.0, 1.0)
                } else {
                    0.0
                });
                let drift = pace_drift
                    .get(index)
                    .copied()
                    .unwrap_or(1.0)
                    .clamp(0.4, 1.6);
                let mut value = components_at(
                    path,
                    terrain,
                    *arc,
                    pace_target * drift,
                    fatigue_speed * drift,
                    limits,
                )
                .combined();
                for modifier in &config.modifiers {
                    if modifier.contains(*arc) {
                        value *= modifier.factor;
                    }
                }
                for stop in &stops {
                    // Only the sample at the stop is zeroed. Zeroing a whole
                    // spacing-wide window would make the sweeps treat two metres
                    // as impassable-at-speed and leave the runner crawling through
                    // it; the acceleration ramp on either side is the sweeps' job.
                    if (arc - stop.s).abs() < spacing * 0.5 {
                        value = 0.0;
                    }
                }
                // A Z-axis link overrides everything above it: a stair's steps
                // are not a slope, so the equivalence the physiology gives is the
                // link's own, and it is slower than any of the limits the
                // surrounding ground would impose.
                if let Some(cap) = path.link_speed_at(*arc) {
                    value = value.min(cap);
                }
                if config.stop_at_end && index == s.len() - 1 && !config.periodic {
                    value = 0.0;
                }
                limit_values[index] = value.max(0.0);
            }

            // Forward sweep: acceleration feasibility and the incline limit.
            let (mut speeds, mut times) = propagate(
                &s,
                &limit_values,
                person.a_max,
                config.periodic,
                config.initial_speed,
                &config.stops,
            );

            // Periodic paths have no standstill: the boundary condition is
            // v(0) == v(L), reached by iterating the sweep with the average of
            // the start and end speeds until they agree.
            if config.periodic {
                let mut start_speed = speeds[0];
                for _ in 0..config.periodic_iterations.max(1) {
                    let (next_speeds, next_times) = propagate_fixed_start(
                        &s,
                        &limit_values,
                        person.a_max,
                        start_speed,
                        &config.stops,
                    );
                    let end_speed = *next_speeds.last().unwrap_or(&start_speed);
                    let gap = (end_speed - start_speed).abs();
                    speeds = next_speeds;
                    times = next_times;
                    if gap < 0.05 {
                        break;
                    }
                    start_speed = 0.5 * (start_speed + end_speed);
                }
            }

            let raw_total = times.last().copied().unwrap_or(0.0);
            outcome = tracker.update(raw_total, iteration);
            best_speeds = speeds;
            best_times = times;
            best_limits = limit_values;

            if outcome.converged && !outcome.fell_back {
                break;
            }
            if outcome.fell_back {
                // Divergence: fall back to even pacing with a single fatigue
                // evaluation, which is stable by construction.
                if config.iteration.report_fallback {
                    tracing::warn!(
                        "pacing iteration diverged at iteration {}; falling back to even pacing",
                        iteration
                    );
                }
                let even = PacingModel {
                    strategy: super::super::person::PaceStrategy::Even,
                    amplitude: 0.0,
                    mean_speed,
                };
                let fallback = Self::build_even(
                    path, terrain, limits, person, config, &even, &fatigue, pace_drift,
                )?;
                return Ok(fallback);
            }
        }

        // The time table already contains the dwell time of every stop, so the
        // total is simply its last entry: adding the stop durations again would
        // count them twice.
        let total_time = best_times.last().copied().unwrap_or(0.0);

        Ok(Self {
            s,
            v: best_speeds,
            t: best_times,
            limits: best_limits,
            total_time_s: total_time,
            stops,
            iteration: outcome,
            mean_speed,
            a_max: person.a_max,
        })
    }

    /// Builds a profile without pace drift.
    ///
    /// Used where the drift is supplied elsewhere or is irrelevant, such as unit
    /// tests that reason about the limit composition itself.
    pub fn build_steady(
        path: &Path,
        terrain: &Terrain,
        limits: &SpeedLimitParams,
        person: &PersonParams,
        config: &ProfileConfig,
    ) -> Result<Self> {
        Self::build(path, terrain, limits, person, config, &[])
    }

    /// Builds a profile with a fixed, even pacing model.
    #[allow(clippy::too_many_arguments)]
    fn build_even(
        path: &Path,
        terrain: &Terrain,
        limits: &SpeedLimitParams,
        person: &PersonParams,
        config: &ProfileConfig,
        pacing: &PacingModel,
        fatigue: &FatigueModel,
        pace_drift: &[f64],
    ) -> Result<Self> {
        let spacing = config.sample_spacing_m.max(0.05);
        let total_length = path.total_length();
        let count = (total_length / spacing).ceil() as usize + 1;
        let s: Vec<f64> = (0..count)
            .map(|index| (index as f64 * spacing).min(total_length))
            .collect();
        let initial_time = IterationOutcome::initial(total_length, person.target_speed);
        let stops = snap_stops(&config.stops, &s);
        let mut limit_values = vec![0.0f64; s.len()];
        for (index, arc) in s.iter().enumerate() {
            let tau = if total_length > 1e-6 {
                arc / total_length
            } else {
                0.0
            };
            let drift = pace_drift
                .get(index)
                .copied()
                .unwrap_or(1.0)
                .clamp(0.4, 1.6);
            let target = pacing.speed_at(tau) * drift;
            let fatigue_speed = fatigue.speed_at(initial_time * tau) * drift;
            let mut value =
                components_at(path, terrain, *arc, target, fatigue_speed, limits).combined();
            for modifier in &config.modifiers {
                if modifier.contains(*arc) {
                    value *= modifier.factor;
                }
            }
            for stop in &stops {
                if (arc - stop.s).abs() < spacing * 0.5 {
                    value = 0.0;
                }
            }
            limit_values[index] = value.max(0.0);
        }
        if config.stop_at_end
            && !config.periodic
            && let Some(last) = limit_values.last_mut()
        {
            *last = 0.0;
        }

        let (speeds, times) = propagate(
            &s,
            &limit_values,
            person.a_max,
            config.periodic,
            config.initial_speed,
            &config.stops,
        );
        let total_time = times.last().copied().unwrap_or(0.0);
        Ok(Self {
            s,
            v: speeds,
            t: times,
            limits: limit_values,
            total_time_s: total_time,
            stops,
            a_max: person.a_max,
            iteration: IterationOutcome {
                total_time_s: total_time,
                iterations: 1,
                converged: true,
                fell_back: true,
                last_change_s: 0.0,
            },
            mean_speed: person.target_speed,
        })
    }

    /// Segment index and position inside it for an arc length.
    fn segment_of(&self, s: f64) -> (usize, f64) {
        if self.s.len() < 2 {
            return (0, 0.0);
        }
        let clamped = s.clamp(0.0, *self.s.last().unwrap_or(&0.0));
        let index = match self.s.binary_search_by(|value| {
            value
                .partial_cmp(&clamped)
                .unwrap_or(std::cmp::Ordering::Equal)
        }) {
            Ok(index) => index.min(self.s.len() - 2),
            Err(index) => index.saturating_sub(1).min(self.s.len() - 2),
        };
        let span = (self.s[index + 1] - self.s[index]).max(1e-9);
        (index, ((clamped - self.s[index]) / span).clamp(0.0, 1.0))
    }

    /// Catmull-Rom evaluation with clamped ends.
    ///
    /// A piecewise-linear profile has a discontinuous derivative, which shows up
    /// as a sawtooth in the acceleration and masks the motion noise in any
    /// autocorrelation analysis; a spline removes it without changing the
    /// sampled values.
    fn spline_at(&self, values: &[f64], index: usize, t: f64) -> f64 {
        if values.len() < 2 {
            return values.first().copied().unwrap_or(0.0);
        }
        let p1 = values[index];
        let p2 = values[(index + 1).min(values.len() - 1)];
        let p0 = values[index.saturating_sub(1)];
        let p3 = values[(index + 2).min(values.len() - 1)];
        let t2 = t * t;
        let t3 = t2 * t;
        let value = 0.5
            * ((2.0 * p1)
                + (-p0 + p2) * t
                + (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3) * t2
                + (-p0 + 3.0 * p1 - 3.0 * p2 + p3) * t3);
        // The spline may overshoot its two control points; clamping keeps every
        // interpolated value inside the range the sweeps actually produced, so a
        // curve through a braking ramp cannot exceed the speeds the propagation
        // deemed reachable.
        let (low, high) = (p1.min(p2), p1.max(p2));
        value.clamp(low, high)
    }

    /// Elapsed time at an arc length.
    ///
    /// Dwell time is already in the table — `propagate` adds it at the stop index
    /// — so nothing further is added here.
    pub fn time_at(&self, s: f64) -> f64 {
        if self.s.is_empty() {
            return 0.0;
        }
        let (index, fraction) = self.segment_of(s);
        if index + 1 >= self.t.len() {
            return *self.t.last().unwrap_or(&0.0);
        }
        lerp(self.t[index], self.t[index + 1], fraction)
    }

    /// Speed ceiling at an arc length, before any motion noise is applied.
    pub fn limit_at(&self, s: f64) -> f64 {
        let (index, fraction) = self.segment_of(s);
        self.spline_at(&self.limits, index, fraction)
    }

    /// Speed at an arc length.
    ///
    /// Interpolated with a Catmull-Rom spline rather than linearly, so the
    /// derivative stays continuous between profile samples. Around a standstill
    /// the spline is replaced by the analytic acceleration ramp: a spline through
    /// a V-shaped dip is flat at its bottom, which would leave the runner
    /// crawling for a metre after every stop instead of accelerating away from it.
    pub fn speed_at(&self, s: f64) -> f64 {
        let (index, fraction) = self.segment_of(s);
        let interpolated = self.spline_at(&self.v, index, fraction);
        let zero_at = |sample: usize| -> Option<f64> {
            (self.v.get(sample).copied().unwrap_or(1.0) < 1e-9).then(|| self.s[sample])
        };
        let anchor = zero_at(index).or_else(|| zero_at(index + 1));
        let Some(anchor) = anchor else {
            return interpolated;
        };
        let distance = (s - anchor).abs();
        let ramp = (2.0 * self.a_max * distance).sqrt();
        // Never exceed the value the sweep actually reached at the far end.
        let ceiling = self.v[index].max(self.v[index + 1]);
        // Clamped by the segment's own ceiling, not by the interpolated limit: a
        // spline through a V-shaped dip undershoots the true ceiling, and clamping
        // to it would leave the runner creeping away from every standstill.
        interpolated.max(ramp.min(ceiling))
    }

    /// Time at which the runner passes an arc length.
    pub fn time_of(&self, s: f64) -> f64 {
        self.time_at(s)
    }

    /// Arc length and speed after a stop, used when sampling a trajectory.
    pub fn is_stopped_at(&self, s: f64, tolerance_m: f64) -> Option<&StopHold> {
        self.stops
            .iter()
            .find(|stop| (stop.s - s).abs() <= tolerance_m)
    }

    /// Arc length of the executed stop nearest to `arc`, within one sample spacing.
    ///
    /// The profile places stops on its own sample grid, so a caller that keeps the
    /// requested arc (a turn maneuver, for example) refers to a point up to half a
    /// spacing away from the sample the runner actually brakes to. The tolerance is
    /// a whole spacing so the association survives the rounding in either direction.
    pub fn stop_arc_near(&self, arc: f64) -> Option<f64> {
        let spacing = match (self.s.first(), self.s.get(1)) {
            (Some(first), Some(second)) => second - first,
            _ => return None,
        };
        self.stops
            .iter()
            .map(|stop| stop.s)
            .filter(|s| (s - arc).abs() <= spacing + 1e-9)
            .min_by(|a, b| {
                (a - arc)
                    .abs()
                    .partial_cmp(&(b - arc).abs())
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
    }

    /// Position and speed at an elapsed time.
    pub fn sample_at_time(&self, t: f64, path: &Path) -> (DVec2, f64) {
        if self.t.is_empty() {
            return (path.start(), 0.0);
        }
        let clamped = t.clamp(0.0, self.total_time_s);
        let index = match self.t.binary_search_by(|value| {
            value
                .partial_cmp(&clamped)
                .unwrap_or(std::cmp::Ordering::Equal)
        }) {
            Ok(index) => index,
            Err(index) => index.saturating_sub(1),
        };
        if index + 1 >= self.t.len() {
            return (path.position_at(*self.s.last().unwrap_or(&0.0)), 0.0);
        }
        let span = (self.t[index + 1] - self.t[index]).max(1e-9);
        let fraction = ((clamped - self.t[index]) / span).clamp(0.0, 1.0);
        let arc = lerp(self.s[index], self.s[index + 1], fraction);
        let speed = lerp(self.v[index], self.v[index + 1], fraction);
        (path.position_at(arc), speed)
    }

    /// Maximum longitudinal acceleration implied by the profile.
    pub fn max_acceleration(&self) -> f64 {
        let mut peak = 0.0f64;
        for index in 1..self.v.len() {
            let dt = (self.t[index] - self.t[index - 1]).max(1e-9);
            let a = (self.v[index] - self.v[index - 1]) / dt;
            peak = peak.max(a.abs());
        }
        peak
    }

    /// Largest excess of speed over the limit, which must stay at zero.
    pub fn max_limit_violation(&self) -> f64 {
        self.v
            .iter()
            .zip(self.limits.iter())
            .map(|(speed, limit)| (speed - limit).max(0.0))
            .fold(0.0f64, f64::max)
    }

    /// Mean speed over the whole path.
    pub fn mean_speed(&self) -> f64 {
        let length = self.s.last().copied().unwrap_or(0.0);
        if self.total_time_s <= 1e-6 {
            0.0
        } else {
            length / self.total_time_s
        }
    }
}

/// Moves each stop onto the nearest profile sample, merging the ones that land
/// on the same sample.
///
/// The profile can only enforce a stop where it has a sample, and the motion
/// timeline decides when to hold by comparing the runner's arc against the stop:
/// both sides have to use the same arc, or the hold is looked for half a metre
/// away from where the speed profile put the standstill.
///
/// Merging is what makes a dwell at a reversal work. The reversal registers a
/// zero-duration stop of its own, and an ordinary waypoint can sit on the same
/// profile sample; two entries at one arc would have the timeline consume the
/// first and then find itself already past the second, dropping a hold the
/// caller asked for. Summing the durations keeps both.
fn snap_stops(stops: &[StopHold], samples: &[f64]) -> Vec<StopHold> {
    if samples.is_empty() {
        return stops.to_vec();
    }
    stops
        .iter()
        .map(|stop| {
            let index = match samples.binary_search_by(|value| {
                value
                    .partial_cmp(&stop.s)
                    .unwrap_or(std::cmp::Ordering::Equal)
            }) {
                Ok(index) => index,
                Err(index) => {
                    if index == 0 {
                        0
                    } else if index >= samples.len() {
                        samples.len() - 1
                    } else if (samples[index] - stop.s).abs() < (stop.s - samples[index - 1]).abs()
                    {
                        index
                    } else {
                        index - 1
                    }
                }
            };
            StopHold {
                s: samples[index],
                duration_s: stop.duration_s,
            }
        })
        .fold(Vec::new(), |mut merged: Vec<StopHold>, stop| {
            match merged.last_mut() {
                Some(last) if (last.s - stop.s).abs() <= 1e-9 => {
                    last.duration_s += stop.duration_s;
                }
                _ => merged.push(stop),
            }
            merged
        })
}

/// Forward and backward sweeps with a zero, fixed or free start.
fn propagate(
    s: &[f64],
    limits: &[f64],
    a_max: f64,
    periodic: bool,
    initial_speed: Option<f64>,
    stops: &[StopHold],
) -> (Vec<f64>, Vec<f64>) {
    // A loop has no boundary; every other path starts from the speed the caller
    // declares, which is zero unless the runner is already moving.
    let start_speed = if periodic {
        None
    } else {
        Some(initial_speed.unwrap_or(0.0).max(0.0))
    };
    propagate_with_start(s, limits, a_max, start_speed, stops)
}

fn propagate_fixed_start(
    s: &[f64],
    limits: &[f64],
    a_max: f64,
    start_speed: f64,
    stops: &[StopHold],
) -> (Vec<f64>, Vec<f64>) {
    propagate_with_start(s, limits, a_max, Some(start_speed), stops)
}

/// Two-sweep propagation.
///
/// Both sweeps use `v' = min(limit, sqrt(v^2 + 2 a d))`; the backward sweep is
/// the same recurrence evaluated from the end, which is what makes deceleration
/// begin early enough.
fn propagate_with_start(
    s: &[f64],
    limits: &[f64],
    a_max: f64,
    start_speed: Option<f64>,
    stops: &[StopHold],
) -> (Vec<f64>, Vec<f64>) {
    let n = s.len();
    let mut v: Vec<f64> = limits.to_vec();
    if let Some(start) = start_speed {
        v[0] = v[0].min(start);
    }

    // Forward: limit acceleration.
    for index in 1..n {
        let d = (s[index] - s[index - 1]).max(0.0);
        let reachable = (v[index - 1] * v[index - 1] + 2.0 * a_max * d).sqrt();
        v[index] = v[index].min(reachable);
    }
    // Backward: guarantee deceleration room.
    for index in (0..n - 1).rev() {
        let d = (s[index + 1] - s[index]).max(0.0);
        let reachable = (v[index + 1] * v[index + 1] + 2.0 * a_max * d).sqrt();
        v[index] = v[index].min(reachable);
    }

    // Rebuild the time table from the final speeds.
    let mut t = vec![0.0f64; n];
    for index in 1..n {
        let d = (s[index] - s[index - 1]).max(0.0);
        let mean = 0.5 * (v[index] + v[index - 1]);
        let step = if mean > 1e-6 { d / mean } else { 0.0 };
        t[index] = t[index - 1] + step;
    }
    // A stop freezes the runner: its dwell time is added at the stop point.
    for stop in stops {
        let index = s
            .iter()
            .position(|arc| (arc - stop.s).abs() <= (s[1] - s[0]).abs() * 0.5 + 1e-9);
        if let Some(index) = index {
            for value in t.iter_mut().skip(index) {
                *value += stop.duration_s;
            }
        }
    }
    (v, t)
}
