//! Parameter calibration against measured reference values.
//!
//! The design's calibration loop is `theta* = argmin d(T_sim(theta), T_real)` with a
//! gradient-free search, and it names two things: the evaluation metrics are the
//! loss, and the individual parameter vector is the variable. This module is that
//! loop with the loss expressed as **moment matching** rather than a trajectory
//! distance, because the reference here is a set of measured statistics (a
//! cadence, two harmonic ratios) rather than a reference trajectory:
//!
//! * [`gait_observables`] measures a signal exactly the way the reference
//!   recordings are measured, so the two sides are comparable by construction;
//! * [`Target`] carries the reference values with weights;
//! * [`coordinate_search`] minimises the weighted loss over a set of [`Knob`]s.
//!
//! ## Why the estimator lives here
//!
//! A calibration is only meaningful if both sides are measured the same way, so
//! the estimator is *shared*: `tests/real_data.rs` and the calibration use the same
//! function. An earlier round compared a simulator measured on one signal against
//! recordings measured on another, and the difference it reported was mostly the
//! difference between the two measurements.
//!
//! ## Reproducibility
//!
//! Every evaluation is expected to be deterministic — the caller configures the run
//! with [`crate::sim::SimulationConfig::deterministic`], whose sensor
//! configuration forces reproducible region events. Without that the optimiser
//! chases event randomness as though it were part of its objective.

use std::collections::BTreeMap;

use crate::error::{CoreError, Result};
use crate::math::fft::Spectrum;
use crate::motion::MotionConfig;
use crate::person::PersonParams;
use crate::sim::SimulationOutput;

/// A named scalar measured from a run or tabulated from a reference.
pub type Observation = BTreeMap<String, f64>;

/// Names of the observables this module produces.
pub mod name {
    /// Cadence, Hz.
    pub const CADENCE_HZ: &str = "cadence_hz";
    /// Amplitude of the accelerometer's fundamental, in its own units.
    pub const FUNDAMENTAL: &str = "fundamental";
    /// Ratio of the second harmonic to the fundamental.
    pub const A2_RATIO: &str = "a2_ratio";
    /// Ratio of the third harmonic to the fundamental.
    pub const A3_RATIO: &str = "a3_ratio";
    /// Mean speed over the running segments, m/s.
    pub const SPEED_MEAN: &str = "speed_mean";
}

/// The band a running step can occupy, Hz.
///
/// Bounded below by a slow walk and above by a sprint, so the estimator cannot lock
/// onto a slow drift or a noise peak.
pub const STEP_BAND_HZ: (f64, f64) = (1.4, 3.6);

/// Gait signature of one signal.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Gait {
    /// Frequency of the strongest line in the step band.
    pub step_hz: f64,
    /// Amplitude of that line.
    pub fundamental: f64,
    /// Ratio of the second harmonic to it.
    pub second_ratio: f64,
    /// Ratio of the third harmonic to it.
    pub third_ratio: f64,
}

/// Measures the gait signature of a signal.
///
/// Returns `None` when no line falls inside the step band, which is how a signal
/// without a gait is rejected rather than silently measured.
///
/// The lines are interpolated, not read off the nearest bin. The spectrum is
/// Hann-windowed, and that window's response half a bin away from a line is about
/// 0.85 of its peak, so a step frequency landing between bins would be under-read by
/// up to fifteen percent. Worse for a calibration, the *ratio* of two lines would
/// then depend on where the cadence happens to fall on the bin grid, and the search
/// would be fitting the grid rather than the model.
pub fn gait_of(signal: &[f64], rate_hz: f64) -> Option<Gait> {
    let spectrum = Spectrum::of(signal, rate_hz);
    let peak = spectrum.peak_in_band(STEP_BAND_HZ.0, STEP_BAND_HZ.1)?;
    let (step_hz, fundamental) = interpolated_line(&spectrum, peak.frequency_hz);
    if fundamental <= 1e-12 {
        return None;
    }
    let (_, second) = interpolated_line(&spectrum, 2.0 * step_hz);
    let (_, third) = interpolated_line(&spectrum, 3.0 * step_hz);
    Some(Gait {
        step_hz,
        fundamental,
        second_ratio: second / fundamental,
        third_ratio: third / fundamental,
    })
}

