//! Pacing, fatigue and the fixed-point iteration that couples them.
//!
//! Fatigue depends on elapsed time and elapsed time depends on the speed
//! profile, so the two are solved together. The iteration is damped because a
//! strong positive split makes the naive update oscillate: a slow first estimate
//! of the total time understates fatigue, which speeds up the profile, which
//! raises the estimate again. Damping plus an explicit divergence test keeps the
//! output *always* kinematically valid — the fallback sacrifices pacing fidelity
//! rather than emitting an inconsistent time profile.

use super::super::person::{PaceStrategy, PersonParams};

/// Fatigue model parameters.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FatigueModel {
    /// Fresh target speed, m/s.
    pub v0: f64,
    /// Speed that can be sustained indefinitely, m/s.
    pub v_crit: f64,
    /// Fatigue time constant, seconds.
    pub tau_f: f64,
}

impl FatigueModel {
    /// Builds the model from individual parameters.
    ///
    /// `v0` is the individual's fresh target speed: the model only ever acts as
    /// an upper bound, and it never forces a runner to hold a pace.
    pub fn for_person(person: &PersonParams) -> Self {
        Self {
            v0: person.target_speed,
            v_crit: person.target_speed * person.critical_speed_ratio,
            tau_f: person.fatigue_tau_s,
        }
    }

    /// Speed cap at an elapsed time.
    pub fn speed_at(&self, t: f64) -> f64 {
        let decay = (-t / self.tau_f.max(1e-6)).exp();
        self.v_crit + (self.v0 - self.v_crit) * decay
    }

    /// Fraction of the fresh speed still available at `t`.
    pub fn retained_fraction(&self, t: f64) -> f64 {
        if self.v0 <= 1e-6 {
            return 1.0;
        }
        self.speed_at(t) / self.v0
    }
}

/// Pacing strategy with its amplitude.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PacingModel {
    /// Strategy shape.
    pub strategy: PaceStrategy,
    /// Relative amplitude of the split.
    pub amplitude: f64,
    /// Mean target speed over the whole run, m/s.
    pub mean_speed: f64,
}

impl PacingModel {
    /// Builds the pacing model of an individual.
    pub fn for_person(person: &PersonParams, mean_speed: f64) -> Self {
        Self {
            strategy: person.pace_strategy,
            amplitude: person.split_amplitude,
            mean_speed,
        }
    }

    /// Instantaneous target speed at normalised time `tau`.
    pub fn speed_at(&self, tau: f64) -> f64 {
        self.mean_speed * self.strategy.factor(tau, self.amplitude)
    }
}

/// Convergence controls of the fixed-point iteration.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct IterationControl {
    /// Damping factor applied to the total-time update.
    pub damping: f64,
    /// Maximum iterations.
    pub max_iterations: usize,
    /// Convergence threshold on the total time, seconds.
    pub convergence_s: f64,
    /// Divergence threshold on the total time change, seconds.
    pub divergence_s: f64,
    /// Whether to warn when the fallback path is taken.
    pub report_fallback: bool,
}

impl Default for IterationControl {
    fn default() -> Self {
        Self {
            damping: 0.5,
            max_iterations: 3,
            convergence_s: 0.5,
            divergence_s: 60.0,
            report_fallback: true,
        }
    }
}

/// Outcome of the fixed-point iteration.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct IterationOutcome {
    /// Total time after the last update, seconds.
    pub total_time_s: f64,
    /// Iterations performed.
    pub iterations: usize,
    /// True when the convergence threshold was met.
    pub converged: bool,
    /// True when the iteration diverged and the even-pace fallback was used.
    pub fell_back: bool,
    /// Change of the total time in the final iteration, seconds.
    pub last_change_s: f64,
}

impl IterationOutcome {
    /// Initial estimate before any iteration: constant target speed.
    pub fn initial(path_length_m: f64, target_speed: f64) -> f64 {
        if target_speed <= 1e-6 {
            0.0
        } else {
            path_length_m / target_speed
        }
    }
}

/// Tracks convergence and divergence across iterations.
#[derive(Debug, Clone, Copy)]
pub struct ConvergenceTracker {
    control: IterationControl,
    previous_total: f64,
    previous_change: f64,
}

impl ConvergenceTracker {
    /// Creates a tracker seeded with the initial time estimate.
    pub fn new(control: IterationControl, initial_total_s: f64) -> Self {
        Self {
            control,
            previous_total: initial_total_s,
            previous_change: f64::INFINITY,
        }
    }

    /// Total time of the previous iteration.
    pub fn previous_total(&self) -> f64 {
        self.previous_total
    }

    /// Damped update of the total time.
    pub fn update(&mut self, raw_total: f64, iteration: usize) -> IterationOutcome {
        let change = raw_total - self.previous_total;
        let damped = self.previous_total + self.control.damping * change;
        let abs_change = change.abs();

        // Divergence: the step grows instead of shrinking, or it explodes.
        let diverging = abs_change > self.control.divergence_s
            || (iteration > 1
                && abs_change > self.previous_change
                && abs_change > self.control.convergence_s);
        let converged = abs_change <= self.control.convergence_s;
        let last_iteration = iteration >= self.control.max_iterations;

        self.previous_change = abs_change;
        self.previous_total = damped;

        IterationOutcome {
            total_time_s: damped,
            iterations: iteration,
            converged: converged || last_iteration,
            fell_back: diverging,
            last_change_s: abs_change,
        }
    }

    /// Control settings.
    pub fn control(&self) -> IterationControl {
        self.control
    }
}
