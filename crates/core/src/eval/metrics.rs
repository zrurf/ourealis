//! Evaluation metrics.
//!
//! "Does this look like a real run?" has to be answerable numerically, otherwise
//! parameter tuning is guesswork and the calibration interface of the design has
//! no objective to optimise. The four primary metrics check four different
//! failures: detour ratio (too straight or too winding), speed distribution,
//! turn-rate distribution and noise colour.

use glam::DVec2;
use serde::{Deserialize, Serialize};

use crate::motion::Trajectory;
use crate::sensor::{GnssSample, Sensors};

/// Summary of a distribution.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct DistributionStats {
    /// Sample count.
    pub count: usize,
    /// Mean.
    pub mean: f64,
    /// Standard deviation.
    pub std_dev: f64,
    /// Minimum.
    pub min: f64,
    /// Fifth percentile.
    pub p05: f64,
    /// Median.
    pub p50: f64,
    /// Ninety-fifth percentile.
    pub p95: f64,
    /// Maximum.
    pub max: f64,
}

impl DistributionStats {
    /// Computes the summary of a sample.
    pub fn of(values: &[f64]) -> Self {
        if values.is_empty() {
            return Self {
                count: 0,
                mean: 0.0,
                std_dev: 0.0,
                min: 0.0,
                p05: 0.0,
                p50: 0.0,
                p95: 0.0,
                max: 0.0,
            };
        }
        let mut sorted = values.to_vec();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let mean = sorted.iter().sum::<f64>() / sorted.len() as f64;
        let variance = sorted
            .iter()
            .map(|value| (value - mean).powi(2))
            .sum::<f64>()
            / sorted.len() as f64;
        Self {
            count: sorted.len(),
            mean,
            std_dev: variance.sqrt(),
            min: sorted[0],
            p05: crate::math::sampling::percentile(&sorted, 0.05),
            p50: crate::math::sampling::percentile(&sorted, 0.50),
            p95: crate::math::sampling::percentile(&sorted, 0.95),
            max: sorted[sorted.len() - 1],
        }
    }

    /// Coefficient of variation, used for lap-time consistency.
    pub fn coefficient_of_variation(&self) -> f64 {
        if self.mean.abs() < 1e-9 {
            0.0
        } else {
            self.std_dev / self.mean.abs()
        }
    }
}

/// Two-sample Kolmogorov–Smirnov statistic.
///
/// `D = sup |F1 - F2|`: the largest gap between the empirical distributions, which
/// is what the design compares against reference speed distributions.
pub fn ks_statistic(sample_a: &[f64], sample_b: &[f64]) -> f64 {
    if sample_a.is_empty() || sample_b.is_empty() {
        return 1.0;
    }
    let mut a = sample_a.to_vec();
    let mut b = sample_b.to_vec();
    a.sort_by(|x, y| x.partial_cmp(y).unwrap_or(std::cmp::Ordering::Equal));
    b.sort_by(|x, y| x.partial_cmp(y).unwrap_or(std::cmp::Ordering::Equal));

    let (mut i, mut j) = (0usize, 0usize);
    let mut max_gap: f64 = 0.0;
    while i < a.len() && j < b.len() {
        let value = a[i].min(b[j]);
        while i < a.len() && a[i] <= value {
            i += 1;
        }
        while j < b.len() && b[j] <= value {
            j += 1;
        }
        let fa = i as f64 / a.len() as f64;
        let fb = j as f64 / b.len() as f64;
        max_gap = max_gap.max((fa - fb).abs());
    }
    max_gap
}

