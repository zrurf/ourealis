//! Individual parameter vector.
//!
//! Everything that makes one runner differ from another lives here: the target
//! pace, the cadence, the acceleration budget, the risk attitude in path choice,
//! the fatigue constants, the lateral offset habit and the sensor noise
//! signature. The presets follow the reference table of the design document;
//! they are catalogue values, not measurements, and the population sampler
//! treats them as distribution means.

use serde::{Deserialize, Serialize};

/// How a runner distributes effort over a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum PaceStrategy {
    /// Constant target speed.
    Even = 0,
    /// Starts fast and fades, common among recreational runners.
    PositiveSplit = 1,
    /// Starts steady and finishes faster, common among trained runners.
    NegativeSplit = 2,
}

impl PaceStrategy {
    /// Multiplier applied to the mean target speed at normalised time `tau`.
    ///
    /// All three shapes average to one over the run, so the total distance
    /// covered matches the target speed regardless of strategy.
    pub fn factor(&self, tau: f64, amplitude: f64) -> f64 {
        let tau = tau.clamp(0.0, 1.0);
        match self {
            PaceStrategy::Even => 1.0,
            PaceStrategy::PositiveSplit => 1.0 + amplitude * (1.0 - 2.0 * tau),
            PaceStrategy::NegativeSplit => 1.0 + amplitude * (2.0 * tau - 1.0),
        }
    }

    /// Parses an on-disk identifier.
    pub const fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(PaceStrategy::Even),
            1 => Some(PaceStrategy::PositiveSplit),
            2 => Some(PaceStrategy::NegativeSplit),
            _ => None,
        }
    }
}

/// Sensor noise parameters of one individual, modelling a device class.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SensorNoiseParams {
    /// GNSS slow-bias steady-state standard deviation, metres.
    pub gnss_bias_sigma_m: f64,
    /// GNSS slow-bias time constant, seconds.
    pub gnss_bias_tau_s: f64,
    /// GNSS white noise standard deviation, metres.
    pub gnss_white_sigma_m: f64,
    /// GNSS speed noise standard deviation, m/s.
    pub gnss_speed_sigma: f64,
    /// Whether GNSS velocity is derived from the noisy positions, which is what
    /// makes position and velocity errors correlated.
    pub gnss_correlated_velocity: bool,
    /// Accelerometer bias standard deviation, m/s^2.
    pub accel_bias_sigma: f64,
    /// Accelerometer white noise standard deviation, m/s^2.
    pub accel_white_sigma: f64,
    /// Gyroscope bias standard deviation, rad/s.
    pub gyro_bias_sigma: f64,
    /// Gyroscope white noise standard deviation, rad/s.
    pub gyro_white_sigma: f64,
    /// Peak angular rate of the step-induced body oscillation, rad/s.
    ///
    /// The trunk rocks once per step, so a real gyroscope carries the cadence in
    /// its spectrum. Measured from the WISDM jogging recordings (phone gyroscope,
    /// 20 Hz): the fundamental at the cadence averages 0.48 rad/s across the
    /// subjects whose recordings are complete, range 0.17–0.91. A model without it
    /// produces a gyroscope whose spectrum has nothing at the cadence, which a step
    /// detector reads as a stationary device.
    pub gyro_step_amplitude_rps: f64,
    /// Magnetometer hard-iron bias standard deviation, microtesla.
    pub mag_bias_sigma_ut: f64,
    /// Magnetometer white noise standard deviation, microtesla.
    pub mag_white_sigma_ut: f64,
    /// Barometer white noise standard deviation, pascals.
    pub baro_white_sigma_pa: f64,
}

impl Default for SensorNoiseParams {
    fn default() -> Self {
        // Consumer phone with a wrist-worn or hand-held unit.
        Self {
            gnss_bias_sigma_m: 5.0,
            gnss_bias_tau_s: 120.0,
            gnss_white_sigma_m: 3.0,
            gnss_speed_sigma: 0.15,
            gnss_correlated_velocity: true,
            accel_bias_sigma: 0.02,
            accel_white_sigma: 0.03,
            gyro_bias_sigma: 0.002,
            gyro_white_sigma: 0.005,
            gyro_step_amplitude_rps: 0.5,
            mag_bias_sigma_ut: 1.5,
            mag_white_sigma_ut: 0.3,
            baro_white_sigma_pa: 2.0,
        }
    }
}

