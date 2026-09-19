//! Gyroscope: body angular velocity.
//!
//! The yaw rate is the derivative of the *body heading ground truth*, which is
//! the low-passed tangent angle of the offset trajectory. Two consequences
//! follow, both of them the point of the design:
//!
//! * in steady state the yaw rate equals `v * kappa_eff`, so the gyroscope is
//!   consistent with the trajectory the simulator reports — using the centre-line
//!   curvature instead would make the two disagree on every bend;
//! * during an on-the-spot turn the heading rotates even though `v = 0`, and
//!   because the rate is taken from the attitude it comes out right without a
//!   special case at the sensor. The maneuver is switched in upstream, where the
//!   attitude is built.
//!
//! Pitch and roll rates come from the same attitude sequence, so all three axes
//! are mutually consistent.

use glam::DMat3;

use serde::{Deserialize, Serialize};

use crate::motion::bounce::BounceConfig;
use crate::noise::{OuParams, OuVector3};
use crate::rng::{Rng, Stream};

use super::truth::TruthState;

/// One gyroscope sample in the body frame.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ImuGyroSample {
    /// Time since the start of the recording, seconds.
    pub time_s: f64,
    /// Roll rate, rad/s.
    pub x: f64,
    /// Pitch rate, rad/s.
    pub y: f64,
    /// Yaw rate, rad/s.
    pub z: f64,
}

impl ImuGyroSample {
    /// Angular velocity as an array.
    pub fn omega(&self) -> [f64; 3] {
        [self.x, self.y, self.z]
    }
}

/// Gyroscope error state.
#[derive(Debug, Clone)]
pub struct GyroscopeModel {
    bias: OuVector3,
    white_sigma: f64,
    previous: Option<(f64, [f64; 3])>,
    use_head_heading: bool,
    step_amplitude_rps: f64,
}

/// Share of the step oscillation that appears about the roll axis.
///
/// The oscillation is mostly a yaw of the trunk; the arms swing against it, which
/// the roll axis sees. The split only affects the shape of the three-axis signal,
/// not the cadence, and is a placeholder until a real recording is decomposed by
/// axis.
const STEP_ROLL_SHARE: f64 = 0.3;

impl GyroscopeModel {
    /// Creates the model for one individual.
    ///
    /// The step oscillation starts at the individual default and can be replaced
    /// with [`GyroscopeModel::with_step_amplitude`].
    pub fn new(bias_sigma: f64, white_sigma: f64, use_head_heading: bool) -> Self {
        Self {
            bias: OuVector3::new(OuParams::centred(bias_sigma, 600.0)),
            white_sigma,
            previous: None,
            use_head_heading,
            step_amplitude_rps: 0.0,
        }
    }

    /// Replaces the step-oscillation amplitude.
    pub fn with_step_amplitude(mut self, amplitude_rps: f64) -> Self {
        self.step_amplitude_rps = amplitude_rps.max(0.0);
        self
    }

    /// Builds the model from the individual's sensor parameters.
    ///
    /// The step oscillation is one of them (`gyro_step_amplitude_rps`) rather than
    /// a constant, so a caller can take it out — a test that checks the turn-rate
    /// relation wants the gyroscope's other components held fixed, the same way
    /// [`crate::sensor::SensorConfig::clean`] takes the noise out.
    pub fn for_person(params: &crate::person::SensorNoiseParams, head_mounted: bool) -> Self {
        Self::new(
            params.gyro_bias_sigma,
            params.gyro_white_sigma,
            head_mounted,
        )
        .with_step_amplitude(params.gyro_step_amplitude_rps)
    }

    /// Measures the angular velocity at a truth state.
    ///
    /// `rotation` is the body attitude; the rate is taken from the change of the
    /// Euler angles that define it, so the gyroscope and the accelerometer can
    /// never disagree about which way the body is turning.
    #[allow(clippy::too_many_arguments)]
    pub fn measure(
        &mut self,
        state: &TruthState,
        _rotation: DMat3,
        bounce: &BounceConfig,
        target_speed: f64,
        dt: f64,
        rng: &mut Rng,
    ) -> [f64; 3] {
        let yaw = if self.use_head_heading {
            state.head_heading
        } else {
            state.heading
        };
        let current = [state.roll, state.pitch, yaw];
        let rate = match self.previous {
            Some((previous_time, previous)) => {
                let interval = (state.time_s - previous_time).max(1e-6);
                [
                    crate::math::angle_difference(current[0], previous[0]) / interval,
                    crate::math::angle_difference(current[1], previous[1]) / interval,
                    crate::math::angle_difference(current[2], previous[2]) / interval,
                ]
            }
            None => [0.0; 3],
        };
        self.previous = Some((state.time_s, current));

        // The step oscillation rides on the turn rate. Its phase is the bounce's
        // `phi0`, the same individual constant the accelerometer's fundamental uses,
        // so the two inertial channels describe one gait instead of two.
        let pace = if target_speed > 1e-6 {
            (state.speed / target_speed).clamp(0.0, 1.5)
        } else {
            0.0
        };
        let amplitude = self.step_amplitude_rps * pace;
        let phase = std::f64::consts::TAU * bounce.step_frequency * state.time_s + bounce.phase0;
        let oscillation = amplitude * phase.sin();
        let step = [oscillation * STEP_ROLL_SHARE, 0.0, oscillation];

        let bias = self.bias.step(dt, rng);
        [
            rate[0] + step[0] + bias[0] + rng.gaussian() * self.white_sigma,
            rate[1] + step[1] + bias[1] + rng.gaussian() * self.white_sigma,
            rate[2] + step[2] + bias[2] + rng.gaussian() * self.white_sigma,
        ]
    }

    /// True when the model reports the head heading rather than the body heading.
    pub fn is_head_mounted(&self) -> bool {
        self.use_head_heading
    }
}

/// RNG stream for gyroscope noise.
pub fn stream(seed: u64, individual: u32) -> Rng {
    Rng::stream(seed, Stream::Gyroscope, individual, 0)
}