/// Approximate p-value of a two-sample KS statistic.
///
/// Uses the asymptotic Kolmogorov distribution, which is accurate enough for the
/// sample sizes involved and avoids a special-function dependency. The two series
/// forms of that distribution are both needed: the alternating sum converges
/// quickly only for large `lambda`, and at small `lambda` it converges so slowly
/// that a fixed number of terms cancels to zero — which reports maximal
/// significance, `p = 0`, for two identical samples.
pub fn ks_p_value(d: f64, n_a: usize, n_b: usize) -> f64 {
    if n_a == 0 || n_b == 0 {
        return 1.0;
    }
    let effective = ((n_a as f64) * (n_b as f64) / (n_a + n_b) as f64).sqrt();
    let lambda = (effective + 0.12 + 0.11 / effective) * d;
    if lambda <= 0.0 || !lambda.is_finite() {
        // No deviation at all: the samples agree everywhere.
        return 1.0;
    }
    if lambda < 1.0 {
        // Theta-function form, `1 - sqrt(2 pi)/lambda sum exp(-(2k-1)^2 pi^2 / 8 lambda^2)`.
        let mut sum = 0.0;
        for k in 1..=100 {
            let odd = (2 * k - 1) as f64;
            let term = (-odd * odd * std::f64::consts::PI * std::f64::consts::PI
                / (8.0 * lambda * lambda))
                .exp();
            sum += term;
            if term < 1e-18 {
                break;
            }
        }
        return (1.0 - (std::f64::consts::TAU).sqrt() / lambda * sum).clamp(0.0, 1.0);
    }
    let mut sum = 0.0;
    for k in 1..=100 {
        let term = (-2.0 * (k as f64).powi(2) * lambda * lambda).exp();
        if term < 1e-18 {
            break;
        }
        sum += if k % 2 == 1 { term } else { -term };
    }
    (2.0 * sum).clamp(0.0, 1.0)
}

/// Autocorrelation of a series at lags zero to `max_lag`.
///
/// The shape of the decay is what distinguishes white sensor noise (immediate
/// drop to zero) from the Ornstein–Uhlenbeck motion noise (exponential decay),
/// making it a direct check of the three-layer noise design.
pub fn autocorrelation(values: &[f64], max_lag: usize) -> Vec<f64> {
    if values.len() < 3 {
        return vec![1.0];
    }
    let mean = values.iter().sum::<f64>() / values.len() as f64;
    let variance: f64 = values.iter().map(|value| (value - mean).powi(2)).sum();
    if variance < 1e-12 {
        return vec![1.0];
    }
    let mut out = Vec::with_capacity(max_lag + 1);
    for lag in 0..=max_lag.min(values.len() - 1) {
        let mut sum = 0.0;
        for index in 0..values.len() - lag {
            sum += (values[index] - mean) * (values[index + lag] - mean);
        }
        out.push(sum / variance);
    }
    out
}

/// Lag at which the autocorrelation first falls below `1/e`.
pub fn correlation_time_lag(acf: &[f64]) -> f64 {
    let threshold = 1.0 / std::f64::consts::E;
    for (lag, value) in acf.iter().enumerate() {
        if *value < threshold {
            return lag as f64;
        }
    }
    acf.len() as f64
}

/// Turn-rate statistics from a trajectory.
///
/// Uses the *effective* curvature, so the metric and the gyroscope agree; the
/// two would diverge on bends if the centre-line curvature were used here.
pub fn turn_rate_stats(trajectory: &Trajectory) -> DistributionStats {
    let rates: Vec<f64> = trajectory
        .samples
        .iter()
        .filter(|sample| sample.is_moving())
        .map(|sample| sample.speed * sample.kappa_eff)
        .collect();
    DistributionStats::of(&rates)
}

/// Speeds below which a sample belongs to a start or stop transition rather
/// than to running.
pub const RUNNING_SPEED_THRESHOLD: f64 = 0.5;

/// Running speed samples of a run, for distribution comparisons.
///
/// The acceleration and braking ramps at the ends of a run are transitions, not
/// running: including them would shift every percentile downward and make a
/// comparison against recorded running data meaningless.
pub fn speed_samples(trajectory: &Trajectory) -> Vec<f64> {
    trajectory
        .samples
        .iter()
        .filter(|sample| sample.is_moving() && sample.speed >= RUNNING_SPEED_THRESHOLD)
        .map(|sample| sample.speed)
        .collect()
}

/// Speed statistics of a trajectory.
pub fn speed_stats(trajectory: &Trajectory) -> DistributionStats {
    DistributionStats::of(&speed_samples(trajectory))
}

