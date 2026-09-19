//! Calibrates the gait-signature parameters against the reference recordings.
//!
//! The design leaves the bounce shape, the harmonic weights and the cadence to be
//! calibrated from real IMU data. The reference is the set of recordings converted
//! by `fetch_real_data`, measured with the *same* estimator the simulator's output
//! is measured with ([`ourealis_core::eval::gait_observables`]); this driver builds
//! the targets from those recordings, runs the coordinate search, and prints both
//! the calibrated values and the residual error.
//!
//! ```text
//! cargo run -p ourealis-core --example calibrate --release
//! ```
//!
//! With `--write` it also rewrites the target table at `data/real/targets.toml`, so
//! the calibration and the test that checks it agree on one set of numbers rather
//! than two.
//!
//! ## What is calibrated, and why those knobs
//!
//! | Knob | Target | Note |
//! |---|---|---|
//! | `step_frequency` | cadence of the jogging recordings | the treadmill and walking sources are excluded: they are not a moderate jog |
//! | `bounce_beta2` | second harmonic of the acceleration | the waveform's own asymmetry lands on the accelerometer, amplified by `k^2` |
//! | `harmonic_2_ratio` | second harmonic | the configured series adds to the waveform's own |
//! | `harmonic_3_ratio` | third harmonic | |
//!
//! The two second-harmonic knobs are not independent — they add at the same
//! frequency — so the search picks whichever combination the loss prefers, and the
//! reported outcome says how much of the total each contributes.

use std::path::PathBuf;

use glam::DVec2;

use ourealis_core::eval::calibrate::{self, Knob, Observation, Target, name as observable};
use ourealis_core::motion::MotionConfig;
use ourealis_core::person::{PersonParams, Preset};
use ourealis_core::plan::StandardRequest;
use ourealis_core::sim::{MapSource, SimulationConfig, Simulator};
use ourealis_map_format::synthetic::SyntheticMapSpec;

/// Bounce displacement asymmetry that accounts for the measured acceleration
/// harmonic on its own.
///
/// The displacement harmonic of order `k` is amplified by `k^2` in acceleration, so
/// a displacement asymmetry `b` shows up as `4b` in the ratio the recordings give.
/// The measured ratio is 0.195, hence `b = 0.049`.
const BETA2_FROM_DISPLACEMENT: f64 = 0.049;

/// Sources whose activity is a self-paced jog, which is what a Moderate preset
/// stands for. The treadmill source varies speed by design and the HAR source is
/// walking, so neither belongs in a cadence or harmonic target.
const JOGGING_SOURCES: [&str; 2] = ["wisdm2", "motionsense"];

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = workspace_root();
    let dir = root.join("data/real");
    let recordings = read_recordings(&dir)?;
    if recordings.is_empty() {
        return Err(format!(
            "no converted recording under {}; run `cargo run -p ourealis-core --example fetch_real_data` first",
            dir.display()
        )
        .into());
    }

    // --- build the targets from the reference recordings ---
    let mut cadences = Vec::new();
    let mut seconds = Vec::new();
    let mut thirds = Vec::new();
    for recording in &recordings {
        let Some(gait) = calibrate::gait_of(&recording.signal, recording.rate_hz) else {
            continue;
        };
        // The same source selection applies to all three targets: a treadmill
        // trial at a fixed speed and a walking collection are different
        // populations, and letting them into the harmonic means would fit the
        // placeholders to a gait the Moderate preset does not describe.
        if JOGGING_SOURCES.contains(&recording.source.as_str()) {
            cadences.push(gait.step_hz);
            seconds.push(gait.second_ratio);
            thirds.push(gait.third_ratio);
        }
    }
    if cadences.is_empty() || seconds.is_empty() {
        return Err("the recordings produced no gait measurement".into());
    }
    let mean = |values: &[f64]| values.iter().sum::<f64>() / values.len() as f64;
    // Tolerances rather than weights: a jog's cadence varies by about a tenth of a
    // hertz across people, and the harmonic ratios by a few hundredths.
    let target = Target::new([
        (observable::CADENCE_HZ, mean(&cadences), 0.10),
        (observable::A2_RATIO, mean(&seconds), 0.03),
        (observable::A3_RATIO, mean(&thirds), 0.03),
    ]);
    println!("=== reference ({} series) ===", recordings.len());
    for entry in &target.entries {
        println!("  {:<12} {:.4}", entry.name, entry.value);
    }

    // --- calibrate ---
    // The bounce asymmetry is deliberately *not* a knob. Its second harmonic is
    // amplified by four in the acceleration (a displacement harmonic of order k
    // appears as k^2 in the second derivative), so the same measured ratio can be
    // produced by a small asymmetry or by the configured series — the accelerometer
    // cannot tell them apart. The barometer could, and there is no reference for it,
    // so the split is fixed by physics instead: a displacement asymmetry of 0.049,
    // which alone accounts for the measured ratio through the k^2 amplification,
    // leaving the configured second harmonic to fit what remains.
    let knobs = [Knob::StepFrequency, Knob::Harmonic2, Knob::Harmonic3];
    let options = calibrate::CalibrationOptions {
        max_evaluations: 160,
        ..Default::default()
    };
    let start_person = PersonParams::preset(Preset::Moderate);
    let start_motion = MotionConfig {
        bounce_beta2: BETA2_FROM_DISPLACEMENT,
        ..Default::default()
    };
    println!(
        "=== start ===\n  {:?}",
        knobs
            .iter()
            .map(|knob| (knob.name(), knob.get(&start_person, &start_motion)))
            .collect::<Vec<_>>()
    );

    if std::env::args().any(|a| a == "--verbose") {
        let start = evaluate(&start_person, &start_motion)?;
        println!("=== start observation ===");
        for (key, value) in &start {
            println!("  {key:<12} {value:.4}");
        }
        println!("  loss {:.6}", target.loss(&start));
        for cadence in [2.2, 2.4, 2.5, 2.6, 2.8, 3.0] {
            let mut probe = start_person.clone();
            probe.step_frequency = cadence;
            let observed = evaluate(&probe, &start_motion)?;
            println!(
                "  step_frequency {cadence:.2} -> measured {:.4} Hz, loss {:.6}",
                observed
                    .get(observable::CADENCE_HZ)
                    .copied()
                    .unwrap_or(f64::NAN),
                target.loss(&observed)
            );
        }
    }
    let outcome = calibrate::coordinate_search(
        start_person,
        start_motion,
        &knobs,
        &target,
        &options,
        evaluate,
    )?;

    println!(
        "=== calibrated ({} evaluations, loss {:.6}) ===",
        outcome.evaluations, outcome.loss
    );
    for (name, value) in outcome.values(&knobs) {
        println!("  {name:<20} {value:.4}");
    }
    let observed = evaluate(&outcome.person, &outcome.motion)?;
    println!("=== residuals ===");
    for entry in &target.entries {
        let value = observed.get(&entry.name).copied().unwrap_or(f64::NAN);
        println!(
            "  {:<12} reference {:.4}  simulated {:.4}  off by {:+.1}%",
            entry.name,
            entry.value,
            value,
            100.0 * (value - entry.value) / entry.value
        );
    }
    println!(
        "  worst error {:.1}% ({:.2} tolerances)",
        100.0 * target.worst_relative_error(&observed),
        target.worst_scaled_error(&observed)
    );

    if std::env::args().any(|argument| argument == "--write") {
        let mut text = String::from(
            "# Reference gait targets, measured from the converted recordings.\n\
             # Written by `cargo run -p ourealis-core --example calibrate -- --write`.\n\
             # Both sides use the estimator in `ourealis_core::eval::calibrate`.\n",
        );
        for entry in &target.entries {
            text.push_str(&format!("{} = {:.6}\n", entry.name, entry.value));
        }
        std::fs::write(dir.join("targets.toml"), text)?;
        println!("wrote {}", dir.join("targets.toml").display());
    }
    Ok(())
}