/// Frequency and amplitude of the line nearest a frequency.
///
/// The local maximum among the three bins around the nominal position is taken, so
/// a line slightly off the nominal harmonic is found rather than its skirt, and its
/// amplitude is interpolated parabolically.
fn interpolated_line(spectrum: &Spectrum, frequency_hz: f64) -> (f64, f64) {
    let magnitudes = &spectrum.magnitudes;
    if magnitudes.len() < 3 {
        return (frequency_hz, 0.0);
    }
    let resolution = spectrum.resolution_hz.max(1e-9);
    let centre = ((frequency_hz / resolution).round() as usize).clamp(1, magnitudes.len() - 2);
    // The scan stops one bin short of the end of the spectrum: the interpolation
    // below needs a bin on each side of the peak, and a local maximum on the last
    // bin would index past the end.
    let last = magnitudes.len() - 2;
    let mut peak = centre;
    for index in centre.saturating_sub(1)..=(centre + 1).min(last) {
        if magnitudes[index] > magnitudes[peak] {
            peak = index;
        }
    }
    let peak = peak.min(last).max(1);
    let (left, middle, right) = (magnitudes[peak - 1], magnitudes[peak], magnitudes[peak + 1]);
    let denominator = left - 2.0 * middle + right;
    if denominator.abs() < 1e-15 {
        return (peak as f64 * resolution, middle);
    }
    let offset = 0.5 * (left - right) / denominator;
    let amplitude = middle - 0.25 * (left - right) * offset;
    ((peak as f64 + offset) * resolution, amplitude.max(0.0))
}

/// Index of the axis whose variance is largest.
///
/// The axis carrying the gait has the most variation in it, whether the device was
/// in a pocket, on the torso or on a wrist. The *magnitude* is orientation-free too
/// but is the wrong signal for a harmonic ratio: the magnitude of an oscillation
/// that is large next to its constant component is a rectified sine, whose
/// strongest line sits at twice the cadence.
pub fn gait_axis(samples: &[[f64; 3]]) -> usize {
    let mut best = (0usize, f64::NEG_INFINITY);
    for axis in 0..3 {
        let mean = samples.iter().map(|sample| sample[axis]).sum::<f64>() / samples.len() as f64;
        let variance = samples
            .iter()
            .map(|sample| (sample[axis] - mean).powi(2))
            .sum::<f64>()
            / samples.len().max(1) as f64;
        if variance > best.1 {
            best = (axis, variance);
        }
    }
    best.0
}

/// Gait observables of a signal, in the names [`name`] declares.
pub fn gait_observables(signal: &[f64], rate_hz: f64) -> Observation {
    let mut out = Observation::new();
    if let Some(gait) = gait_of(signal, rate_hz) {
        out.insert(name::CADENCE_HZ.to_string(), gait.step_hz);
        out.insert(name::FUNDAMENTAL.to_string(), gait.fundamental);
        out.insert(name::A2_RATIO.to_string(), gait.second_ratio);
        out.insert(name::A3_RATIO.to_string(), gait.third_ratio);
    }
    out
}

/// Gait observables of a simulated run's accelerometer, plus its mean speed.
///
/// The accelerometer is the channel the design defines the signature on, and the
/// gait axis is chosen the same way the reference recordings choose it.
pub fn run_observables(output: &SimulationOutput) -> Observation {
    let samples: Vec<[f64; 3]> = output
        .sensors
        .imu
        .accel
        .iter()
        .map(|sample| [sample.x, sample.y, sample.z])
        .collect();
    let axis = gait_axis(&samples);
    let signal: Vec<f64> = samples.iter().map(|sample| sample[axis]).collect();
    let mut out = gait_observables(&signal, output.manifest.rates_hz[1]);
    if let Some(metrics) = &output.metrics {
        out.insert(name::SPEED_MEAN.to_string(), metrics.speed.mean);
    }
    out
}

/// One reference value.
#[derive(Debug, Clone, PartialEq)]
pub struct TargetEntry {
    /// Observable name from [`name`].
    pub name: String,
    /// What it should be.
    pub value: f64,
    /// How far off it may be before the error counts as one unit.
    ///
    /// A tolerance rather than a weight, because the observables have different
    /// units and a squared *relative* error lets one badly-off entry hide all the
    /// others: a ratio five hundred percent wrong contributes twenty-five while a
    /// cadence eight percent wrong contributes six thousandths, and the search then
    /// never bothers with the cadence. Dividing by a tolerance makes each entry's
    /// error read as "how many tolerances off", and the search balances them.
    pub tolerance: f64,
}

/// Reference values to match.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Target {
    /// The entries.
    pub entries: Vec<TargetEntry>,
}