/// Path ratio `L_path / d_euclid`.
pub fn path_ratio(trajectory: &Trajectory) -> f64 {
    let start = trajectory.path.start();
    let end = trajectory.path.end();
    let direct = (end - start).length();
    if direct < 1e-6 {
        // A closed loop has no straight-line equivalent; report the ratio
        // against the path's own extent instead of dividing by zero.
        let extent = trajectory
            .path
            .points()
            .iter()
            .map(|point| (*point - start).length())
            .fold(0.0f64, f64::max);
        if extent < 1e-6 {
            1.0
        } else {
            trajectory.path.total_length() / (2.0 * extent)
        }
    } else {
        trajectory.path.total_length() / direct
    }
}

/// Duration of each lap of a loop session, seconds.
///
/// Laps are delimited by the trajectory's lap length, so a single-lap run yields
/// one entry. The seam is where the last sample of one lap meets the first of the
/// next, which is exactly what the consistency metric examines.
pub fn lap_times(trajectory: &Trajectory) -> Vec<f64> {
    if trajectory.laps <= 1 || trajectory.lap_length_m <= 1e-6 {
        return vec![trajectory.duration_s()];
    }
    // Lap boundaries are the *first sample inside* each lap. Measuring from the
    // first and last sample of a lap instead would make the result depend on how
    // densely that particular stretch happens to be sampled.
    let mut starts = vec![f64::NAN; trajectory.laps];
    for sample in &trajectory.samples {
        let index =
            ((sample.arc_s / trajectory.lap_length_m).floor() as usize).min(trajectory.laps - 1);
        if starts[index].is_nan() {
            starts[index] = sample.time_s;
        }
    }
    let mut laps = Vec::with_capacity(trajectory.laps);
    for index in 0..trajectory.laps {
        if starts[index].is_nan() {
            continue;
        }
        let end = starts
            .iter()
            .skip(index + 1)
            .find(|value| !value.is_nan())
            .copied()
            .unwrap_or(trajectory.duration_s());
        laps.push(end - starts[index]);
    }
    laps
}

/// Coefficient of variation of the lap times, `std / mean`.
///
/// A loop session with no noise at all produces zero variation, which is itself a
/// defect: real runners vary by a few percent. Values far above that indicate the
/// motion noise is being integrated into the timing rather than modulating it.
pub fn lap_time_cv(trajectory: &Trajectory) -> Option<f64> {
    if trajectory.laps <= 1 {
        return None;
    }
    // The first lap carries the standing start and its acceleration ramp, which
    // is not part of steady-state lap consistency; including it would make the
    // metric report the start-up transient rather than the pacing stability.
    let times = lap_times(trajectory);
    let times = if trajectory.starts_standing && times.len() > 1 {
        times[1..].to_vec()
    } else {
        times
    };
    if times.len() < 2 {
        return None;
    }
    Some(DistributionStats::of(&times).coefficient_of_variation())
}

/// Speed residual series: the deviation of the speed from its local mean, which
/// is the motion-noise component the ACF metric examines.
pub fn speed_residuals(trajectory: &Trajectory, window: usize) -> Vec<f64> {
    let speeds: Vec<f64> = trajectory
        .samples
        .iter()
        .map(|sample| sample.speed)
        .collect();
    let trend = crate::math::sampling::moving_average(&speeds, window.max(3));
    speeds
        .iter()
        .zip(trend.iter())
        .map(|(value, mean)| value - mean)
        .collect()
}

/// Position residual magnitude relative to the local mean path, in metres.
pub fn position_residuals(trajectory: &Trajectory, window: usize) -> Vec<f64> {
    let offsets: Vec<f64> = trajectory
        .samples
        .iter()
        .map(|sample| sample.offset_m)
        .collect();
    let trend = crate::math::sampling::moving_average(&offsets, window.max(3));
    offsets
        .iter()
        .zip(trend.iter())
        .map(|(value, mean)| value - mean)
        .collect()
}

/// GNSS error statistics against the truth positions.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct GnssErrorStats {
    /// Horizontal error distribution, metres.
    pub horizontal: DistributionStats,
    /// Vertical error distribution, metres.
    pub vertical: DistributionStats,
    /// Speed error distribution, m/s.
    pub speed: DistributionStats,
    /// Share of epochs that produced a fix.
    pub availability: f64,
}