/// Runs one simulation and measures it.
fn evaluate(person: &PersonParams, motion: &MotionConfig) -> ourealis_core::Result<Observation> {
    let mut config = SimulationConfig::deterministic();
    config.motion = motion.clone();
    config.motion.adapt_individual = false;
    if !config.with_metrics {
        config.with_metrics = true;
    }
    let output = Simulator::builder()
        .map(MapSource::synthetic(SyntheticMapSpec::compact()))
        .person(person.clone())
        .standard(StandardRequest::new(
            DVec2::new(40.0, 100.0),
            DVec2::new(260.0, 100.0),
        ))
        .config(config)
        .seed(17)
        .build()?
        .run()?;
    Ok(calibrate::run_observables(&output))
}

/// One reference series.
struct Recording {
    source: String,
    signal: Vec<f64>,
    rate_hz: f64,
}

/// Inertial channel a table holds.
///
/// Only the accelerometer carries the design's step signature: the harmonic
/// ratios are defined on it, so a gyroscope table must not enter the target
/// means even when its source is a jogging collection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Channel {
    Accel,
    Gyro,
}

/// Reads every converted table, reducing each series to its gait axis.
fn read_recordings(dir: &PathBuf) -> Result<Vec<Recording>, Box<dyn std::error::Error>> {
    let mut tables: Vec<PathBuf> = std::fs::read_dir(dir)?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.extension().is_some_and(|extension| extension == "csv"))
        .collect();
    tables.sort();
    let mut out = Vec::new();
    for table in tables {
        let source = table
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_default();
        let meta = std::fs::read_to_string(table.with_extension("meta")).unwrap_or_default();
        let channel = if meta
            .lines()
            .find_map(|line| line.strip_prefix("channel ="))
            .is_some_and(|value| value.trim().eq_ignore_ascii_case("gyro"))
        {
            Channel::Gyro
        } else {
            Channel::Accel
        };
        if channel != Channel::Accel {
            continue;
        }
        let rate_hz = meta
            .lines()
            .find_map(|line| line.strip_prefix("rate_hz ="))
            .and_then(|value| value.trim().parse::<f64>().ok())
            .unwrap_or(20.0);
        let text = std::fs::read_to_string(&table)?;
        let mut by_subject: std::collections::BTreeMap<u32, Vec<[f64; 3]>> =
            std::collections::BTreeMap::new();
        for line in text.lines().skip(1) {
            let mut fields = line.split(',');
            let (Some(subject), Some(ax), Some(ay), Some(az)) =
                (fields.next(), fields.next(), fields.next(), fields.next())
            else {
                continue;
            };
            let (Ok(subject), Ok(ax), Ok(ay), Ok(az)) = (
                subject.parse::<u32>(),
                ax.parse::<f64>(),
                ay.parse::<f64>(),
                az.parse::<f64>(),
            ) else {
                continue;
            };
            by_subject.entry(subject).or_default().push([ax, ay, az]);
        }
        for (_, samples) in by_subject {
            if (samples.len() as f64) < 4.0 * rate_hz {
                continue;
            }
            let axis = calibrate::gait_axis(&samples);
            out.push(Recording {
                source: source.clone(),
                signal: samples.iter().map(|sample| sample[axis]).collect(),
                rate_hz,
            });
        }
    }
    Ok(out)
}

fn workspace_root() -> PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(std::path::Path::parent)
        .map(std::path::Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
}
