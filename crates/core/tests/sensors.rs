//! Sensor models: magnitudes, consistency with the truth and determinism.

mod fixtures;

use glam::DVec2;

use ourealis_core::eval::{accel_vertical_spectrum, baro_altitude_spectrum, spectrum, summarise};
use ourealis_core::motion::{MotionConfig, Trajectory};
use ourealis_core::person::{PersonParams, Preset};
use ourealis_core::sensor::{self, SensorConfig};

use fixtures::{arc_path, flat_environment, straight_path};

#[allow(clippy::field_reassign_with_default)]
fn run(
    length_m: f64,
    person: PersonParams,
    config: SensorConfig,
) -> (Trajectory, sensor::SensorBundle) {
    let environment = flat_environment(&[]);
    let path = straight_path(length_m);
    let trajectory = Trajectory::build(
        path,
        &environment.terrain,
        &environment.hard,
        &environment.distance,
        &person,
        &MotionConfig {
            sample_rate_hz: config.imu_rate_hz,
            ..Default::default()
        },
        17,
        0,
    )
    .expect("trajectory");
    let bundle = sensor::generate(
        &trajectory,
        None,
        Some(&ourealis_map_format::tlv::value::MagneticField::default()),
        None,
        &config,
        &person,
        17,
        0,
    )
    .expect("sensors");
    (trajectory, bundle)
}

#[test]
fn streams_have_the_configured_rates_and_monotonic_time() {
    let person = PersonParams::preset(Preset::Moderate);
    let config = SensorConfig::default();
    let (trajectory, bundle) = run(150.0, person, config);
    let duration = trajectory.duration_s();

    let expected_gnss = (duration * config.gnss_rate_hz) as usize;
    assert!((bundle.sensors.gnss.len() as i64 - expected_gnss as i64).abs() <= 2);
    assert_eq!(bundle.sensors.imu.accel.len(), bundle.truth.len());
    assert_eq!(bundle.sensors.imu.gyro.len(), bundle.truth.len());

    for window in bundle.sensors.gnss.windows(2) {
        assert!(window[1].time_s > window[0].time_s);
    }
    for window in bundle.sensors.baro.windows(2) {
        assert!(window[1].time_s > window[0].time_s);
    }
    for window in bundle.sensors.mag.windows(2) {
        assert!(window[1].time_s > window[0].time_s);
    }
}

#[test]
fn same_seed_reproduces_every_sample() {
    let person = PersonParams::preset(Preset::Moderate);
    let (_, first) = run(120.0, person.clone(), SensorConfig::default());
    let (_, second) = run(120.0, person, SensorConfig::default());
    assert_eq!(first.sensors.gnss.len(), second.sensors.gnss.len());
    for (a, b) in first.sensors.gnss.iter().zip(second.sensors.gnss.iter()) {
        assert_eq!(a.x, b.x);
        assert_eq!(a.y, b.y);
        assert_eq!(a.speed_mps, b.speed_mps);
    }
    for (a, b) in first
        .sensors
        .imu
        .accel
        .iter()
        .zip(second.sensors.imu.accel.iter())
    {
        assert_eq!(a.z, b.z);
    }
}

#[test]
fn gnss_error_stays_in_the_configured_range() {
    let person = PersonParams::preset(Preset::Moderate);
    let config = SensorConfig::default();
    let (_, bundle) = run(200.0, person.clone(), config);
    let errors: Vec<f64> = bundle
        .sensors
        .gnss
        .iter()
        .map(|fix| (DVec2::new(fix.x, fix.y) - DVec2::new(fix.x, fix.y)).length())
        .collect();
    let _ = errors;

    // Compare against the truth at the same times.
    let mut deviations = Vec::new();
    for fix in &bundle.sensors.gnss {
        let Some(state) = ourealis_core::sensor::state_at(&bundle.truth, fix.time_s) else {
            continue;
        };
        deviations.push((DVec2::new(fix.x, fix.y) - state.position).length());
    }
    let stats = ourealis_core::eval::DistributionStats::of(&deviations);
    let combined_sigma = (person.sensors.gnss_bias_sigma_m.powi(2)
        + person.sensors.gnss_white_sigma_m.powi(2))
    .sqrt();
    assert!(
        stats.mean > combined_sigma * 0.3 && stats.mean < combined_sigma * 2.5,
        "mean horizontal error {} is implausible for sigma {combined_sigma}",
        stats.mean
    );
    assert!(stats.max < combined_sigma * 8.0);
}

