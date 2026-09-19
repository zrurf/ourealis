//! Population sampling of individual parameters.
//!
//! Batch simulation needs a *population*, not one runner repeated. Parameters
//! are drawn around the preset means with the spreads of the reference table and
//! truncated to physically meaningful ranges; the cadence keeps its weak
//! positive correlation with speed, which is the one structural relation the
//! reference data supports.

use crate::error::Result;
use crate::rng::{Rng, Stream};

use super::params::{PaceStrategy, PersonParams, Preset};

/// Spread of each sampled parameter, as a standard deviation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PopulationSpread {
    /// Standard deviation of the target speed, m/s.
    pub target_speed: f64,
    /// Standard deviation of the step frequency, Hz.
    pub step_frequency: f64,
    /// Standard deviation of the bounce amplitude, m.
    pub bounce_amplitude: f64,
    /// Probability of drawing a positive split rather than a negative one.
    pub positive_split_share: f64,
    /// Standard deviation of the Logit temperature, equivalent metres.
    pub beta_logit: f64,
}

impl Default for PopulationSpread {
    fn default() -> Self {
        Self {
            target_speed: 0.5,
            step_frequency: 0.1,
            bounce_amplitude: 0.01,
            positive_split_share: 0.6,
            beta_logit: 25.0,
        }
    }
}

/// Draws individuals around a preset.
#[derive(Debug, Clone, Copy)]
pub struct PersonSampler {
    preset: Preset,
    spread: PopulationSpread,
}

impl PersonSampler {
    /// Creates a sampler around a preset.
    pub fn new(preset: Preset, spread: PopulationSpread) -> Self {
        Self { preset, spread }
    }

    /// Creates a sampler with the default spread.
    pub fn preset(preset: Preset) -> Self {
        Self::new(preset, PopulationSpread::default())
    }

    /// Draws one individual.
    ///
    /// `index` selects the stream, so individual `i` of a batch is the same
    /// person no matter how the batch is scheduled.
    pub fn sample(&self, rng: &mut Rng, index: u32) -> Result<PersonParams> {
        let base = PersonParams::preset(self.preset);
        let mut params = base.clone();

        params.label = Some(format!("{:?}-{index}", self.preset));
        params.target_speed =
            (base.target_speed + rng.normal(0.0, self.spread.target_speed)).clamp(1.6, 6.5);

        // Cadence rises weakly with speed: about 10 % from jog to race pace.
        let speed_ratio = params.target_speed / base.target_speed;
        params.step_frequency = (base.step_frequency * (0.85 + 0.15 * speed_ratio)
            + rng.normal(0.0, self.spread.step_frequency))
        .clamp(2.0, 3.4);

        params.a_max = (base.a_max * rng.uniform_range(0.85, 1.15)).clamp(0.6, 3.0);
        params.a_lat_max = (base.a_lat_max * rng.uniform_range(0.85, 1.15)).clamp(1.0, 4.0);
        params.beta_logit =
            (base.beta_logit + rng.normal(0.0, self.spread.beta_logit)).clamp(20.0, 400.0);
        params.lateral_offset_mean = rng.normal(base.lateral_offset_mean, 0.15).clamp(-1.5, 1.5);
        params.lateral_offset_std = rng.normal(base.lateral_offset_std, 0.05).clamp(0.05, 1.2);
        params.bounce_amplitude_m = (base.bounce_amplitude_m
            + rng.normal(0.0, self.spread.bounce_amplitude))
        .clamp(0.01, 0.12);
        params.critical_speed_ratio =
            (base.critical_speed_ratio + rng.normal(0.0, 0.04)).clamp(0.5, 0.95);
        params.fatigue_tau_s =
            (base.fatigue_tau_s * rng.uniform_range(0.8, 1.25)).clamp(200.0, 3000.0);
        params.k_down = (base.k_down + rng.normal(0.0, 0.03)).clamp(1.02, 1.3);

        params.pace_strategy = if rng.chance(self.spread.positive_split_share) {
            PaceStrategy::PositiveSplit
        } else {
            PaceStrategy::NegativeSplit
        };
        params.split_amplitude = rng.uniform_range(0.02, 0.09);

        params.validate()?;
        Ok(params)
    }

    /// Draws a whole population.
    pub fn sample_population(&self, seed: u64, count: usize) -> Result<Vec<PersonParams>> {
        let mut out = Vec::with_capacity(count);
        for index in 0..count {
            let mut rng = Rng::stream(seed, Stream::Person, index as u32, 0);
            out.push(self.sample(&mut rng, index as u32)?);
        }
        Ok(out)
    }
}
