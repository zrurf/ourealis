//! Barometer: pressure derived from the truth altitude.
//!
//! The altitude fed to the model includes the vertical bounce, and that is
//! deliberate: a consumer-grade barometer resolves 8–25 cm, so the 3–8 cm step
//! ripple is visible in real recordings. Data without it is identifiable in the
//! frequency domain at a glance, and any downstream step-detection algorithm
//! tested against it would be tested against the wrong signal.

use serde::{Deserialize, Serialize};

use crate::math::ATMOSPHERE_SCALE_HEIGHT_M;
use crate::noise::{OuParams, OuProcess};
use crate::rng::{Rng, Stream};

/// One barometer sample.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct BaroSample {
    /// Time since the start of the recording, seconds.
    pub time_s: f64,
    /// Pressure, pascals.
    pub pressure_pa: f64,
    /// Altitude implied by the pressure, metres.
    pub altitude_m: f64,
}

/// Barometer error state.
#[derive(Debug, Clone)]
pub struct BarometerModel {
    weather: OuProcess,
    white_sigma_pa: f64,
    reference_pressure_pa: f64,
}

impl BarometerModel {
    /// Creates the model.
    pub fn new(
        reference_pressure_pa: f64,
        white_sigma_pa: f64,
        weather_sigma_pa: f64,
        weather_tau_s: f64,
    ) -> Self {
        Self {
            weather: OuProcess::new(OuParams::centred(weather_sigma_pa, weather_tau_s)),
            white_sigma_pa,
            reference_pressure_pa,
        }
    }

    /// Builds the model from the run configuration.
    pub fn for_config(
        params: &crate::person::SensorNoiseParams,
        config: &super::SensorConfig,
    ) -> Self {
        Self::new(
            config.reference_pressure_pa,
            params.baro_white_sigma_pa,
            config.baro_weather_sigma_pa,
            config.baro_weather_tau_s,
        )
    }

    /// Pressure at the standard atmosphere for an altitude.
    pub fn pressure_for_altitude(&self, altitude_m: f64) -> f64 {
        self.reference_pressure_pa * (-altitude_m / ATMOSPHERE_SCALE_HEIGHT_M).exp()
    }

    /// Measures the pressure at a truth altitude.
    pub fn measure(&mut self, altitude_m: f64, dt: f64, rng: &mut Rng) -> (f64, f64) {
        let weather = self.weather.step(dt, rng);
        let pressure =
            self.pressure_for_altitude(altitude_m) + weather + rng.gaussian() * self.white_sigma_pa;
        let implied =
            -((pressure / self.reference_pressure_pa).max(1e-6)).ln() * ATMOSPHERE_SCALE_HEIGHT_M;
        (pressure, implied)
    }

    /// Reference pressure of the model.
    pub fn reference_pressure_pa(&self) -> f64 {
        self.reference_pressure_pa
    }
}

/// RNG stream for barometer noise.
pub fn stream(seed: u64, individual: u32) -> Rng {
    Rng::stream(seed, Stream::Barometer, individual, 0)
}