/// Preset running styles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Preset {
    /// Easy running: gentle pace, short look-ahead, soft-surface preference.
    Jog,
    /// Steady training pace.
    Moderate,
    /// Racing pace.
    Race,
}

/// Complete parameter vector of one simulated individual.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PersonParams {
    /// Optional label, carried into the output manifest.
    pub label: Option<String>,

    /// Target speed on flat ground with no fatigue, m/s.
    pub target_speed: f64,
    /// Step frequency, Hz.
    pub step_frequency: f64,
    /// Maximum longitudinal acceleration, m/s^2.
    pub a_max: f64,
    /// Maximum comfortable lateral acceleration, m/s^2. Lower than cycling
    /// because the flight phase offers no lateral support.
    pub a_lat_max: f64,
    /// Slope look-ahead distance, m.
    pub look_ahead_m: f64,
    /// Logit rationality temperature in equivalent metres.
    pub beta_logit: f64,

    /// Mean lateral offset from the path centre line, metres; positive is left.
    pub lateral_offset_mean: f64,
    /// Steady-state standard deviation of the lateral offset, metres.
    pub lateral_offset_std: f64,
    /// Time constant of the offset drift, seconds.
    pub lateral_offset_tau_s: f64,
    /// Steady-state standard deviation of the pace drift, as a fraction of the
    /// target speed. The design's range is 3–8 %.
    pub pace_drift_sigma: f64,
    /// Time constant of the pace drift, seconds; the design's range is 30–120 s.
    pub pace_drift_tau_s: f64,

    /// Critical speed as a fraction of the fresh target speed.
    pub critical_speed_ratio: f64,
    /// Fatigue time constant, seconds.
    pub fatigue_tau_s: f64,
    /// Pace distribution over the run.
    pub pace_strategy: PaceStrategy,
    /// Amplitude of the pace split, as a fraction of the target speed.
    pub split_amplitude: f64,

    /// Downhill speed-cap coefficient.
    pub k_down: f64,
    /// Maximum forward lean at target speed, degrees.
    pub lean_max_deg: f64,
    /// Reference bounce amplitude at target speed, metres.
    pub bounce_amplitude_m: f64,
    /// Head-orientation look-ahead time, seconds.
    pub head_look_ahead_s: f64,
    /// Peak angular velocity of an on-the-spot turn, rad/s.
    pub turn_omega_max: f64,

    /// Ratio of the accelerometer's second step harmonic to the first.
    ///
    /// Calibrated to **zero** against the reference recordings (WISDM 2.0 and
    /// MotionSense jogging, 40 series): the vertical bounce is already asymmetric,
    /// and a displacement harmonic of order `k` appears as `k^2` in the
    /// acceleration, so the waveform's own asymmetry alone accounts for the whole
    /// measured ratio. Adding this term as well counted the same harmonic twice and
    /// put the simulator at several times the measured value. The knob remains
    /// because a caller may model a runner whose acceleration carries a harmonic the
    /// waveform does not.
    pub harmonic_2_ratio: f64,
    /// Ratio of the accelerometer's third step harmonic to the first.
    ///
    /// Calibrated to 0.107 from the same recordings, which is the measured mean
    /// itself: nothing else in the model produces a third harmonic, so this term
    /// carries it whole.
    pub harmonic_3_ratio: f64,

    /// Sensor noise signature.
    pub sensors: SensorNoiseParams,
}

impl Default for PersonParams {
    fn default() -> Self {
        Self::preset(Preset::Moderate)
    }
}

/// Second step harmonic of the accelerometer, calibrated from the reference
/// recordings. See [`PersonParams::harmonic_2_ratio`].
pub const CALIBRATED_HARMONIC_2_RATIO: f64 = 0.0;

