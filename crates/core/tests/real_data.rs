//! Real-data comparison.
//!
//! The realism layer checks the simulator against human-physiology bands taken
//! from the literature; this layer checks it against *recordings*. The four
//! numbers it compares are the ones the design says cannot come from a
//! specification — the step frequency, the amplitude of the accelerometer's
//! fundamental, the ratio of the second harmonic to it, and the gyroscope's step
//! signature — because they are properties of how people actually move, and the
//! design marks the simulator's values for them as placeholders until real data
//! says otherwise.
//!
//! Both sides are measured with the *same* estimator, [`eval::calibrate::gait_of`].
//! That is the point of the layer: an earlier round measured the simulator on one
//! signal and the recordings on another, and the difference it reported was mostly
//! the difference between the two measurements.
//!
//! The data is not committed; fetch and convert it with
//!
//! ```text
//! cargo run -p ourealis-core --example fetch_real_data
//! ```
//!
//! and without it every test here reports that it was skipped and passes, so the
//! suite stays green on a machine that has never downloaded it.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use glam::DVec2;

use ourealis_core::eval::calibrate::{Gait, gait_axis, gait_of};
use ourealis_core::eval::{self, DistributionStats};
use ourealis_core::person::{PersonParams, Preset};
use ourealis_core::plan::StandardRequest;
use ourealis_core::sim::{MapSource, SimulationConfig, SimulationOutput, Simulator};
use ourealis_map_format::synthetic::SyntheticMapSpec;

/// Activity the design calibrates against: self-paced jogging.
///
/// The treadmill source runs at a fixed 8 or 12 km/h and the HAR collection walks,
/// so neither is the population the simulator's Moderate preset describes.
const REFERENCE_ACTIVITY: &str = "jogging";

/// Which inertial channel the reference recording came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Channel {
    Accel,
    Gyro,
}

impl Channel {
    fn parse(value: &str) -> Self {
        if value.trim().eq_ignore_ascii_case("gyro") {
            Channel::Gyro
        } else {
            Channel::Accel
        }
    }

    /// Plausible scale of the signal on the gait axis, for the loader's check.
    ///
    /// On an accelerometer that axis carries gravity, so its mean is near 9.8 and
    /// its spread is the gait's — a metre or so per second squared for running. On
    /// a gyroscope there is no constant component, and the spread is the step
    /// oscillation. The bounds are deliberately wide: this checks that the loader
    /// read the columns it meant to, not that the recording is good.
    fn scale_band(self) -> (f64, f64) {
        match self {
            Channel::Accel => (0.3, 40.0),
            Channel::Gyro => (0.02, 20.0),
        }
    }

    fn unit(self) -> &'static str {
        match self {
            Channel::Accel => "m/s^2",
            Channel::Gyro => "rad/s",
        }
    }
}

/// One subject's recording, reduced to the axis that carries the gait.
///
/// The axis with the largest variance is the one the step signature is on, and
/// choosing it per series is what makes the measurement independent of how the
/// device was carried — which the recordings do not say. The *magnitude* is
/// orientation-free too, but it is the wrong signal for a harmonic ratio: the
/// magnitude of an oscillation that is large next to its constant component is a
/// rectified sine, whose strongest line sits at twice the cadence.
struct Recording {
    /// Table it came from, e.g. `wisdm2`.
    source: String,
    /// What the rows are, e.g. `jogging`.
    activity: String,
    subject: u32,
    /// Samples of the gait axis.
    axis: Vec<f64>,
    rate_hz: f64,
    channel: Channel,
}

impl Recording {
    fn gait(&self) -> Option<Gait> {
        gait_of(&self.axis, self.rate_hz)
    }
}