#[test]
fn gnss_velocity_is_correlated_with_position_when_requested() {
    let person = PersonParams::preset(Preset::Moderate);
    let config = SensorConfig::default();
    let (_, bundle) = run(400.0, person.clone(), config);

    // The slow position bias differentiates into the velocity, so the speed
    // error must show a positive lag-one autocorrelation.
    let errors: Vec<f64> = bundle
        .sensors
        .gnss
        .iter()
        .filter_map(|fix| {
            let state = ourealis_core::sensor::state_at(&bundle.truth, fix.time_s)?;
            Some(fix.speed_mps - state.speed)
        })
        .collect();
    let acf = ourealis_core::eval::autocorrelation(&errors, 3);
    assert!(
        acf.get(1).copied().unwrap_or(0.0) > 0.1,
        "correlated mode should leave a positive lag-one correlation: {acf:?}"
    );

    // With correlation disabled the same series is white.
    let mut uncorrelated = person;
    uncorrelated.sensors.gnss_correlated_velocity = false;
    let (_, bundle) = run(400.0, uncorrelated, config);
    let errors: Vec<f64> = bundle
        .sensors
        .gnss
        .iter()
        .filter_map(|fix| {
            let state = ourealis_core::sensor::state_at(&bundle.truth, fix.time_s)?;
            Some(fix.speed_mps - state.speed)
        })
        .collect();
    let acf = ourealis_core::eval::autocorrelation(&errors, 3);
    assert!(
        acf.get(1).copied().unwrap_or(0.0).abs() < 0.35,
        "independent velocity noise should not be strongly correlated: {acf:?}"
    );
}

#[test]
fn accelerometer_reads_gravity_while_standing() {
    let person = PersonParams::preset(Preset::Moderate);
    let config = SensorConfig::clean();
    let (trajectory, bundle) = run(60.0, person, config);

    let standing: Vec<&sensor::ImuAccelSample> = bundle
        .sensors
        .imu
        .accel
        .iter()
        .zip(bundle.truth.iter())
        .filter(|(_, state)| state.standing)
        .map(|(sample, _)| sample)
        .take(200)
        .collect();
    assert!(!standing.is_empty(), "the run must start standing still");
    let _ = trajectory;

    let mean_z = standing.iter().map(|sample| sample.z).sum::<f64>() / standing.len() as f64;
    assert!(
        (mean_z - 9.81).abs() < 0.3,
        "a device at rest must read about +9.81 m/s^2 on its vertical axis, got {mean_z}"
    );
    let magnitude = standing
        .iter()
        .map(|sample| sample.magnitude())
        .sum::<f64>()
        / standing.len() as f64;
    assert!((magnitude - 9.81).abs() < 0.4, "magnitude {magnitude}");
}

