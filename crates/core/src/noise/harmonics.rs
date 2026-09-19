//! Step-frequency harmonics of the vertical acceleration.
//!
//! Running is periodic, and the accelerometer shows it: a peak at the step
//! frequency with a characteristic harmonic stack. The fundamental is **not** a
//! free parameter — it is the bounce of the centre of mass, taken straight from
//! the motion truth, so amplitude and phase match exactly. Only the higher
//! harmonics, which encode the asymmetry of ground contact, are calibration
//! parameters and are flagged as placeholders until real IMU data is available.

use super::super::motion::BounceConfig;

/// Ratios of the higher harmonics to the fundamental.
///
/// The design document's placeholder values (0.30 and 0.10). They are only
/// defaults for a caller that builds a [`StepHarmonics`] by hand; the individual
/// parameter vector carries the calibrated values, see
/// [`crate::person::PersonParams::harmonic_2_ratio`].
pub const DEFAULT_HARMONIC_2_RATIO: f64 = 0.30;
/// Third harmonic ratio, same caveat as [`DEFAULT_HARMONIC_2_RATIO`].
pub const DEFAULT_HARMONIC_3_RATIO: f64 = 0.10;

/// Relative phases of the higher harmonics, radians.
pub const DEFAULT_HARMONIC_PHASES: [f64; 3] = [0.0, 0.4, -0.3];

/// Harmonic stack of one individual.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StepHarmonics {
    /// Step frequency, Hz.
    pub step_frequency: f64,
    /// Phase reference shared with the bounce, radians.
    pub phase0: f64,
    /// Amplitude ratio of the second harmonic.
    pub ratio_2: f64,
    /// Amplitude ratio of the third harmonic.
    pub ratio_3: f64,
    /// Relative phases of harmonics two and three.
    pub phases: [f64; 2],
}

impl StepHarmonics {
    /// Builds the harmonic stack of an individual.
    pub fn new(step_frequency: f64, phase0: f64, ratio_2: f64, ratio_3: f64) -> Self {
        Self {
            step_frequency: step_frequency.max(0.5),
            phase0,
            ratio_2,
            ratio_3,
            phases: [DEFAULT_HARMONIC_PHASES[1], DEFAULT_HARMONIC_PHASES[2]],
        }
    }

    /// Angular frequency, rad/s.
    pub fn omega(&self) -> f64 {
        std::f64::consts::TAU * self.step_frequency
    }

    /// Vertical acceleration measured at the body.
    ///
    /// The fundamental is the bounce waveform's own second derivative, which locks
    /// amplitude *and* phase to the ground truth. Its sign is `+`: an
    /// accelerometer measures specific force `f = p_ddot - g`, so the oscillating
    /// part of the reading *is* the vertical acceleration of the bounce, not its
    /// negation. Getting this backwards puts the accelerometer 180 degrees out of
    /// phase with the altitude and the barometer that carry the same bounce, and
    /// any downstream height estimate that integrates acceleration inverts.
    ///
    /// The second and third harmonics are added with calibrated ratios.
    pub fn vertical_acceleration(
        &self,
        t: f64,
        speed: f64,
        target_speed: f64,
        bounce: &BounceConfig,
    ) -> f64 {
        let fundamental = bounce.vertical_acceleration_at(t, speed, target_speed);
        let amplitude = bounce.first_harmonic_amplitude(speed, target_speed);
        let omega = self.omega();
        let second =
            amplitude * self.ratio_2 * (2.0 * omega * t + 2.0 * self.phase0 + self.phases[0]).sin();
        let third =
            amplitude * self.ratio_3 * (3.0 * omega * t + 3.0 * self.phase0 + self.phases[1]).sin();
        fundamental + second + third
    }

    /// Amplitude of the fundamental for reporting and evaluation.
    pub fn fundamental_amplitude(
        &self,
        speed: f64,
        target_speed: f64,
        bounce: &BounceConfig,
    ) -> f64 {
        bounce.first_harmonic_amplitude(speed, target_speed)
    }
}