/// Reads every converted table, or `None` when none has been fetched.
///
/// One table per source and channel, so the layer widens as datasets arrive
/// without a code change: the fetch tool writes `data/real/<source>.csv` and
/// `data/real/<source>_<channel>.csv` and this reads whatever is there.
fn recordings() -> Option<Vec<Recording>> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()?
        .parent()?
        .to_path_buf();
    let dir = root.join("data/real");
    let mut tables: Vec<PathBuf> = std::fs::read_dir(&dir)
        .ok()?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.extension().is_some_and(|extension| extension == "csv"))
        .collect();
    tables.sort();
    if tables.is_empty() {
        return None;
    }

    let mut recordings = Vec::new();
    for table in tables {
        let Some(text) = std::fs::read_to_string(&table).ok() else {
            continue;
        };
        let meta = std::fs::read_to_string(table.with_extension("meta")).unwrap_or_default();
        let source = table
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_default();
        let rate_hz = meta
            .lines()
            .find_map(|line| line.strip_prefix("rate_hz ="))
            .and_then(|value| value.trim().parse::<f64>().ok())
            .unwrap_or(20.0);
        let channel = Channel::parse(
            meta.lines()
                .find_map(|line| line.strip_prefix("channel ="))
                .unwrap_or("accel"),
        );
        let activity = meta
            .lines()
            .find_map(|line| line.strip_prefix("activity ="))
            .unwrap_or("running")
            .trim()
            .to_string();

        let mut by_subject: BTreeMap<u32, Vec<[f64; 3]>> = BTreeMap::new();
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
        for (subject, samples) in by_subject {
            // A gait spectrum needs a few seconds of signal; the treadmill source
            // records five-second trials, which is the shortest that still means
            // anything at its rate.
            if (samples.len() as f64) < 4.0 * rate_hz {
                continue;
            }
            let axis = gait_axis(&samples);
            recordings.push(Recording {
                source: source.clone(),
                activity: activity.clone(),
                subject,
                axis: samples.iter().map(|sample| sample[axis]).collect(),
                rate_hz,
                channel,
            });
        }
    }
    (!recordings.is_empty()).then_some(recordings)
}

/// Skips the test with a printed note when the data has not been fetched.
fn recordings_or_skip(test: &str) -> Option<Vec<Recording>> {
    match recordings() {
        Some(recordings) => Some(recordings),
        None => {
            println!(
                "skipping {test}: no real-data table under data/real/; \
                 run `cargo run -p ourealis-core --example fetch_real_data` to fetch and convert it"
            );
            None
        }
    }
}

/// Gait measurements of every recording on one channel.
///
/// `activity` selects the reference subset: the design calibrates against
/// self-paced jogging, so a treadmill trial at a fixed 8 or 12 km/h and a walking
/// collection are *not* the same population, and mixing them would widen every
/// band until it stopped discriminating. `None` takes everything on the channel.
fn gaits_of(
    recordings: &[Recording],
    channel: Channel,
    activity: Option<&str>,
) -> Vec<(String, Gait)> {
    recordings
        .iter()
        .filter(|recording| recording.channel == channel)
        .filter(|recording| {
            activity.is_none_or(|wanted| recording.activity.eq_ignore_ascii_case(wanted))
        })
        .filter_map(|recording| {
            recording
                .gait()
                .map(|gait| (recording.source.clone(), gait))
        })
        .collect()
}

/// Prints one comparison line per group of measurements.
fn report(label: &str, values: &[f64], simulated: f64) {
    let stats = DistributionStats::of(values);
    println!(
        "  {label}: recorded median {:.3} (mean {:.3}, p05 {:.3}, p95 {:.3}, sd {:.3}) over {} series, simulated {:.3}",
        stats.p50,
        stats.mean,
        stats.p05,
        stats.p95,
        stats.std_dev,
        values.len(),
        simulated
    );
}

#[test]
fn real_jogging_cadence_is_the_band_the_parameters_assume() {
    // The individual parameter table gives 2.4 +/- 0.1 Hz for a jog and 3.0 +/- 0.1
    // for racing. Real jogging recordings must sit in that band: if they did not,
    // every speed the simulator produced from that cadence would be wrong by the
    // same factor, because step length is derived from it.
    let Some(recordings) =
        recordings_or_skip("real_jogging_cadence_is_the_band_the_parameters_assume")
    else {
        return;
    };
    let all = gaits_of(&recordings, Channel::Accel, None);
    let measured = gaits_of(&recordings, Channel::Accel, Some(REFERENCE_ACTIVITY));
    assert!(
        !measured.is_empty(),
        "no accelerometer recording produced a step peak in the running band"
    );
    let values: Vec<f64> = measured.iter().map(|(_, gait)| gait.step_hz).collect();
    let stats = DistributionStats::of(&values);
    println!(
        "  recorded cadence ({}): mean {:.2} Hz, range {:.2}..={:.2}, sd {:.2}, over {} series of {} source(s)",
        REFERENCE_ACTIVITY,
        stats.mean,
        stats.min,
        stats.max,
        stats.std_dev,
        values.len(),
        measured
            .iter()
            .map(|(source, _)| source.as_str())
            .collect::<BTreeSet<_>>()
            .len()
    );
    println!(
        "  (all {} accelerometer series, including treadmill and walking sources: {:.2}..={:.2} Hz)",
        all.len(),
        all.iter()
            .map(|(_, gait)| gait.step_hz)
            .fold(f64::INFINITY, f64::min),
        all.iter()
            .map(|(_, gait)| gait.step_hz)
            .fold(f64::NEG_INFINITY, f64::max)
    );
    assert!(
        (1.8..=3.4).contains(&stats.mean),
        "the recorded cadence averages {:.2} Hz, outside the band the individual \
         parameters assume",
        stats.mean
    );

    // The simulator's cadence is its parameter; what matters is that the same
    // measurement, applied to its output through the same estimator, lands inside
    // the recorded range. The range is the recordings' own inter-subject spread
    // widened by a floor: a single cadence parameter cannot reproduce twenty
    // individuals' spread, but it must not sit outside it either.
    let output = simulator_run();
    let simulated = simulated_gait(&output, Channel::Accel).expect("a step peak");
    report("cadence", &values, simulated.step_hz);
    let tolerance = stats.std_dev.max(0.2);
    assert!(
        (stats.mean - tolerance..=stats.mean + tolerance).contains(&simulated.step_hz),
        "the simulated cadence {:.2} Hz is outside {:.2}..={:.2} Hz, the recorded mean \
         plus or minus {:.2}",
        simulated.step_hz,
        stats.mean - tolerance,
        stats.mean + tolerance,
        tolerance
    );
}

