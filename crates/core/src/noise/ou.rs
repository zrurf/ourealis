//! Ornstein–Uhlenbeck drift.
//!
//! Every slowly drifting quantity in the simulator — GNSS bias, pace drift,
//! lateral offset, sensor zero-bias — uses this one process with different
//! parameters. It is a mean-reverting random walk: it wanders like a random
//! walk but is pulled back towards its mean, which is what real drift does.
//!
//! The implementation is parameterised by the **steady-state standard
//! deviation** `sigma_s`, not by the continuous diffusion coefficient. The two
//! are related by `sigma_c = sigma_s sqrt(2 theta)`; exposing only one of them
//! is what prevents the classic mistake of setting one and expecting the other.

use crate::rng::Rng;

/// Parameters of an Ornstein–Uhlenbeck process.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OuParams {
    /// Steady-state standard deviation of the process.
    pub sigma_steady: f64,
    /// Mean-reversion time constant, seconds.
    pub tau_s: f64,
    /// Long-run mean the process reverts to.
    pub mean: f64,
}

impl OuParams {
    /// Creates parameters.
    pub fn new(sigma_steady: f64, tau_s: f64, mean: f64) -> Self {
        Self {
            sigma_steady,
            tau_s: tau_s.max(1e-6),
            mean,
        }
    }

    /// Parameters of a process that only diffuses without a mean offset.
    pub fn centred(sigma_steady: f64, tau_s: f64) -> Self {
        Self::new(sigma_steady, tau_s, 0.0)
    }

    /// Regression rate `theta = 1 / tau`.
    #[inline]
    pub fn theta(&self) -> f64 {
        1.0 / self.tau_s
    }

    /// Equivalent continuous diffusion coefficient.
    #[inline]
    pub fn sigma_continuous(&self) -> f64 {
        self.sigma_steady * (2.0 * self.theta()).sqrt()
    }

    /// Steady-state variance.
    #[inline]
    pub fn steady_variance(&self) -> f64 {
        self.sigma_steady * self.sigma_steady
    }
}

/// Stateful Ornstein–Uhlenbeck drift.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OuProcess {
    params: OuParams,
    value: f64,
}

impl OuProcess {
    /// Starts the process at its mean.
    pub fn new(params: OuParams) -> Self {
        Self {
            value: params.mean,
            params,
        }
    }

    /// Starts the process at an explicit value.
    pub fn starting_at(params: OuParams, value: f64) -> Self {
        Self { params, value }
    }

    /// Current value.
    #[inline]
    pub fn value(&self) -> f64 {
        self.value
    }

    /// Parameters.
    #[inline]
    pub fn params(&self) -> OuParams {
        self.params
    }

    /// Advances the process by `dt` and returns the new value.
    ///
    /// Uses the exact discrete solution of the Ornstein–Uhlenbeck SDE,
    /// `x' = mean + (x - mean) e^{-theta dt} + sigma_s sqrt(1 - e^{-2 theta dt}) N(0,1)`,
    /// whose stationary standard deviation is `sigma_s` for *any* `dt`. The
    /// Euler form the design quotes (`sigma_s sqrt(2 theta dt)`) is its first
    /// order expansion and only holds for `theta dt << 1`; it inflates the
    /// realised spread as `theta dt` approaches one, and at `dt >= tau` it keeps
    /// growing with `sqrt(dt)` instead of saturating.
    pub fn step(&mut self, dt: f64, rng: &mut Rng) -> f64 {
        if dt <= 0.0 {
            return self.value;
        }
        let (decay, step_sigma) = self.exact_coefficients(dt);
        let noise = step_sigma * rng.gaussian();
        self.value = self.params.mean + (self.value - self.params.mean) * decay + noise;
        self.value
    }

    /// Advances the process with a pre-drawn standard normal sample.
    ///
    /// Used where the noise has to come from a batch generator (the GPU path),
    /// so the CPU and GPU code consume identical streams.
    pub fn step_with_sample(&mut self, dt: f64, sample: f64) -> f64 {
        if dt <= 0.0 {
            return self.value;
        }
        let (decay, step_sigma) = self.exact_coefficients(dt);
        let noise = step_sigma * sample;
        self.value = self.params.mean + (self.value - self.params.mean) * decay + noise;
        self.value
    }

    /// Decay and innovation scale of the exact discrete update.
    fn exact_coefficients(&self, dt: f64) -> (f64, f64) {
        let theta = self.params.theta();
        let decay = (-theta * dt).exp();
        let step_sigma = self.params.sigma_steady * (1.0 - decay * decay).max(0.0).sqrt();
        (decay, step_sigma)
    }

    /// Replaces the current value.
    pub fn set(&mut self, value: f64) {
        self.value = value;
    }

    /// Resets to the mean, dropping all history.
    pub fn reset(&mut self) {
        self.value = self.params.mean;
    }
}

/// Two-dimensional Ornstein–Uhlenbeck drift with independent channels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OuVector2 {
    x: OuProcess,
    y: OuProcess,
}

impl OuVector2 {
    /// Creates a 2D process sharing one parameter set.
    pub fn new(params: OuParams) -> Self {
        Self {
            x: OuProcess::new(params),
            y: OuProcess::new(params),
        }
    }

    /// Advances both channels.
    pub fn step(&mut self, dt: f64, rng: &mut Rng) -> [f64; 2] {
        [self.x.step(dt, rng), self.y.step(dt, rng)]
    }

    /// Current value.
    pub fn value(&self) -> [f64; 2] {
        [self.x.value(), self.y.value()]
    }

    /// Replaces both channels.
    pub fn set(&mut self, value: [f64; 2]) {
        self.x.set(value[0]);
        self.y.set(value[1]);
    }

    /// Resets both channels to their mean.
    pub fn reset(&mut self) {
        self.x.reset();
        self.y.reset();
    }
}

/// Three-dimensional Ornstein–Uhlenbeck drift with independent channels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OuVector3 {
    channels: [OuProcess; 3],
}

impl OuVector3 {
    /// Creates a 3D process sharing one parameter set.
    pub fn new(params: OuParams) -> Self {
        Self {
            channels: [
                OuProcess::new(params),
                OuProcess::new(params),
                OuProcess::new(params),
            ],
        }
    }

    /// Advances all three channels.
    pub fn step(&mut self, dt: f64, rng: &mut Rng) -> [f64; 3] {
        [
            self.channels[0].step(dt, rng),
            self.channels[1].step(dt, rng),
            self.channels[2].step(dt, rng),
        ]
    }

    /// Current value.
    pub fn value(&self) -> [f64; 3] {
        [
            self.channels[0].value(),
            self.channels[1].value(),
            self.channels[2].value(),
        ]
    }

    /// Resets all channels to their mean.
    pub fn reset(&mut self) {
        for channel in self.channels.iter_mut() {
            channel.reset();
        }
    }
}