/// Third step harmonic of the accelerometer, calibrated from the reference
/// recordings.
pub const CALIBRATED_HARMONIC_3_RATIO: f64 = 0.107;

impl PersonParams {
    /// Builds the reference parameter vector of a preset.
    pub fn preset(preset: Preset) -> Self {
        let (
            target_speed,
            step_frequency,
            a_max,
            a_lat_max,
            look_ahead,
            beta,
            k_down,
            lean,
            bounce,
            head,
            turn,
            critical,
            tau,
        ) = match preset {
            Preset::Jog => (
                2.5, 2.4, 1.0, 1.5, 15.0, 80.0, 1.10, 1.0, 0.03, 1.5, 1.7, 0.65, 600.0,
            ),
            // The cadence is the calibrated value for a moderate jog: the reference
            // recordings give 2.50 Hz, where the design's table suggests 2.4.
            Preset::Moderate => (
                3.3, 2.50, 1.5, 2.5, 20.0, 120.0, 1.15, 3.0, 0.05, 1.0, 2.4, 0.75, 900.0,
            ),
            Preset::Race => (
                4.5, 3.0, 2.5, 3.5, 30.0, 60.0, 1.20, 5.0, 0.07, 0.6, 3.5, 0.85, 1500.0,
            ),
        };
        Self {
            label: None,
            target_speed,
            step_frequency,
            a_max,
            a_lat_max,
            look_ahead_m: look_ahead,
            beta_logit: beta,
            lateral_offset_mean: 0.6,
            lateral_offset_std: 0.35,
            lateral_offset_tau_s: 45.0,
            pace_drift_sigma: 0.05,
            pace_drift_tau_s: 60.0,
            critical_speed_ratio: critical,
            fatigue_tau_s: tau,
            pace_strategy: PaceStrategy::Even,
            split_amplitude: 0.06,
            k_down,
            lean_max_deg: lean,
            bounce_amplitude_m: bounce,
            head_look_ahead_s: head,
            turn_omega_max: turn,
            harmonic_2_ratio: CALIBRATED_HARMONIC_2_RATIO,
            harmonic_3_ratio: CALIBRATED_HARMONIC_3_RATIO,
            sensors: SensorNoiseParams::default(),
        }
    }

    /// Applies a label.
    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// Applies a pace strategy.
    pub fn with_pace_strategy(mut self, strategy: PaceStrategy) -> Self {
        self.pace_strategy = strategy;
        self
    }

    /// Sets the target speed.
    pub fn with_target_speed(mut self, speed: f64) -> Self {
        self.target_speed = speed.max(0.5);
        self
    }

    /// Self-consistency checks, applied before a run starts.
    pub fn validate(&self) -> crate::error::Result<()> {
        let checks = [
            (self.target_speed > 0.5, "target speed must exceed 0.5 m/s"),
            (
                self.step_frequency > 0.5,
                "step frequency must exceed 0.5 Hz",
            ),
            (self.a_max > 0.0, "maximum acceleration must be positive"),
            (
                self.a_lat_max > 0.0,
                "lateral acceleration limit must be positive",
            ),
            (
                self.look_ahead_m >= 0.0,
                "look-ahead distance cannot be negative",
            ),
            (self.beta_logit > 0.0, "Logit temperature must be positive"),
            (
                self.k_down >= 1.0,
                "downhill cap coefficient must be at least one",
            ),
            (
                (0.0..1.0).contains(&self.critical_speed_ratio),
                "critical speed ratio must lie in [0, 1)",
            ),
            (
                self.fatigue_tau_s > 0.0,
                "fatigue time constant must be positive",
            ),
            (
                self.bounce_amplitude_m >= 0.0,
                "bounce amplitude cannot be negative",
            ),
            (
                self.pace_drift_sigma >= 0.0,
                "pace drift cannot be negative",
            ),
            (
                self.pace_drift_tau_s > 0.0,
                "pace drift time constant must be positive",
            ),
            (self.turn_omega_max > 0.0, "turn rate must be positive"),
        ];
        for (ok, message) in checks {
            if !ok {
                return Err(crate::error::CoreError::config(message));
            }
        }
        Ok(())
    }
}