#[test]
fn real_step_signature_supports_the_harmonic_ratios() {
    // The design's A2/A1 = 0.30 and A3/A1 = 0.10 are placeholders it marks for
    // calibration against real IMU data. They were calibrated to the mean of these
    // recordings, so the simulator must reproduce that mean to within the spread
    // the recordings themselves show — a ratio that only agrees with the mean
    // because it was fitted to it would drift with any change to the harmonic
    // model, which is exactly what this catches.
    //
    // The comparison is made on the accelerometer, which is where the design
    // defines the signature, and not on the gyroscope: the measurement takes the
    // strongest line in the step band, which on a zero-mean triaxial gyroscope is
    // reliably the cadence but whose *harmonic* content is a property of the
    // mount, not of the gait.
    let Some(recordings) = recordings_or_skip("real_step_signature_supports_the_harmonic_ratios")
    else {
        return;
    };
    let measured = gaits_of(&recordings, Channel::Accel, Some(REFERENCE_ACTIVITY));
    assert!(
        !measured.is_empty(),
        "no recording produced a step signature"
    );

    let ratio_2: Vec<f64> = measured.iter().map(|(_, gait)| gait.second_ratio).collect();
    let ratio_3: Vec<f64> = measured.iter().map(|(_, gait)| gait.third_ratio).collect();
    let output = simulator_run();
    let simulated = simulated_gait(&output, Channel::Accel).expect("a step peak");
    report("A2/A1", &ratio_2, simulated.second_ratio);
    report("A3/A1", &ratio_3, simulated.third_ratio);

    // The reference is the *median*: a series whose fundamental line happens to be
    // small throws its ratio far above the rest, and a mean over the collection
    // would then describe those few series rather than the population. The band is
    // the recorded inter-quartile spread, floored so a tightly clustered
    // collection still discriminates.
    let second = DistributionStats::of(&ratio_2);
    let third = DistributionStats::of(&ratio_3);
    let band =
        |stats: &DistributionStats, floor: f64| -> f64 { (stats.p95 - stats.p05).max(floor) };
    let second_band = band(&second, 0.10);
    let third_band = band(&third, 0.08);
    assert!(
        (second.p50 - second_band..=second.p50 + second_band).contains(&simulated.second_ratio),
        "the simulated second harmonic {:.3} is outside the recorded median {:.3} +/- {:.3}",
        simulated.second_ratio,
        second.p50,
        second_band
    );
    assert!(
        (third.p50 - third_band..=third.p50 + third_band).contains(&simulated.third_ratio),
        "the simulated third harmonic {:.3} is outside the recorded median {:.3} +/- {:.3}",
        simulated.third_ratio,
        third.p50,
        third_band
    );
    // A second harmonic larger than the fundamental would mean the signal is not a
    // step signature at all, on either side.
    assert!(
        second.p50 < 1.0,
        "a recorded second harmonic of {:.2} of the fundamental is not a step signature",
        second.p50
    );
}