#[test]
fn accelerometer_shows_the_step_signature_with_the_locked_amplitude() {
    let person = PersonParams::preset(Preset::Race);
    let config = SensorConfig::clean();
    let (trajectory, bundle) = run(600.0, person, config);

    let spectrum = accel_vertical_spectrum(&bundle.sensors.imu.accel, config.imu_rate_hz);
    let summary = summarise(&spectrum, Some(trajectory.step_frequency));
    let peak = summary.step_peak.expect("a step peak must be present");
    assert!(
        (peak.frequency_hz - trajectory.step_frequency).abs() < 0.3,
        "peak at {} Hz, expected {} Hz",
        peak.frequency_hz,
        trajectory.step_frequency
    );

    // The fundamental must be within a factor of two of A_b (2 pi f)^2, which is
    // the consistency the design requires between bounce and accelerometer. The
    // prediction uses the nominal amplitude, which scales with pace, so the
    // comparison is made at the pace actually run.
    let pace_ratio = trajectory.bounce_amplitude_ratio();
    let predicted = ourealis_core::eval::predicted_fundamental(
        trajectory.bounce.amplitude_m,
        trajectory.step_frequency,
    ) * pace_ratio;
    let ratio = peak.magnitude / predicted;
    assert!(
        (0.4..2.5).contains(&ratio),
        "fundamental ratio {ratio} (measured {} predicted {predicted})",
        peak.magnitude
    );

    // A second harmonic must be present: ground contact is asymmetric.
    let second = spectrum.magnitude_at(2.0 * trajectory.step_frequency);
    assert!(
        second > peak.magnitude * 0.05,
        "the second harmonic ({second}) should be visible next to the fundamental ({})",
        peak.magnitude
    );
}

#[test]
fn gyroscope_tracks_the_yaw_rate_of_the_truth_heading() {
    // The design's consistency requirement is that the gyroscope and the
    // trajectory agree, so the finite difference of the truth heading is the
    // reference, not a closed-form rate.
    let environment = flat_environment(&[]);
    let mut person = PersonParams::preset(Preset::Moderate);
    person.sensors.gyro_step_amplitude_rps = 0.0;
    // The rate relation is the subject here; leaving the instrument's own noise in
    // place would put a ceiling on the correlation that says nothing about it.
    person.sensors.gyro_white_sigma = 0.0;
    person.sensors.gyro_bias_sigma = 0.0;
    let path = arc_path(30.0, std::f64::consts::PI * 1.5);
    let trajectory = Trajectory::build(
        path,
        &environment.terrain,
        &environment.hard,
        &environment.distance,
        &person,
        &MotionConfig::default(),
        21,
        0,
    )
    .expect("trajectory");
    let bundle = sensor::generate(
        &trajectory,
        None,
        None,
        None,
        &SensorConfig::clean(),
        &person,
        21,
        0,
    )
    .expect("sensors");

    let mut measured = Vec::new();
    let mut expected = Vec::new();
    for (index, sample) in bundle.sensors.imu.gyro.iter().enumerate() {
        if index == 0 || index + 1 >= bundle.truth.len() {
            continue;
        }
        let state = &bundle.truth[index];
        if state.standing || state.turning || state.speed < 1.0 {
            continue;
        }
        let dt = state.time_s - bundle.truth[index - 1].time_s;
        if dt <= 0.0 {
            continue;
        }
        let rate =
            ourealis_core::math::angle_difference(state.heading, bundle.truth[index - 1].heading)
                / dt;
        measured.push(sample.z);
        expected.push(rate);
    }
    assert!(measured.len() > 100);
    // The step oscillation is held out of this run: it is the larger component of
    // the yaw rate on a gentle route, and what this test is about is the turn-rate
    // relation. Its own signature is checked by `tests/real_data.rs` against real
    // recordings.
    let correlation = pearson(&measured, &expected);
    assert!(
        correlation > 0.95,
        "gyroscope and truth heading must agree, correlation was {correlation}"
    );
}