/// Computes GNSS errors by matching each fix to the truth at its time.
pub fn gnss_error_stats(
    gnss: &[GnssSample],
    sensors: &Sensors,
    truth: &[crate::sensor::TruthState],
) -> Option<GnssErrorStats> {
    if gnss.is_empty() || truth.is_empty() {
        return None;
    }
    let mut horizontal = Vec::new();
    let mut vertical = Vec::new();
    let mut speed = Vec::new();
    for fix in gnss {
        let state = crate::sensor::state_at(truth, fix.time_s)?;
        horizontal.push((DVec2::new(fix.x, fix.y) - state.position).length());
        vertical.push(fix.altitude_m - state.z);
        // A receiver derives speed from Doppler, and its position-differenced
        // analogue is dominated by position noise when standing still — an effect
        // that says nothing about the device being modelled. Stationary epochs are
        // therefore excluded, as they are when comparing against real recordings.
        if state.speed >= RUNNING_SPEED_THRESHOLD {
            speed.push(fix.speed_mps - state.speed);
        }
    }
    Some(GnssErrorStats {
        horizontal: DistributionStats::of(&horizontal),
        vertical: DistributionStats::of(&vertical),
        speed: DistributionStats::of(&speed),
        availability: sensors.gnss_availability(),
    })
}

/// Linear fit of `y` against `x`, used for the cadence–speed relation.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct LinearFit {
    /// Slope.
    pub slope: f64,
    /// Intercept.
    pub intercept: f64,
    /// Coefficient of determination.
    pub r_squared: f64,
}

impl LinearFit {
    /// Least-squares fit of `y = slope * x + intercept`.
    pub fn of(x: &[f64], y: &[f64]) -> Option<Self> {
        let n = x.len().min(y.len());
        if n < 3 {
            return None;
        }
        let mean_x = x[..n].iter().sum::<f64>() / n as f64;
        let mean_y = y[..n].iter().sum::<f64>() / n as f64;
        let mut covariance = 0.0;
        let mut variance = 0.0;
        for index in 0..n {
            covariance += (x[index] - mean_x) * (y[index] - mean_y);
            variance += (x[index] - mean_x).powi(2);
        }
        if variance.abs() < 1e-12 {
            return None;
        }
        let slope = covariance / variance;
        let intercept = mean_y - slope * mean_x;
        let mut residual = 0.0;
        let mut total = 0.0;
        for index in 0..n {
            residual += (y[index] - (slope * x[index] + intercept)).powi(2);
            total += (y[index] - mean_y).powi(2);
        }
        let r_squared = if total.abs() < 1e-12 {
            1.0
        } else {
            (1.0 - residual / total).clamp(0.0, 1.0)
        };
        Some(Self {
            slope,
            intercept,
            r_squared,
        })
    }
}

/// Path-choice frequencies of a batch: how often each candidate was taken.
pub fn choice_frequencies(choices: &[usize], candidates: usize) -> Vec<f64> {
    if choices.is_empty() || candidates == 0 {
        return vec![0.0; candidates];
    }
    let mut counts = vec![0usize; candidates];
    for index in choices {
        if *index < candidates {
            counts[*index] += 1;
        }
    }
    counts
        .into_iter()
        .map(|count| count as f64 / choices.len() as f64)
        .collect()
}

/// Vertical bounce signature measured from the barometer, in metres.
pub fn baro_bounce_amplitude_m(samples: &[crate::sensor::BaroSample]) -> f64 {
    if samples.len() < 8 {
        return 0.0;
    }
    let altitudes: Vec<f64> = samples.iter().map(|sample| sample.altitude_m).collect();
    let trend = crate::math::sampling::moving_average(&altitudes, 5);
    let residuals: Vec<f64> = altitudes
        .iter()
        .zip(trend.iter())
        .map(|(value, mean)| value - mean)
        .collect();
    let max = residuals.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let min = residuals.iter().copied().fold(f64::INFINITY, f64::min);
    (max - min) * 0.5
}