impl Target {
    /// Builds a target from `(name, value, tolerance)` triples.
    pub fn new(entries: impl IntoIterator<Item = (&'static str, f64, f64)>) -> Self {
        Self {
            entries: entries
                .into_iter()
                .map(|(name, value, tolerance)| TargetEntry {
                    name: name.to_string(),
                    value,
                    tolerance: tolerance.max(f64::MIN_POSITIVE),
                })
                .collect(),
        }
    }

    /// Sum of squared errors, each in units of its own tolerance.
    ///
    /// An observable the run did not produce — a signal with no line in the step
    /// band, for instance — costs a large fixed penalty, so the search is pushed
    /// away from parameter vectors that produce no gait at all instead of treating
    /// them as perfect.
    pub fn loss(&self, observed: &Observation) -> f64 {
        const MISSING_PENALTY: f64 = 1.0e3;
        let mut total = 0.0;
        for entry in &self.entries {
            match observed.get(&entry.name) {
                Some(value) => {
                    let scaled = (value - entry.value) / entry.tolerance;
                    total += scaled * scaled;
                }
                None => total += MISSING_PENALTY,
            }
        }
        total
    }

    /// Largest error in units of tolerance, which is what the search minimises.
    pub fn worst_scaled_error(&self, observed: &Observation) -> f64 {
        self.entries
            .iter()
            .map(|entry| match observed.get(&entry.name) {
                Some(value) => ((value - entry.value) / entry.tolerance).abs(),
                None => f64::INFINITY,
            })
            .fold(0.0f64, f64::max)
    }

    /// Largest relative error over the entries, as a readable summary.
    pub fn worst_relative_error(&self, observed: &Observation) -> f64 {
        self.entries
            .iter()
            .filter(|entry| entry.value.abs() > f64::EPSILON)
            .map(|entry| match observed.get(&entry.name) {
                Some(value) => ((value - entry.value) / entry.value).abs(),
                None => f64::INFINITY,
            })
            .fold(0.0f64, f64::max)
    }
}

/// A parameter the optimiser may move.
///
/// An enum rather than a name lookup: the set of parameters under calibration is
/// part of the experiment, and a typo in a string should not silently produce a
/// search that moves nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Knob {
    /// Individual target speed, m/s.
    TargetSpeed,
    /// Step frequency, Hz.
    StepFrequency,
    /// Bounce amplitude, m.
    BounceAmplitude,
    /// Bounce waveform asymmetry: the second harmonic of the *displacement*.
    BounceBeta2,
    /// Second harmonic of the accelerometer's step signature, as a ratio.
    Harmonic2,
    /// Third harmonic of it.
    Harmonic3,
}

impl Knob {
    /// Name of the parameter as it appears in the output.
    pub fn name(self) -> &'static str {
        match self {
            Knob::TargetSpeed => "target_speed",
            Knob::StepFrequency => "step_frequency",
            Knob::BounceAmplitude => "bounce_amplitude_m",
            Knob::BounceBeta2 => "bounce_beta2",
            Knob::Harmonic2 => "harmonic_2_ratio",
            Knob::Harmonic3 => "harmonic_3_ratio",
        }
    }

    /// Bounds the search must stay inside.
    ///
    /// They are the range the design's parameter table calls plausible, widened
    /// only where a value outside it would still be physical: the harmonics are
    /// ratios of a fundamental, so they cannot be negative, and a bounce asymmetry
    /// above one would make the second harmonic the dominant term.
    pub fn bounds(self) -> (f64, f64) {
        match self {
            Knob::TargetSpeed => (1.0, 8.0),
            Knob::StepFrequency => (1.6, 3.6),
            Knob::BounceAmplitude => (0.0, 0.15),
            Knob::BounceBeta2 => (0.0, 1.0),
            Knob::Harmonic2 => (0.0, 1.0),
            Knob::Harmonic3 => (0.0, 1.0),
        }
    }

    /// Reads the parameter.
    pub fn get(self, person: &PersonParams, motion: &MotionConfig) -> f64 {
        match self {
            Knob::TargetSpeed => person.target_speed,
            Knob::StepFrequency => person.step_frequency,
            Knob::BounceAmplitude => person.bounce_amplitude_m,
            Knob::BounceBeta2 => motion.bounce_beta2,
            Knob::Harmonic2 => person.harmonic_2_ratio,
            Knob::Harmonic3 => person.harmonic_3_ratio,
        }
    }

    /// Writes the parameter, clamped to its bounds.
    pub fn set(self, person: &mut PersonParams, motion: &mut MotionConfig, value: f64) {
        let (low, high) = self.bounds();
        let value = value.clamp(low, high);
        match self {
            Knob::TargetSpeed => person.target_speed = value,
            Knob::StepFrequency => person.step_frequency = value,
            Knob::BounceAmplitude => person.bounce_amplitude_m = value,
            Knob::BounceBeta2 => motion.bounce_beta2 = value,
            Knob::Harmonic2 => person.harmonic_2_ratio = value,
            Knob::Harmonic3 => person.harmonic_3_ratio = value,
        }
    }
}