#[test]
fn gyroscope_matches_v_kappa_eff_in_steady_state() {
    // The step oscillation is held out so the relation being checked is the only
    // thing in the signal; see `gyroscope_tracks_the_yaw_rate_of_the_truth_heading`.
    // With no lateral drift the trajectory is the path itself, and the design's
    // steady-state relation yaw rate = v * kappa_eff must hold closely.
    let environment = flat_environment(&[]);
    let mut person = PersonParams::preset(Preset::Moderate);
    person.sensors.gyro_step_amplitude_rps = 0.0;
    // The rate relation is the subject here; leaving the instrument's own noise in
    // place would put a ceiling on the correlation that says nothing about it.
    person.sensors.gyro_white_sigma = 0.0;
    person.sensors.gyro_bias_sigma = 0.0;
    let path = arc_path(30.0, std::f64::consts::PI * 1.5);
    let mut steady = MotionConfig {
        adapt_individual: false,
        ..Default::default()
    };
    steady.offset.std_m = 0.0;
    steady.offset.mean_m = 0.5;
    let trajectory = Trajectory::build(
        path,
        &environment.terrain,
        &environment.hard,
        &environment.distance,
        &person,
        &steady,
        21,
        0,
    )
    .expect("trajectory");
    let bundle = sensor::generate(
        &trajectory,
        None,
        None,
        None,
        &SensorConfig::clean(),
        &person,
        21,
        0,
    )
    .expect("sensors");

    let pairs: Vec<(f64, f64)> = bundle
        .sensors
        .imu
        .gyro
        .iter()
        .zip(bundle.truth.iter())
        .filter(|(_, state)| !state.standing && !state.turning && state.speed > 1.0)
        .map(|(sample, state)| (sample.z, state.speed * state.kappa_eff))
        .collect();
    assert!(pairs.len() > 100);

    let target: Vec<f64> = pairs.iter().map(|(_, expected)| *expected).collect();
    // As above, the step oscillation is held out: this test is the turn-rate
    // relation, not the gait signature.
    let measured: Vec<f64> = pairs.iter().map(|(value, _)| *value).collect();
    // The design's relation is a steady-state one: the heading is low-passed, so
    // the gyroscope lags the instantaneous v * kappa_eff slightly. The magnitude
    // must match closely and the shapes must track.
    let mean_measured = measured.iter().sum::<f64>() / measured.len() as f64;
    let mean_target = target.iter().sum::<f64>() / target.len() as f64;
    let ratio = mean_measured / mean_target;
    assert!(
        (0.8..1.2).contains(&ratio),
        "steady-state yaw rate ratio {ratio} (measured {mean_measured}, expected {mean_target})"
    );
    let correlation = pearson(&measured, &target);
    assert!(
        correlation > 0.7,
        "yaw rate must track v * kappa_eff, correlation was {correlation}"
    );
}

#[test]
fn gyroscope_reports_the_turn_maneuver_instead_of_zero() {
    // A reversal forces an on-the-spot turn where v = 0; the gyroscope must still
    // show the rotation, which a v * kappa model could not produce.
    let environment = flat_environment(&[]);
    let person = PersonParams::preset(Preset::Moderate);
    let points = vec![
        DVec2::new(20.0, 40.0),
        DVec2::new(80.0, 40.0),
        DVec2::new(20.0, 40.0),
    ];
    let path = ourealis_core::path::Path::resampled(points, 1.0).expect("path");
    let trajectory = Trajectory::build(
        path,
        &environment.terrain,
        &environment.hard,
        &environment.distance,
        &person,
        &MotionConfig::default(),
        3,
        0,
    )
    .expect("trajectory");
    let turning_samples = trajectory.samples.iter().filter(|s| s.turning).count();
    assert!(
        turning_samples > 0,
        "the reversal must schedule a turn maneuver"
    );

    let bundle = sensor::generate(
        &trajectory,
        None,
        None,
        None,
        &SensorConfig::clean(),
        &person,
        3,
        0,
    )
    .expect("sensors");
    let peak = bundle
        .sensors
        .imu
        .gyro
        .iter()
        .zip(bundle.truth.iter())
        .filter(|(_, state)| state.turning)
        .map(|(sample, _)| sample.z.abs())
        .fold(0.0f64, f64::max);
    assert!(
        peak > 0.5,
        "the turn must appear in the yaw rate, peak {peak}"
    );
    assert!(
        peak <= person.turn_omega_max + 0.2,
        "peak yaw rate {peak} exceeds the maneuver limit {}",
        person.turn_omega_max
    );
}

