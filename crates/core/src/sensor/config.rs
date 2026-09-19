//! Sensor configuration and the mount choice.

use serde::{Deserialize, Serialize};

/// Where the inertial unit is carried.
///
/// The mount selects which heading the gyroscope reports: a unit on the torso
/// sees the body heading, a head-mounted unit sees the head, which leads into a
/// turn. The choice is declared in the output metadata so downstream code knows
/// which pattern to expect.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[repr(u8)]
pub enum DeviceMount {
    /// Torso or hand-held device: body heading.
    #[default]
    Body = 0,
    /// Head-mounted device: head heading, which leads the body.
    Head = 1,
}

impl DeviceMount {
    /// Parses an on-disk identifier.
    pub const fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(DeviceMount::Body),
            1 => Some(DeviceMount::Head),
            _ => None,
        }
    }

    /// Human-readable name used in the output manifest.
    pub const fn name(self) -> &'static str {
        match self {
            DeviceMount::Body => "body",
            DeviceMount::Head => "head",
        }
    }
}

/// Sampling and noise configuration of the sensor suite.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SensorConfig {
    /// GNSS output rate, Hz.
    pub gnss_rate_hz: f64,
    /// Inertial output rate, Hz (accelerometer and gyroscope together).
    pub imu_rate_hz: f64,
    /// Magnetometer output rate, Hz.
    pub mag_rate_hz: f64,
    /// Barometer output rate, Hz.
    pub baro_rate_hz: f64,
    /// Whether multipath events are applied to GNSS positions.
    pub multipath_enabled: bool,
    /// Whether transient magnetic disturbances are applied.
    pub magnetic_disturbance_enabled: bool,
    /// Whether high-frequency position jitter enters the reported truth.
    pub jitter_enabled: bool,
    /// Standard deviation of the position jitter, metres.
    pub jitter_sigma_m: f64,
    /// Weather-driven barometric drift, steady-state standard deviation in Pa.
    pub baro_weather_sigma_pa: f64,
    /// Weather drift time constant, seconds.
    pub baro_weather_tau_s: f64,
    /// Magnetic disturbance amplitude, microtesla.
    pub mag_disturbance_ut: f64,
    /// Device mount.
    pub mount: DeviceMount,
    /// Pressure at the reference altitude, pascals.
    pub reference_pressure_pa: f64,
    /// Whether region events ignore the mode their region declares.
    ///
    /// The map decides per region whether an event is drawn or hashed, and the
    /// format's default is to draw. Calibration, regression tests and any
    /// comparison of two runs of the same parameters need the hashed behaviour
    /// regardless of what the map says, or the difference they measure includes
    /// event randomness that is not part of the question.
    pub force_deterministic_events: bool,
}

impl Default for SensorConfig {
    fn default() -> Self {
        Self {
            gnss_rate_hz: 1.0,
            imu_rate_hz: 100.0,
            mag_rate_hz: 50.0,
            baro_rate_hz: 25.0,
            multipath_enabled: true,
            magnetic_disturbance_enabled: true,
            jitter_enabled: true,
            jitter_sigma_m: 0.03,
            baro_weather_sigma_pa: 40.0,
            baro_weather_tau_s: 1800.0,
            mag_disturbance_ut: 20.0,
            mount: DeviceMount::Body,
            reference_pressure_pa: crate::math::SEA_LEVEL_PRESSURE_PA,
            force_deterministic_events: false,
        }
    }
}

impl SensorConfig {
    /// A configuration with noise disabled, for tests that need exact values.
    pub fn clean() -> Self {
        Self {
            multipath_enabled: false,
            magnetic_disturbance_enabled: false,
            jitter_enabled: false,
            baro_weather_sigma_pa: 0.0,
            ..Default::default()
        }
    }

    /// A configuration whose region events are reproducible whatever the map says.
    ///
    /// Used by calibration and regression runs; see
    /// [`SensorConfig::force_deterministic_events`].
    pub fn calibrated() -> Self {
        Self {
            force_deterministic_events: true,
            ..Self::default()
        }
    }

    /// Validates the rates and amplitudes.
    pub fn validate(&self) -> crate::error::Result<()> {
        let checks = [
            (self.gnss_rate_hz > 0.0, "GNSS rate must be positive"),
            (self.imu_rate_hz > 0.0, "IMU rate must be positive"),
            (self.mag_rate_hz > 0.0, "magnetometer rate must be positive"),
            (self.baro_rate_hz > 0.0, "barometer rate must be positive"),
            (
                self.jitter_sigma_m >= 0.0,
                "position jitter cannot be negative",
            ),
            (
                self.reference_pressure_pa > 1000.0,
                "reference pressure looks implausible",
            ),
        ];
        for (ok, message) in checks {
            if !ok {
                return Err(crate::error::CoreError::SensorConfig(message.to_string()));
            }
        }
        Ok(())
    }

    /// Sample interval of the inertial sensors, seconds.
    pub fn imu_dt(&self) -> f64 {
        1.0 / self.imu_rate_hz.max(1e-3)
    }
}
