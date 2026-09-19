//! Calibration: the optimiser, the estimator, and the calibrated defaults.
//!
//! Three things are checked here, and they are separable on purpose:
//!
//! * the **estimator** recovers a signature it was given, on a signal with no
//!   simulator involved — if this drifts, every calibration number moves;
//! * the **optimiser** recovers a known parameter vector from a target built out of
//!   it, which is the self-consistency check the design's calibration loop needs
//!   before it can be trusted on real data;
//! * the **calibrated preset** matches the reference targets measured from the
//!   recordings, which is skipped when the recordings have not been converted.

use std::collections::BTreeMap;
use std::path::PathBuf;

use glam::DVec2;

use ourealis_core::eval::calibrate::{
    self, CalibrationOptions, Knob, Observation, Target, name as observable,
};
use ourealis_core::motion::MotionConfig;
use ourealis_core::person::{PersonParams, Preset};
use ourealis_core::plan::StandardRequest;
use ourealis_core::sim::{MapSource, SimulationConfig, Simulator};
use ourealis_map_format::synthetic::SyntheticMapSpec;

/// Rate used by the synthetic signals in this file, Hz.
const RATE_HZ: f64 = 100.0;

/// A signal whose gait signature is known exactly.
///
/// The fundamental, the second and the third harmonic with fixed relative phases,
/// plus a small amount of noise so the estimator cannot lean on an exact zero.
fn synthetic_signal(step_hz: f64, a2: f64, a3: f64, seconds: f64) -> Vec<f64> {
    let count = (seconds * RATE_HZ) as usize;
    (0..count)
        .map(|index| {
            let t = index as f64 / RATE_HZ;
            let phase = std::f64::consts::TAU * step_hz * t;
            1.0 * phase.sin()
                + a2 * (2.0 * phase + 0.7).sin()
                + a3 * (3.0 * phase + 1.3).sin()
                + 0.05 * ((index * 7919) % 13) as f64 / 13.0
        })
        .collect()
}

#[test]
fn the_estimator_recovers_a_known_signature() {
    // The fallback case the calibrated numbers depend on: if the estimator reads the
    // bin grid instead of the line, the ratios it reports depend on where the cadence
    // falls, and the calibration would be fitting the grid.
    for step_hz in [2.20, 2.50, 2.77, 3.10] {
        for (a2, a3) in [(0.0, 0.0), (0.23, 0.14), (0.5, 0.05)] {
            let signal = synthetic_signal(step_hz, a2, a3, 60.0);
            let gait = calibrate::gait_of(&signal, RATE_HZ).expect("a step peak");
            assert!(
                (gait.step_hz - step_hz).abs() < 0.02,
                "cadence {:.3} against {step_hz}",
                gait.step_hz
            );
            assert!(
                (gait.second_ratio - a2).abs() < 0.03,
                "second harmonic {:.3} against {a2} at {step_hz} Hz",
                gait.second_ratio
            );
            assert!(
                (gait.third_ratio - a3).abs() < 0.03,
                "third harmonic {:.3} against {a3} at {step_hz} Hz",
                gait.third_ratio
            );
        }
    }
}

#[test]
fn the_optimiser_recovers_a_known_parameter_vector() {
    // Self-consistency: build a target from one parameter vector and start the search
    // from another. Without this the loop could converge on real data and still be
    // reporting whatever its starting point was.
    let truth = (2.85, 0.42, 0.21);
    let signal = synthetic_signal(truth.0, truth.1, truth.2, 60.0);
    let measured = calibrate::gait_observables(&signal, RATE_HZ);
    let target = Target::new([
        (
            observable::CADENCE_HZ,
            measured[observable::CADENCE_HZ],
            0.02,
        ),
        (observable::A2_RATIO, measured[observable::A2_RATIO], 0.01),
        (observable::A3_RATIO, measured[observable::A3_RATIO], 0.01),
    ]);

    // The same synthesis, driven by the candidate parameters.
    let evaluate = |person: &PersonParams, _motion: &MotionConfig| {
        let signal = synthetic_signal(
            person.step_frequency,
            person.harmonic_2_ratio,
            person.harmonic_3_ratio,
            60.0,
        );
        Ok::<Observation, ourealis_core::CoreError>(calibrate::gait_observables(&signal, RATE_HZ))
    };

    let mut start = PersonParams::preset(Preset::Moderate);
    start.step_frequency = 2.20;
    start.harmonic_2_ratio = 0.10;
    start.harmonic_3_ratio = 0.50;
    let outcome = calibrate::coordinate_search(
        start,
        MotionConfig::default(),
        &[Knob::StepFrequency, Knob::Harmonic2, Knob::Harmonic3],
        &target,
        &CalibrationOptions {
            max_evaluations: 200,
            ..Default::default()
        },
        evaluate,
    )
    .expect("search");

    let found = (
        outcome.person.step_frequency,
        outcome.person.harmonic_2_ratio,
        outcome.person.harmonic_3_ratio,
    );
    println!(
        "recovered ({:.4}, {:.4}, {:.4}) from ({:.4}, {:.4}, {:.4}) in {} evaluations",
        found.0, found.1, found.2, truth.0, truth.1, truth.2, outcome.evaluations
    );
    assert!(
        (found.0 - truth.0).abs() < 0.03,
        "cadence {:.4} against {:.4}",
        found.0,
        truth.0
    );
    assert!(
        (found.1 - truth.1).abs() < 0.02,
        "second harmonic {:.4} against {:.4}",
        found.1,
        truth.1
    );
    assert!(
        (found.2 - truth.2).abs() < 0.02,
        "third harmonic {:.4} against {:.4}",
        found.2,
        truth.2
    );
}