#[test]
fn magnetometer_magnitude_matches_the_earth_field() {
    let person = PersonParams::preset(Preset::Moderate);
    let mut config = SensorConfig::clean();
    config.magnetic_disturbance_enabled = false;
    let (_, bundle) = run(120.0, person, config);
    let field = ourealis_map_format::tlv::value::MagneticField::default();
    for sample in &bundle.sensors.mag {
        assert!(
            (sample.magnitude() - field.strength_ut as f64).abs() < 3.0,
            "field magnitude {} should be near {} uT",
            sample.magnitude(),
            field.strength_ut
        );
    }
}

#[test]
fn barometer_altitude_follows_the_truth_and_carries_the_step_ripple() {
    let person = PersonParams::preset(Preset::Moderate);
    let baro_sigma_pa = person.sensors.baro_white_sigma_pa;
    let config = SensorConfig::clean();
    let (trajectory, bundle) = run(300.0, person, config);

    // The error is white noise of sigma = `baro_white_sigma_pa` (2 Pa, about
    // 0.17 m), so the bound is statistical rather than per-sample: a fixed 3-sigma
    // per-sample limit would fail on the tail of a 7500-sample run about half the
    // time. What must hold is that the error is centred and has the right scale,
    // and that no sample is wildly out.
    let mut errors = Vec::new();
    for sample in &bundle.sensors.baro {
        let Some(state) = ourealis_core::sensor::state_at(&bundle.truth, sample.time_s) else {
            continue;
        };
        errors.push(sample.altitude_m - state.z);
        let expected_pressure = config.reference_pressure_pa
            * (-state.z / ourealis_core::math::ATMOSPHERE_SCALE_HEIGHT_M).exp();
        assert!((sample.pressure_pa - expected_pressure).abs() < 80.0);
    }
    let count = errors.len() as f64;
    let mean = errors.iter().sum::<f64>() / count;
    let rms = (errors.iter().map(|e| e * e).sum::<f64>() / count).sqrt();
    let worst = errors.iter().fold(0.0f64, |acc, e| acc.max(e.abs()));
    let expected_sigma_m = baro_sigma_pa / config.reference_pressure_pa
        * ourealis_core::math::ATMOSPHERE_SCALE_HEIGHT_M;
    assert!(mean.abs() < expected_sigma_m, "barometric bias {mean} m");
    assert!(
        (rms / expected_sigma_m - 1.0).abs() < 0.25,
        "barometric error rms {rms} m should be near {expected_sigma_m} m"
    );
    assert!(
        worst < expected_sigma_m * 6.0,
        "barometric error {worst} m is far outside the instrument's noise"
    );

    // A consumer barometer resolves the bounce, so the ripple must be present at
    // the step frequency.
    let spectrum = baro_altitude_spectrum(&bundle.sensors.baro, config.baro_rate_hz);
    let summary = summarise(&spectrum, Some(trajectory.step_frequency));
    assert!(
        summary.step_peak.is_some(),
        "the bounce must leave a spectral line at the step frequency"
    );
    let ripple = ourealis_core::eval::baro_bounce_amplitude_m(&bundle.sensors.baro);
    assert!(
        ripple > 0.005,
        "ripple amplitude {ripple} m is too small to be realistic"
    );
}

#[test]
fn multipath_events_only_fire_inside_regions() {
    // Without regions there can be no multipath bias, so the GNSS error must be
    // confined to the configured noise.
    let person = PersonParams::preset(Preset::Moderate);
    let config = SensorConfig::default();
    let (_, bundle) = run(200.0, person, config);
    assert_eq!(bundle.sensors.multipath_events, 0);
    let max_error = bundle
        .sensors
        .gnss
        .iter()
        .filter_map(|fix| {
            let state = ourealis_core::sensor::state_at(&bundle.truth, fix.time_s)?;
            Some((DVec2::new(fix.x, fix.y) - state.position).length())
        })
        .fold(0.0f64, f64::max);
    assert!(
        max_error < 40.0,
        "without regions the error must stay within the noise budget, saw {max_error}"
    );
}

