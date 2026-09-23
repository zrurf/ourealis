//! Evaluation metrics.
//!
//! The report produced here doubles as the calibration objective of the design:
//! parameters are the individual vectors, the loss is built from these numbers.
//! Keeping the metrics in the same crate as the generator is deliberate — a
//! metric implementation that drifts from the generator silently makes tuning
//! meaningless.

pub mod calibrate;
pub mod metrics;
pub mod spectrum;

pub use calibrate::{
    CalibrationOptions, CalibrationOutcome, Gait, Knob, Observation, Target, gait_axis,
    gait_observables, gait_of, run_observables,
};
pub use metrics::{
    DistributionStats, GnssErrorStats, LinearFit, autocorrelation, baro_bounce_amplitude_m,
    choice_frequencies, correlation_time_lag, gnss_error_stats, ks_p_value, ks_statistic,
    lap_time_cv, lap_times, path_ratio, position_residuals, speed_residuals, speed_samples,
    speed_stats, turn_rate_stats,
};

/// Pooled moving speed samples of several runs, for distribution comparisons.
///
/// This is the input side of the KS metric: a set of simulated speeds can be
/// compared against a reference distribution, or one population against another.
pub fn pooled_speed_samples(outputs: &[crate::sim::SimulationOutput]) -> Vec<f64> {
    let mut out = Vec::new();
    for output in outputs {
        out.extend(speed_samples(&output.trajectory));
    }
    out
}
pub use spectrum::{
    PeakSummary, SpectrumSummary, accel_vertical_spectrum, baro_altitude_spectrum,
    fundamental_consistency, predicted_fundamental, summarise,
};

use serde::{Deserialize, Serialize};

use crate::motion::Trajectory;
use crate::sensor::{Sensors, TruthState};

/// Length of the window used to remove the slow trend before measuring noise
/// colour, seconds.
///
/// Long enough that a pace drift of tens of seconds survives it, short enough to
/// remove the fatigue trend. It is a duration rather than a sample count because
/// the metric is *colour*: counted in samples it would change meaning with the
/// configured inertial rate, and a five-second window at 200 Hz is 2.5 s of
/// detrending, which is close enough to the correlation time being measured to
/// make any residual look white.
pub const TREND_WINDOW_S: f64 = 5.0;

/// Longest autocorrelation lag reported, seconds.
pub const ACF_MAX_LAG_S: f64 = 1.0;

/// Complete metric report of one run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MetricsReport {
    /// Path ratio against the straight-line distance.
    pub path_ratio: f64,
    /// Path length, metres.
    pub length_m: f64,
    /// Duration, seconds.
    pub duration_s: f64,
    /// Speed distribution.
    pub speed: DistributionStats,
    /// Turn-rate distribution from the effective curvature.
    pub turn_rate: DistributionStats,
    /// Autocorrelation of the speed residual.
    pub speed_acf: Vec<f64>,
    /// Autocorrelation of the lateral position residual.
    pub position_acf: Vec<f64>,
    /// Time between two consecutive autocorrelation lags, seconds.
    pub acf_lag_s: f64,
    /// GNSS error statistics, when GNSS was simulated.
    pub gnss: Option<GnssErrorStats>,
    /// Vertical bounce amplitude at the step frequency, metres.
    ///
    /// Taken from the spectrum rather than from the time series: the altitude
    /// also follows the terrain, and on a hilly map that trend dwarfs the step
    /// ripple, so a time-domain peak-to-peak measure would report the hill.
    pub baro_bounce_m: f64,
    /// Accelerometer spectrum summary.
    pub accel_spectrum: Option<SpectrumSummary>,
    /// Barometer spectrum summary.
    pub baro_spectrum: Option<SpectrumSummary>,
    /// Ratio of the measured accelerometer fundamental to the predicted one.
    pub bounce_consistency: Option<f64>,
    /// Mean absolute curvature, a coarse turning-load indicator.
    pub mean_abs_curvature: f64,
    /// Lap times of a loop session, seconds.
    pub lap_times: Vec<f64>,
    /// Coefficient of variation of the lap times, when there is more than one.
    pub lap_time_cv: Option<f64>,
}