/// Controls of the search.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CalibrationOptions {
    /// Initial step of each coordinate, as a fraction of its plausible range.
    pub initial_step_fraction: f64,
    /// Stop when a coordinate's step falls below this fraction of its range.
    pub min_step_fraction: f64,
    /// Upper bound on evaluations, so a pathological loss cannot run forever.
    pub max_evaluations: usize,
}

impl Default for CalibrationOptions {
    fn default() -> Self {
        Self {
            initial_step_fraction: 0.15,
            min_step_fraction: 0.01,
            max_evaluations: 400,
        }
    }
}

/// What the search produced.
#[derive(Debug, Clone)]
pub struct CalibrationOutcome {
    /// Calibrated individual parameters.
    pub person: PersonParams,
    /// Calibrated motion configuration.
    pub motion: MotionConfig,
    /// Final loss.
    pub loss: f64,
    /// Number of evaluations spent.
    pub evaluations: usize,
    /// Loss after each improvement, with the evaluation it was found at.
    pub history: Vec<(usize, f64)>,
}

impl CalibrationOutcome {
    /// Calibrated values of the knobs.
    pub fn values(&self, knobs: &[Knob]) -> Vec<(&'static str, f64)> {
        knobs
            .iter()
            .map(|knob| (knob.name(), knob.get(&self.person, &self.motion)))
            .collect()
    }
}

/// Minimises a target's loss over the knobs by coordinate search.
///
/// Each coordinate is probed up and down by a step; the best move is kept, and when
/// no coordinate improves, every step is halved. It is the design's "coordinate
/// search" option: gradient-free, deterministic, and easy to reason about — its
/// bias is that it cannot follow a diagonal valley, which for a handful of weakly
/// coupled parameters costs evaluations rather than correctness.
pub fn coordinate_search<F>(
    start: PersonParams,
    motion: MotionConfig,
    knobs: &[Knob],
    target: &Target,
    options: &CalibrationOptions,
    evaluate: F,
) -> Result<CalibrationOutcome>
where
    F: Fn(&PersonParams, &MotionConfig) -> Result<Observation>,
{
    if knobs.is_empty() {
        return Err(CoreError::config("calibration needs at least one knob"));
    }
    let mut person = start;
    let mut motion = motion;
    let mut evaluations = 0usize;
    let mut history = Vec::new();

    let mut steps: Vec<f64> = knobs
        .iter()
        .map(|knob| {
            let (low, high) = knob.bounds();
            (high - low) * options.initial_step_fraction
        })
        .collect();
    let floor: Vec<f64> = knobs
        .iter()
        .map(|knob| {
            let (low, high) = knob.bounds();
            (high - low) * options.min_step_fraction
        })
        .collect();

    let mut best = {
        evaluations += 1;
        target.loss(&evaluate(&person, &motion)?)
    };
    history.push((evaluations, best));

    loop {
        if evaluations >= options.max_evaluations {
            break;
        }
        let mut improved = false;
        for (index, knob) in knobs.iter().enumerate() {
            let current = knob.get(&person, &motion);
            for direction in [1.0, -1.0] {
                if evaluations >= options.max_evaluations {
                    break;
                }
                let candidate = current + direction * steps[index];
                let (low, high) = knob.bounds();
                if candidate < low || candidate > high {
                    continue;
                }
                let mut probe_person = person.clone();
                let mut probe_motion = motion.clone();
                knob.set(&mut probe_person, &mut probe_motion, candidate);
                evaluations += 1;
                let loss = target.loss(&evaluate(&probe_person, &probe_motion)?);
                if loss < best - f64::EPSILON {
                    best = loss;
                    person = probe_person;
                    motion = probe_motion;
                    improved = true;
                    history.push((evaluations, best));
                }
            }
        }
        if !improved {
            let mut shrunk = false;
            for (step, floor) in steps.iter_mut().zip(floor.iter()) {
                if *step > *floor {
                    *step *= 0.5;
                    shrunk = true;
                }
            }
            if !shrunk {
                break;
            }
        }
    }

    Ok(CalibrationOutcome {
        person,
        motion,
        loss: best,
        evaluations,
        history,
    })
}