#[test]
fn the_loss_balances_observables_by_tolerance() {
    // A squared relative error would let one very wrong observable hide every other,
    // so the error of each is divided by its own tolerance. This pins that, and the
    // penalty for an observable a run did not produce.
    let target = Target::new([
        (observable::CADENCE_HZ, 2.5, 0.1),
        (observable::A2_RATIO, 0.2, 0.02),
    ]);
    let exact: Observation = [
        (observable::CADENCE_HZ.to_string(), 2.5),
        (observable::A2_RATIO.to_string(), 0.2),
    ]
    .into_iter()
    .collect();
    assert!(target.loss(&exact) < 1e-12);

    // One cadence tolerance off, and one ratio tolerance off, must cost the same.
    let cadence_off: Observation = [
        (observable::CADENCE_HZ.to_string(), 2.6),
        (observable::A2_RATIO.to_string(), 0.2),
    ]
    .into_iter()
    .collect();
    let ratio_off: Observation = [
        (observable::CADENCE_HZ.to_string(), 2.5),
        (observable::A2_RATIO.to_string(), 0.22),
    ]
    .into_iter()
    .collect();
    assert!((target.loss(&cadence_off) - 1.0).abs() < 1e-12);
    assert!((target.loss(&ratio_off) - 1.0).abs() < 1e-12);

    // A missing observable is a failure, not a perfect score.
    let missing: Observation = BTreeMap::new();
    assert!(target.loss(&missing) > 100.0);
}

/// Reference targets measured from the recordings, if they have been converted.
fn reference_targets() -> Option<Target> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()?
        .parent()?
        .to_path_buf();
    let text = std::fs::read_to_string(root.join("data/real/targets.toml")).ok()?;
    let mut entries = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (name, value) = line.split_once('=')?;
        let value: f64 = value.trim().parse().ok()?;
        let tolerance = match name.trim() {
            observable::CADENCE_HZ => 0.10,
            _ => 0.03,
        };
        entries.push((name.trim().to_string(), value, tolerance));
    }
    if entries.is_empty() {
        return None;
    }
    let mut target = Target::default();
    for (name, value, tolerance) in entries {
        target
            .entries
            .push(ourealis_core::eval::calibrate::TargetEntry {
                name,
                value,
                tolerance,
            });
    }
    Some(target)
}

#[test]
fn the_calibrated_preset_matches_the_reference_recordings() {
    // The end-to-end check of the calibration: the shipped defaults, run through the
    // simulator and measured with the shared estimator, must sit inside the reference
    // tolerances. Skipped when the recordings have not been converted.
    let Some(target) = reference_targets() else {
        println!(
            "skipping the_calibrated_preset_matches_the_reference_recordings: no \
             data/real/targets.toml; run `cargo run -p ourealis-core --example calibrate -- --write` \
             after `fetch_real_data`"
        );
        return;
    };
    let candidate_target = Target::new(
        target
            .entries
            .iter()
            .map(|entry| {
                let name: &'static str = match entry.name.as_str() {
                    observable::CADENCE_HZ => observable::CADENCE_HZ,
                    observable::A2_RATIO => observable::A2_RATIO,
                    _ => observable::A3_RATIO,
                };
                (name, entry.value, entry.tolerance)
            })
            .collect::<Vec<_>>(),
    );

    let output = Simulator::builder()
        .map(MapSource::synthetic(SyntheticMapSpec::compact()))
        .person(PersonParams::preset(Preset::Moderate))
        .standard(StandardRequest::new(
            DVec2::new(40.0, 100.0),
            DVec2::new(260.0, 100.0),
        ))
        .config(SimulationConfig::deterministic())
        .seed(17)
        .build()
        .expect("simulator")
        .run()
        .expect("run");
    let observed = calibrate::run_observables(&output);
    println!("=== calibrated preset against the recordings ===");
    for entry in &candidate_target.entries {
        let value = observed.get(&entry.name).copied().unwrap_or(f64::NAN);
        println!(
            "  {:<12} reference {:.4}  simulated {:.4}  off by {:+.1}%",
            entry.name,
            entry.value,
            value,
            100.0 * (value - entry.value) / entry.value
        );
    }
    let worst = candidate_target.worst_scaled_error(&observed);
    println!("  worst error {worst:.2} tolerances");
    assert!(
        worst < 3.0,
        "the shipped defaults are {worst:.2} tolerances from the reference recordings"
    );
}

#[test]
fn a_short_record_does_not_panic_the_estimator() {
    // The estimator scans the local maximum among the bins around a line and then
    // interpolates between its neighbours. On a record short enough that the peak
    // can fall on the last bin, the neighbour does not exist — the scan has to stop
    // one bin short, not index past the end.
    for count in 4..200 {
        for rate_hz in [8.0, 25.0, 50.0, 100.0] {
            let signal: Vec<f64> = (0..count).map(|index| (0.5 * index as f64).sin()).collect();
            let _ = ourealis_core::eval::calibrate::gait_of(&signal, rate_hz);
            let _ = ourealis_core::eval::calibrate::gait_observables(&signal, rate_hz);
        }
    }
}