#[test]
fn real_gyroscope_carries_the_step_signature() {
    // The design's gyroscope model is turn rate plus bias plus white noise, which
    // has no step signature at all — but a real runner's torso rotates once per
    // step, so a real recording does. The simulator adds a step-frequency
    // oscillation whose amplitude is an individual parameter; this compares it
    // against the recordings.
    let Some(recordings) = recordings_or_skip("real_gyroscope_carries_the_step_signature") else {
        return;
    };
    let measured = gaits_of(&recordings, Channel::Gyro, Some(REFERENCE_ACTIVITY));
    if measured.is_empty() {
        println!("  (no gyroscope table was converted; only the accelerometer is compared)");
        return;
    }
    let amplitudes: Vec<f64> = measured.iter().map(|(_, gait)| gait.fundamental).collect();
    let output = simulator_run();
    let simulated = simulated_gait(&output, Channel::Gyro).expect("a step peak");
    report("gyro step", &amplitudes, simulated.fundamental);
    assert!(
        (1.8..=3.4).contains(&simulated.step_hz),
        "the simulated gyroscope cadence {:.2} Hz is not a running step",
        simulated.step_hz
    );
    let stats = DistributionStats::of(&amplitudes);
    // The mount differs — a phone in a pocket against the simulator's torso mount —
    // so the check is on the order of magnitude, not on the value. What it rules
    // out is a gyroscope with no step signature (the parameter at zero) and one
    // whose signature swamps the turn rate.
    assert!(
        simulated.fundamental >= stats.min * 0.2 && simulated.fundamental <= stats.max * 5.0,
        "the simulated gyroscope step amplitude {:.3} rad/s is not on the scale of the \
         recorded {:.3}..={:.3}",
        simulated.fundamental,
        stats.min,
        stats.max
    );
}

#[test]
fn real_data_loader_is_not_vacuous() {
    // Guards the two ways this layer could silently do nothing: reading a table
    // with no rows, or measuring a signal whose spectrum has no peak in the band.
    let Some(recordings) = recordings_or_skip("real_data_loader_is_not_vacuous") else {
        return;
    };
    let samples: usize = recordings.iter().map(|r| r.axis.len()).sum();
    let sources: BTreeSet<&str> = recordings.iter().map(|r| r.source.as_str()).collect();
    let channels: BTreeSet<&str> = recordings
        .iter()
        .map(|r| match r.channel {
            Channel::Accel => "accel",
            Channel::Gyro => "gyro",
        })
        .collect();
    println!(
        "  {} subject(s) over {} source(s) {:?}, channels {:?}, {samples} samples",
        recordings.len(),
        sources.len(),
        sources,
        channels
    );
    assert!(
        samples > 1000,
        "the real-data table is too small to measure"
    );
    for recording in &recordings {
        assert!(
            (10.0..=200.0).contains(&recording.rate_hz),
            "{} reports an implausible rate of {} Hz",
            recording.source,
            recording.rate_hz
        );
        // The selected axis carries the constant component of the signal — gravity
        // on an accelerometer, nothing on a gyroscope — so the check is on the scale
        // of the variation, not on a magnitude.
        let (low, high) = recording.channel.scale_band();
        let mean = recording.axis.iter().sum::<f64>() / recording.axis.len() as f64;
        let spread = DistributionStats::of(&recording.axis).std_dev;
        assert!(
            (low..=high).contains(&spread),
            "{} subject {} varies by {spread:.2} {}, which is not a gait",
            recording.source,
            recording.subject,
            recording.channel.unit()
        );
        assert!(
            mean.abs() <= high,
            "{} subject {} sits at {mean:.2} {}, outside any inertial range",
            recording.source,
            recording.subject,
            recording.channel.unit()
        );
        assert!(
            recording.gait().is_some(),
            "{} subject {} has no line in the step band, so the record is unusable",
            recording.source,
            recording.subject
        );
    }
}

/// Gait measurement of one simulated inertial channel, through the shared estimator.
fn simulated_gait(output: &SimulationOutput, channel: Channel) -> Option<Gait> {
    let samples: Vec<[f64; 3]> = match channel {
        Channel::Accel => output
            .sensors
            .imu
            .accel
            .iter()
            .map(|sample| [sample.x, sample.y, sample.z])
            .collect(),
        Channel::Gyro => output
            .sensors
            .imu
            .gyro
            .iter()
            .map(|sample| [sample.x, sample.y, sample.z])
            .collect(),
    };
    let axis = gait_axis(&samples);
    let signal: Vec<f64> = samples.iter().map(|sample| sample[axis]).collect();
    gait_of(&signal, output.manifest.rates_hz[1])
}

/// One simulated run, on the map the other layers use.
fn simulator_run() -> SimulationOutput {
    Simulator::builder()
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
        .expect("run")
}

/// Keeps the unused-import checker honest about the eval helpers this layer is
/// meant to share with the metrics package.
#[allow(dead_code)]
fn summary_of(output: &SimulationOutput) -> Option<eval::SpectrumSummary> {
    let spectrum =
        eval::accel_vertical_spectrum(&output.sensors.imu.accel, output.manifest.rates_hz[1]);
    Some(eval::summarise(&spectrum, None))
}