/// Pearson correlation of two equal-length series.
fn pearson(a: &[f64], b: &[f64]) -> f64 {
    let n = a.len().min(b.len());
    if n < 3 {
        return 0.0;
    }
    let mean_a = a[..n].iter().sum::<f64>() / n as f64;
    let mean_b = b[..n].iter().sum::<f64>() / n as f64;
    let mut covariance = 0.0;
    let mut var_a = 0.0;
    let mut var_b = 0.0;
    for index in 0..n {
        let da = a[index] - mean_a;
        let db = b[index] - mean_b;
        covariance += da * db;
        var_a += da * da;
        var_b += db * db;
    }
    if var_a <= 1e-12 || var_b <= 1e-12 {
        0.0
    } else {
        covariance / (var_a.sqrt() * var_b.sqrt())
    }
}

#[test]
fn spectrum_summary_reports_harmonic_ratios() {
    let person = PersonParams::preset(Preset::Moderate);
    let (trajectory, bundle) = run(400.0, person, SensorConfig::clean());
    let spectrum = accel_vertical_spectrum(&bundle.sensors.imu.accel, 100.0);
    let summary = summarise(&spectrum, Some(trajectory.step_frequency));
    assert_eq!(summary.harmonic_ratios.len(), 2);
    assert!(summary.dominant.is_some());
    assert!(spectrum::predicted_fundamental(0.05, 2.7) > 0.0);
}

#[test]
fn truth_acceleration_matches_the_derivative_of_the_speed_profile() {
    // The acceleration every inertial sensor is built on is a second difference
    // of the low-frequency position over a *non-uniform* time grid: the profile is
    // spaced in arc length, so a runner accelerating from rest passes a sample
    // every few milliseconds. Two things have to be right for it to mean anything,
    // and neither shows up in a spectrum: the denominator of the difference (the
    // spacing between the two velocity estimates, not the full span), and the ends
    // (where a one-sided difference over a zero-length interval reads the runner's
    // own speed as an acceleration).
    let person = PersonParams::preset(Preset::Moderate);
    let (_trajectory, bundle) = run(200.0, person, SensorConfig::clean());
    let states = &bundle.truth;
    assert!(states.len() > 500);

    // On this straight, flat route the longitudinal acceleration of the truth must
    // follow the profile's own speed: it is a different computation from the
    // second difference, so agreement is evidence rather than a tautology. The last
    // few samples of the stop are excluded — the runner covers millimetres there,
    // and the discretisation of the final step is a separate question from whether
    // the second difference is scaled correctly (and twenty times too large is what
    // it was).
    let mut worst = 0.0f64;
    let mut measured = 0.0f64;
    let mut stop_peak = 0.0f64;
    for index in 1..states.len() - 1 {
        if states[index].standing || states[index].turning {
            continue;
        }
        let dt = states[index + 1].time_s - states[index - 1].time_s;
        let from_speed = (states[index + 1].speed - states[index - 1].speed) / dt;
        let heading = states[index].heading;
        let along = glam::DVec2::new(states[index].acceleration[0], states[index].acceleration[1])
            .dot(glam::DVec2::new(heading.cos(), heading.sin()));
        if states[index].speed < 0.5 {
            stop_peak = stop_peak.max(along.abs());
            continue;
        }
        measured = measured.max(along.abs());
        worst = worst.max((along - from_speed).abs());
    }
    println!(
        "longitudinal acceleration: worst {measured:.2} m/s^2, residual {worst:.3},          stopping peak {stop_peak:.2}"
    );
    assert!(measured > 0.3, "the run must contain real acceleration");
    assert!(
        worst < 0.35,
        "the truth acceleration disagrees with the speed derivative by {worst:.3} m/s^2"
    );
    assert!(
        stop_peak < 6.0,
        "stopping asks for {stop_peak:.1} m/s^2, which is not a deceleration"
    );

    // The ends carry no spurious spike. A one-sided fallback over a zero-length
    // interval reports `v / dt` there, which at 100 Hz is hundreds of m/s^2.
    let first = states[0].acceleration;
    let last = states[states.len() - 1].acceleration;
    let magnitude =
        |value: [f64; 3]| (value[0] * value[0] + value[1] * value[1] + value[2] * value[2]).sqrt();
    println!(
        "boundary accelerations: first {:.3}, last {:.3}",
        magnitude(first),
        magnitude(last)
    );
    assert!(
        magnitude(first) < 5.0 && magnitude(last) < 5.0,
        "the end samples must not invent an acceleration"
    );
}

