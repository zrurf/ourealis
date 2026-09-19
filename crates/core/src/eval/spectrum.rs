//! Spectral metrics.
//!
//! Two checks need a spectrum rather than a time-domain statistic:
//!
//! * the accelerometer must show a peak at the step frequency with a harmonic
//!   stack, and the fundamental's amplitude must match `A_b (2 pi f)^2` derived
//!   from the bounce model. If the two were generated independently this test
//!   fails, which is exactly why the design locks their phase;
//! * the barometer must show a ripple at the step frequency, because a
//!   consumer-grade device resolves the bounce.

use serde::{Deserialize, Serialize};

use crate::math::fft::{Spectrum, SpectrumPeak};
use crate::sensor::{BaroSample, ImuAccelSample};

/// Spectral summary of a sensor channel.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SpectrumSummary {
    /// Frequency resolution, Hz.
    pub resolution_hz: f64,
    /// Strongest peak in the band searched.
    pub dominant: Option<PeakSummary>,
    /// Peak closest to the expected step frequency, when one was requested.
    pub step_peak: Option<PeakSummary>,
    /// Ratios of the second and third harmonic to the fundamental.
    pub harmonic_ratios: Vec<f64>,
}

/// One spectral peak.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PeakSummary {
    /// Frequency, Hz.
    pub frequency_hz: f64,
    /// Amplitude.
    pub magnitude: f64,
}

impl From<SpectrumPeak> for PeakSummary {
    fn from(peak: SpectrumPeak) -> Self {
        Self {
            frequency_hz: peak.frequency_hz,
            magnitude: peak.magnitude,
        }
    }
}

/// Spectrum of the vertical accelerometer channel.
pub fn accel_vertical_spectrum(samples: &[ImuAccelSample], sample_rate_hz: f64) -> Spectrum {
    let signal: Vec<f64> = samples.iter().map(|sample| sample.z).collect();
    Spectrum::of(&signal, sample_rate_hz)
}

/// Spectrum of the barometric altitude.
pub fn baro_altitude_spectrum(samples: &[BaroSample], sample_rate_hz: f64) -> Spectrum {
    let signal: Vec<f64> = samples.iter().map(|sample| sample.altitude_m).collect();
    Spectrum::of(&signal, sample_rate_hz)
}

/// Summarises a spectrum around an expected step frequency.
pub fn summarise(spectrum: &Spectrum, expected_step_hz: Option<f64>) -> SpectrumSummary {
    let dominant = spectrum.peaks(1, 0.2).first().copied();
    let step_peak = expected_step_hz
        .and_then(|frequency| spectrum.peak_in_band(frequency * 0.8, frequency * 1.2));
    let harmonic_ratios = match expected_step_hz {
        Some(frequency) if frequency > 0.0 => {
            let fundamental = spectrum.magnitude_at(frequency).max(1e-9);
            vec![
                spectrum.magnitude_at(2.0 * frequency) / fundamental,
                spectrum.magnitude_at(3.0 * frequency) / fundamental,
            ]
        }
        _ => Vec::new(),
    };
    SpectrumSummary {
        resolution_hz: spectrum.resolution_hz,
        dominant: dominant.map(PeakSummary::from),
        step_peak: step_peak.map(PeakSummary::from),
        harmonic_ratios,
    }
}

/// Compares the measured fundamental with the value the bounce model predicts.
///
/// A ratio near one means the accelerometer and the vertical truth agree, which
/// is the design's phase-locking requirement expressed as a number.
pub fn fundamental_consistency(measured: f64, predicted: f64) -> f64 {
    if predicted.abs() < 1e-9 {
        return 0.0;
    }
    measured / predicted
}

/// Expected fundamental amplitude of the vertical acceleration.
pub fn predicted_fundamental(bounce_amplitude_m: f64, step_frequency_hz: f64) -> f64 {
    let omega = std::f64::consts::TAU * step_frequency_hz;
    bounce_amplitude_m * omega * omega
}
