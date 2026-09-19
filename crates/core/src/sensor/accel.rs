//! Accelerometer: specific force in the body frame.
//!
//! An accelerometer does not measure gravity; it measures the specific force
//! `f = a - g` with `g = (0, 0, -9.81)`, so a device at rest reads `+9.81` on its
//! vertical axis. The sign convention is written once, here and in
//! [`super::truth::specific_force`], because getting it wrong flips the entire
//! dataset.
//!
//! The acceleration used is the low-frequency centre-of-mass motion. Adding the
//! bounce's own acceleration and the calibrated higher harmonics on top gives the
//! step signature that downstream cadence detectors rely on.

use glam::DMat3;

use serde::{Deserialize, Serialize};

use crate::motion::BounceConfig;
use crate::noise::harmonics::StepHarmonics;
use crate::noise::{OuParams, OuVector3};
use crate::rng::{Rng, Stream};

use super::truth::TruthState;

/// One accelerometer sample in the body frame.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ImuAccelSample {
    /// Time since the start of the recording, seconds.
    pub time_s: f64,
    /// Body-frame acceleration including gravity reaction, m/s^2.
    pub x: f64,
    /// Body-frame acceleration, m/s^2.
    pub y: f64,
    /// Body-frame acceleration, m/s^2.
    pub z: f64,
}

impl ImuAccelSample {
    /// Acceleration as an array.
    pub fn accel(&self) -> [f64; 3] {
        [self.x, self.y, self.z]
    }

    /// Magnitude of the measured specific force, m/s^2.
    pub fn magnitude(&self) -> f64 {
        (self.x * self.x + self.y * self.y + self.z * self.z).sqrt()
    }
}

/// Accelerometer error state.
#[derive(Debug, Clone)]
pub struct AccelerometerModel {
    bias: OuVector3,
    white_sigma: f64,
}

impl AccelerometerModel {
    /// Creates the model for one individual.
    pub fn new(bias_sigma: f64, bias_tau_s: f64, white_sigma: f64) -> Self {
        Self {
            bias: OuVector3::new(OuParams::centred(bias_sigma, bias_tau_s)),
            white_sigma,
        }
    }

    /// Builds the model from individual sensor parameters.
    pub fn for_person(params: &crate::person::SensorNoiseParams) -> Self {
        Self::new(params.accel_bias_sigma, 300.0, params.accel_white_sigma)
    }

    /// Measures the specific force at a truth state.
    ///
    /// `world_acceleration` is the low-frequency centre-of-mass acceleration;
    /// `rotation` maps body axes to world axes, so its transpose brings the
    /// specific force into the body frame.
    #[allow(clippy::too_many_arguments)]
    pub fn measure(
        &mut self,
        state: &TruthState,
        rotation: DMat3,
        harmonics: &StepHarmonics,
        bounce: &BounceConfig,
        target_speed: f64,
        dt: f64,
        rng: &mut Rng,
    ) -> [f64; 3] {
        let world = super::truth::specific_force(state.acceleration);
        let body = rotation.transpose() * glam::DVec3::new(world[0], world[1], world[2]);
        let bias = self.bias.step(dt, rng);

        // The step signature acts along the body vertical axis, which is where a
        // torso- or head-mounted device sees it.
        let oscillation = if state.standing {
            0.0
        } else {
            harmonics.vertical_acceleration(state.time_s, state.speed, target_speed, bounce)
        };

        [
            body.x + bias[0] + rng.gaussian() * self.white_sigma,
            body.y + bias[1] + rng.gaussian() * self.white_sigma,
            body.z + bias[2] + rng.gaussian() * self.white_sigma + oscillation,
        ]
    }
}

/// RNG stream for accelerometer noise.
pub fn stream(seed: u64, individual: u32) -> Rng {
    Rng::stream(seed, Stream::Accelerometer, individual, 0)
}