impl MetricsReport {
    /// Computes the report of a finished run.
    #[allow(clippy::too_many_arguments)]
    pub fn compute(
        trajectory: &Trajectory,
        sensors: Option<&Sensors>,
        truth: Option<&[TruthState]>,
        imu_rate_hz: f64,
        baro_rate_hz: f64,
    ) -> Self {
        let speed = speed_stats(trajectory);
        let turn_rate = turn_rate_stats(trajectory);
        // The detrending window has to be far longer than the correlation time
        // being measured, otherwise the moving-average kernel itself becomes the
        // dominant structure and every residual looks white by construction. Both
        // it and the lag range are durations converted at the rate that produced
        // the samples, so the metric means the same thing at any rate.
        let rate = imu_rate_hz.max(1e-3);
        let trend_window = (TREND_WINDOW_S * rate).round().max(3.0) as usize;
        let max_lag = (ACF_MAX_LAG_S * rate).round().max(1.0) as usize;
        let speed_acf = autocorrelation(&speed_residuals(trajectory, trend_window), max_lag);
        let position_acf = autocorrelation(&position_residuals(trajectory, trend_window), max_lag);

        let gnss = match (sensors, truth) {
            (Some(sensors), Some(truth)) => gnss_error_stats(&sensors.gnss, sensors, truth),
            _ => None,
        };
        let accel_spectrum = sensors.map(|sensors| {
            let spectrum = accel_vertical_spectrum(&sensors.imu.accel, imu_rate_hz);
            summarise(&spectrum, Some(trajectory.step_frequency))
        });
        let baro_spectrum = sensors.map(|sensors| {
            let spectrum = baro_altitude_spectrum(&sensors.baro, baro_rate_hz);
            summarise(&spectrum, Some(trajectory.step_frequency))
        });
        let baro_bounce_m = baro_spectrum
            .as_ref()
            .and_then(|summary| summary.step_peak.map(|peak| peak.magnitude))
            .unwrap_or(0.0);
        // The prediction is scaled to the pace actually run: the bounce amplitude
        // grows with speed, so comparing against the nominal amplitude would
        // report a pace difference as a model error.
        let bounce_consistency = accel_spectrum.as_ref().and_then(|summary| {
            summary.step_peak.map(|peak| {
                fundamental_consistency(
                    peak.magnitude,
                    predicted_fundamental(
                        trajectory.bounce.amplitude_m * trajectory.bounce_amplitude_ratio(),
                        trajectory.step_frequency,
                    ),
                )
            })
        });

        let lap_times = lap_times(trajectory);
        let lap_time_cv = lap_time_cv(trajectory);
        // Moving samples only, in both terms: a standing start and an end hold carry
        // no curvature, so dividing by them would make a healthy route read a lower
        // curvature the longer the runner stood still.
        let moving_count = trajectory
            .samples
            .iter()
            .filter(|sample| sample.is_moving())
            .count();
        let mean_abs_curvature = if moving_count == 0 {
            0.0
        } else {
            trajectory
                .samples
                .iter()
                .filter(|sample| sample.is_moving())
                .map(|sample| sample.kappa_eff.abs())
                .sum::<f64>()
                / moving_count as f64
        };

        Self {
            path_ratio: path_ratio(trajectory),
            length_m: trajectory.path.total_length(),
            duration_s: trajectory.duration_s(),
            speed,
            turn_rate,
            speed_acf,
            acf_lag_s: 1.0 / rate,
            position_acf,
            gnss,
            baro_bounce_m,
            accel_spectrum,
            baro_spectrum,
            bounce_consistency,
            mean_abs_curvature,
            lap_times,
            lap_time_cv,
        }
    }

    /// True when the run passes the coarse plausibility checks of the design.
    ///
    /// The thresholds are wide on purpose: they catch a broken generator, not a
    /// marginal parameter choice, which is what the calibration workflow is for.
    pub fn passes_plausibility(&self) -> bool {
        let ratio_ok = (1.0..=3.0).contains(&self.path_ratio);
        let speed_ok = self.speed.mean > 0.5 && self.speed.p95 < 8.0;
        // The autocorrelation is measured against a five-second trend, so it
        // stays near one for the first few samples by construction; the check is
        // made at a tenth of a second, where a frozen or white signal would
        // already be distinguishable.
        let tenth = (0.1f64 / self.acf_lag_s.max(1e-9)).round() as usize;
        let noise_ok = self
            .speed_acf
            .get(tenth)
            .map(|value| value.abs() < 0.999)
            .unwrap_or(false);
        ratio_ok && speed_ok && noise_ok
    }

    /// Serialises the report as pretty JSON.
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_else(|_| "{}".to_string())
    }
}