#[test]
fn accelerometer_step_signature_is_in_phase_with_the_bounce() {
    // The design's phase-lock rule: the accelerometer's step harmonic and the
    // bounce are one and the same motion, so a downstream height estimator that
    // integrates acceleration twice must recover the barometric altitude. Getting
    // the sign of the fundamental wrong leaves both streams individually plausible
    // and the relation inverted, which is exactly what a spectral magnitude
    // comparison cannot see.
    let mut person = PersonParams::preset(Preset::Moderate);
    // 2 Pa of barometric noise is 0.17 m of altitude — four times the bounce it
    // carries — so the instrument's own noise is held out and the phase relation
    // between the two streams is what remains.
    person.sensors.baro_white_sigma_pa = 0.0;
    let (trajectory, bundle) = run(200.0, person, SensorConfig::clean());

    // Vertical acceleration in the body frame against the bounce height of the
    // same run. For a sinusoid `z = A sin(w t)` the acceleration is `-A w^2 sin`,
    // so the correlation is -1; anything near +1 means the sign is inverted.
    let count = bundle
        .sensors
        .imu
        .accel
        .len()
        .min(bundle.truth.len())
        .min(trajectory.samples.len());
    let mut acceleration = Vec::with_capacity(count);
    let mut height = Vec::with_capacity(count);
    for index in 0..count {
        let state = &bundle.truth[index];
        if state.standing || state.turning {
            continue;
        }
        acceleration.push(bundle.sensors.imu.accel[index].z);
        height.push(trajectory.samples[index].bounce_z);
    }
    assert!(acceleration.len() > 500);
    let correlation = pearson(&acceleration, &height);
    println!("acceleration/bounce correlation {correlation:.3}");
    assert!(
        correlation < -0.5,
        "the vertical acceleration must be anti-phase with the bounce height,          not {correlation:.3}"
    );

    // The same relation against the barometer, which carries the bounce in its
    // altitude: again anti-phase. The barometer runs at its own rate, so each of
    // its samples is paired with the *mean* of the accelerometer over the interval
    // it covers — pairing by index instead would compare two different instants and
    // wash the correlation out.
    let baro = &bundle.sensors.baro;
    let accel = &bundle.sensors.imu.accel;
    let mut a = Vec::with_capacity(baro.len());
    let mut b = Vec::with_capacity(baro.len());
    for (index, sample) in baro.iter().enumerate() {
        let start = index.saturating_sub(1) * accel.len() / baro.len();
        let end = ((index + 1) * accel.len() / baro.len())
            .max(start + 1)
            .min(accel.len());
        let window = &accel[start..end];
        if window.is_empty() {
            continue;
        }
        a.push(window.iter().map(|value| value.z).sum::<f64>() / window.len() as f64);
        b.push(sample.altitude_m);
    }
    let mean = b.iter().sum::<f64>() / b.len() as f64;
    b.iter_mut().for_each(|value| *value -= mean);
    let baro_correlation = pearson(&a, &b);
    println!("acceleration/barometer correlation {baro_correlation:.3}");
    assert!(
        baro_correlation < -0.5,
        "the vertical acceleration must be anti-phase with the barometric altitude,          not {baro_correlation:.3}"
    );
}
