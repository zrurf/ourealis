//! End-to-end simulator behaviour: invariants, determinism, batch and export.

mod fixtures;

use glam::DVec2;

use ourealis_core::person::{PersonParams, PersonSampler, Preset};
use ourealis_core::plan::{Checkpoint, LoopRequest, StandardRequest, ViaSemantics, Waypoint};
use ourealis_core::sim::{BatchRunner, MapSource, SimulationConfig, Simulator};
use ourealis_map_format::synthetic::SyntheticMapSpec;

fn map() -> MapSource {
    MapSource::synthetic(SyntheticMapSpec::compact())
}

fn runner(start: DVec2, goal: DVec2) -> Simulator {
    Simulator::builder()
        .map(map())
        .person(PersonParams::preset(Preset::Moderate))
        .standard(StandardRequest::new(start, goal))
        .config(SimulationConfig::deterministic())
        .seed(31)
        .build()
        .expect("simulator")
}

#[test]
fn end_to_end_run_produces_consistent_streams() {
    let output = runner(DVec2::new(30.0, 100.0), DVec2::new(260.0, 100.0))
        .run()
        .expect("run");

    assert!(output.route.length_m > 100.0);
    assert!(output.duration_s() > 30.0);
    assert_eq!(output.manifest.mode, "standard");
    assert!(output.manifest.map_name.is_some());

    // The truth sequence is strictly ordered in time and entirely on passable
    // ground.
    let environment = runner(DVec2::new(30.0, 100.0), DVec2::new(260.0, 100.0))
        .environment()
        .expect("environment");
    for window in output.truth.windows(2) {
        assert!(window[1].time_s > window[0].time_s);
    }
    for state in &output.truth {
        assert!(
            environment.hard.is_passable(state.position),
            "the trajectory left passable ground at {:?}",
            state.position
        );
    }

    // Every sensor stream exists and shares the time base.
    assert!(!output.sensors.gnss.is_empty());
    assert!(!output.sensors.imu.accel.is_empty());
    assert!(!output.sensors.baro.is_empty());
    assert_eq!(output.sensors.imu.accel.len(), output.truth.len());
    assert!(output.sensors.gnss_availability() > 0.9);

    // The evaluation report passes its plausibility checks.
    let metrics = output.metrics.as_ref().expect("metrics");
    assert!(metrics.passes_plausibility(), "{metrics:?}");
    assert!(metrics.speed.mean > 1.0);
    assert!(metrics.path_ratio >= 1.0);
}

#[test]
fn identical_seeds_produce_identical_runs() {
    let first = runner(DVec2::new(40.0, 60.0), DVec2::new(240.0, 140.0))
        .run()
        .expect("first");
    let second = runner(DVec2::new(40.0, 60.0), DVec2::new(240.0, 140.0))
        .run()
        .expect("second");

    assert_eq!(first.route.points.len(), second.route.points.len());
    assert_eq!(first.truth.len(), second.truth.len());
    for (a, b) in first.truth.iter().zip(second.truth.iter()) {
        assert_eq!(a.position.x, b.position.x);
        assert_eq!(a.position.y, b.position.y);
        assert_eq!(a.z, b.z);
        assert_eq!(a.speed, b.speed);
    }
    for (a, b) in first.sensors.gnss.iter().zip(second.sensors.gnss.iter()) {
        assert_eq!(a.x, b.x);
        assert_eq!(a.speed_mps, b.speed_mps);
    }
}

#[test]
fn waypoint_semantics_change_the_speed_profile() {
    let base = StandardRequest::new(DVec2::new(30.0, 100.0), DVec2::new(260.0, 100.0));
    let direct = Simulator::builder()
        .map(map())
        .person(PersonParams::preset(Preset::Moderate))
        .standard(base.clone())
        .config(SimulationConfig::deterministic())
        .seed(5)
        .build()
        .expect("build")
        .run()
        .expect("run");

    let with_dwell = base.clone().via(
        Waypoint::new(DVec2::new(150.0, 100.0))
            .with_semantics(ViaSemantics::Dwell { duration_s: 15.0 }),
    );
    let dwelled = Simulator::builder()
        .map(map())
        .person(PersonParams::preset(Preset::Moderate))
        .standard(with_dwell)
        .config(SimulationConfig::deterministic())
        .seed(5)
        .build()
        .expect("build")
        .run()
        .expect("run");

    // The two runs take different routes — every leg is a Logit draw — so the
    // dwell is measured where it is unambiguous: the extra time spent standing,
    // with the run being longer overall as the weaker check.
    let standing_s = |output: &ourealis_core::sim::SimulationOutput| {
        output
            .trajectory
            .samples
            .iter()
            .filter(|sample| sample.standing)
            .count() as f64
            / 100.0
    };
    let extra_standing = standing_s(&dwelled) - standing_s(&direct);
    assert!(
        extra_standing > 14.0,
        "a dwell of 15 s must be held: {extra_standing} s of extra standstill"
    );
    assert!(
        dwelled.duration_s() > direct.duration_s(),
        "a dwell of 15 s must lengthen the run: {} vs {}",
        dwelled.duration_s(),
        direct.duration_s()
    );
    // Standing still has to be what it says: the ground truth does not move while
    // the flag is set, which is what the barometer and the GNSS drift cloud are
    // derived from.
    let mut worst_drift: f64 = 0.0;
    for window in dwelled.trajectory.samples.windows(2) {
        if window[0].standing && window[1].standing {
            worst_drift = worst_drift.max((window[1].position - window[0].position).length());
        }
    }
    assert!(
        worst_drift < 1e-9,
        "the runner drifted {worst_drift} m while standing"
    );

    let with_slow = base.via(
        Waypoint::new(DVec2::new(150.0, 100.0))
            .with_semantics(ViaSemantics::Slow)
            .with_radius(8.0),
    );
    let slowed = Simulator::builder()
        .map(map())
        .person(PersonParams::preset(Preset::Moderate))
        .standard(with_slow)
        .config(SimulationConfig::deterministic())
        .seed(5)
        .build()
        .expect("build")
        .run()
        .expect("run");

    // `slow` is a dip in the speed limit at the waypoint, so it is checked as a
    // dip: the runner is slower passing it than it is on the approach and on the
    // way out. Comparing total durations would compare two different routes,
    // because each leg is its own Logit draw.
    let waypoint = DVec2::new(150.0, 100.0);
    let samples = &slowed.trajectory.samples;
    let closest = samples
        .iter()
        .enumerate()
        .min_by(|a, b| {
            (a.1.position - waypoint)
                .length()
                .partial_cmp(&(b.1.position - waypoint).length())
                .expect("finite distances")
        })
        .map(|(index, _)| index)
        .expect("the run has samples");
    let arc = samples[closest].arc_s;
    let window = samples
        .iter()
        .filter(|sample| (sample.arc_s - arc).abs() <= 8.0)
        .map(|sample| sample.speed)
        .fold(f64::INFINITY, f64::min);
    let mean_over = |from: f64, to: f64| {
        let selected: Vec<f64> = samples
            .iter()
            .filter(|sample| {
                let offset = sample.arc_s - arc;
                offset >= from && offset <= to
            })
            .map(|sample| sample.speed)
            .collect();
        selected.iter().sum::<f64>() / selected.len().max(1) as f64
    };
    let neighbouring = mean_over(-30.0, -15.0).min(mean_over(15.0, 30.0));
    assert!(
        window < 0.85 * neighbouring,
        "the waypoint must slow the runner: {window:.2} m/s at it against {neighbouring:.2} m/s around it"
    );
}

#[test]
fn loop_mode_returns_to_the_start_without_a_seam() {
    let output = Simulator::builder()
        .map(map())
        .person(PersonParams::preset(Preset::Moderate))
        .looped(LoopRequest::new(DVec2::new(60.0, 100.0), 1))
        .config(SimulationConfig::deterministic())
        .seed(11)
        .build()
        .expect("build")
        .run()
        .expect("run");

    assert_eq!(output.manifest.mode, "loop");
    let start = output.route.points.first().copied().expect("start");
    let end = output.route.points.last().copied().expect("end");
    let gap = ((end[0] - start[0]).powi(2) + (end[1] - start[1]).powi(2)).sqrt();
    assert!(gap < 2.0, "the loop must close, gap was {gap} m");

    // Lap continuity: the speed must not drop to a standstill at the seam, which
    // is what a zero-speed boundary condition would produce.
    let moving_at_end = output
        .trajectory
        .samples
        .iter()
        .rev()
        .take(30)
        .filter(|sample| sample.speed > 0.5)
        .count();
    assert!(moving_at_end > 20, "the runner must run through the seam");
    assert!(!output.trajectory.ends_standing());
}

#[test]
fn loop_smoothing_uses_the_callers_configuration() {
    // The closed-band pass used to run with `ElasticBandConfig::default()` no
    // matter what the caller configured. Disabling the per-half pass isolates it:
    // with the halves left alone, only the closed-band iteration count can make
    // the two runs differ.
    let run = |iterations: usize| {
        let mut config = SimulationConfig::deterministic();
        config.loop_route.route.smooth = false;
        config.loop_route.route.smoothing.iterations = iterations;
        Simulator::builder()
            .map(map())
            .person(PersonParams::preset(Preset::Moderate))
            .looped(LoopRequest::new(DVec2::new(60.0, 100.0), 1))
            .config(config)
            .seed(11)
            .build()
            .expect("build")
            .run()
            .expect("run")
    };

    let brief = run(1);
    let configured = run(30);
    assert_ne!(
        brief.route.points, configured.route.points,
        "the closed-band pass must use the caller's smoothing configuration"
    );
}

#[test]
fn dynamic_checkpoint_redirects_without_a_discontinuity() {
    let request = StandardRequest::new(DVec2::new(30.0, 100.0), DVec2::new(260.0, 100.0));
    let checkpoint = Checkpoint {
        issued_at_s: 40.0,
        position: DVec2::new(150.0, 30.0),
    };
    let output = Simulator::builder()
        .map(map())
        .person(PersonParams::preset(Preset::Moderate))
        .dynamic(request, vec![checkpoint])
        .config(SimulationConfig::deterministic())
        .seed(13)
        .build()
        .expect("build")
        .run_dynamic()
        .expect("run");

    // The endpoint must now be near the checkpoint rather than the original goal.
    let end = output.trajectory.samples.last().expect("last").position;
    assert!(
        (end - checkpoint.position).length() < 30.0,
        "the run should end near the checkpoint, it ended {end:?}"
    );

    // Continuity: the speed implied by consecutive positions must stay within the
    // acceleration budget, which is what the smoothstep blend guarantees.
    let dt = 1.0 / output.manifest.rates_hz[1];
    for (index, window) in output.trajectory.samples.windows(2).enumerate() {
        let implied = (window[1].position - window[0].position).length() / dt;
        assert!(
            implied < 12.0,
            "positional jump of {implied} m/s between samples {index} and {} \
             (t = {} -> {}) indicates a discontinuity",
            index + 1,
            window[0].time_s,
            window[1].time_s
        );
    }
}

#[test]
fn batch_runs_are_parallel_safe_and_reproducible() {
    let simulator = Simulator::builder()
        .map(map())
        .standard(StandardRequest::new(
            DVec2::new(40.0, 60.0),
            DVec2::new(250.0, 150.0),
        ))
        .config(SimulationConfig::deterministic())
        .seed(19)
        .build()
        .expect("build");
    let people = PersonSampler::preset(Preset::Moderate)
        .sample_population(19, 6)
        .expect("population");

    let batch = BatchRunner::new(simulator.clone());
    let first = batch.run(&people).expect("batch");
    let second = batch.run(&people).expect("batch");
    assert_eq!(first.len(), people.len());

    // Rayon distributes the batch; the streams are keyed by individual index, so
    // two runs must agree exactly.
    for (a, b) in first.iter().zip(second.iter()) {
        assert_eq!(a.truth.len(), b.truth.len());
        for (x, y) in a.truth.iter().zip(b.truth.iter()) {
            assert_eq!(x.position.x, y.position.x);
            assert_eq!(x.speed, y.speed);
        }
    }

    // Individuals differ from one another: the population is not one runner
    // repeated.
    let speeds: Vec<f64> = first
        .iter()
        .map(|output| output.metrics.as_ref().map(|m| m.speed.mean).unwrap_or(0.0))
        .collect();
    let spread = speeds.iter().cloned().fold(f64::MIN, f64::max)
        - speeds.iter().cloned().fold(f64::MAX, f64::min);
    assert!(
        spread > 0.05,
        "the population must vary, spread was {spread}"
    );

    // Path-choice frequencies are available for the batch.
    let frequencies = batch.choice_frequencies(&first);
    assert!(!frequencies.is_empty());
    let total: f64 = frequencies.iter().sum();
    assert!((total - 1.0).abs() < 1e-9);
}

#[test]
fn outputs_export_to_json_csv_and_geojson() {
    let output = runner(DVec2::new(40.0, 60.0), DVec2::new(200.0, 120.0))
        .run()
        .expect("run");
    let directory = std::env::temp_dir().join("ourealis-export-test");
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).expect("temp directory");

    let json_path = directory.join("run.json");
    ourealis_core::sim::export::write_json(&output, &json_path).expect("json");
    let text = std::fs::read_to_string(&json_path).expect("read json");
    assert!(text.contains("\"manifest\""));
    assert!(text.contains("\"gnss\""));

    ourealis_core::sim::export::write_csv_dir(&output, &directory).expect("csv");
    for name in [
        "truth.csv",
        "gnss.csv",
        "accel.csv",
        "gyro.csv",
        "mag.csv",
        "baro.csv",
    ] {
        let path = directory.join(name);
        let contents = std::fs::read_to_string(&path).expect("csv file");
        let lines = contents.lines().count();
        assert!(lines > 2, "{name} has only {lines} lines");
    }

    let geojson_path = directory.join("track.geojson");
    ourealis_core::sim::export::write_geojson(&output, None, &geojson_path).expect("geojson");
    let geojson = std::fs::read_to_string(&geojson_path).expect("read geojson");
    assert!(geojson.contains("LineString"));

    let summary = output.summary_json();
    assert!(summary.contains("sample_counts"));

    // The exported columns must be the quantities their headers name. Two of them
    // were not: `offset_m` carried the position-jitter magnitude, which is exactly
    // zero in a deterministic run, and `grade` carried the vertical acceleration.
    let truth = std::fs::read_to_string(directory.join("truth.csv")).expect("truth csv");
    let header: Vec<&str> = truth.lines().next().expect("header").split(',').collect();
    let offset_column = header
        .iter()
        .position(|name| *name == "offset_m")
        .expect("offset");
    let grade_column = header
        .iter()
        .position(|name| *name == "grade")
        .expect("grade");
    let mut offset_mismatch = 0.0f64;
    let mut grade_mismatch = 0.0f64;
    let mut offset_seen = 0.0f64;
    for (line, sample) in truth.lines().skip(1).zip(output.trajectory.samples.iter()) {
        let fields: Vec<&str> = line.split(',').collect();
        let offset: f64 = fields[offset_column].parse().expect("offset value");
        let grade: f64 = fields[grade_column].parse().expect("grade value");
        offset_mismatch = offset_mismatch.max((offset - sample.offset_m).abs());
        grade_mismatch = grade_mismatch.max((grade - sample.grade).abs());
        offset_seen = offset_seen.max(offset.abs());
    }
    assert!(offset_seen > 0.05, "the run must carry a lateral offset");
    assert!(
        offset_mismatch < 1e-3,
        "the exported offset differs from the trajectory by {offset_mismatch:.4} m"
    );
    assert!(
        grade_mismatch < 1e-4,
        "the exported grade differs from the trajectory by {grade_mismatch:.6}"
    );

    let _ = std::fs::remove_dir_all(&directory);
}

#[test]
fn a_multi_lap_session_ends_at_a_standstill() {
    // One lap is a closed circuit and takes the periodic profile boundary. Several
    // laps are not a circuit that happens to be long: the session starts from rest
    // and ends at rest, and the periodic boundary leaves the runner at racing speed
    // on the last sample — no deceleration into the finish, and the truth
    // differentiation reads that speed into the accelerometer.
    let output = Simulator::builder()
        .map(MapSource::synthetic(SyntheticMapSpec::compact()))
        .person(PersonParams::preset(Preset::Moderate))
        .looped(LoopRequest::new(DVec2::new(120.0, 160.0), 3))
        .config(SimulationConfig::deterministic())
        .seed(3)
        .build()
        .expect("simulator")
        .run()
        .expect("run");

    assert_eq!(output.trajectory.laps, 3);
    let last = output.trajectory.samples.last().expect("a sample");
    println!(
        "final sample: speed {:.3} m/s, standing {}",
        last.speed, last.standing
    );
    assert!(
        last.speed < 0.05,
        "the session must decelerate into the finish, not end at {:.2} m/s",
        last.speed
    );
    assert!(output.trajectory.ends_standing());
    // And the last inertial sample must not carry the speed as an acceleration: a
    // run that ends while moving reads `v / dt` there, which at jogging speed and
    // 100 Hz is three hundred metres per second squared. The bound is loose because
    // the last two samples of a stop are a discretisation of the final ramp, not
    // because the artefact is expected to be there.
    let final_acceleration = output
        .truth
        .last()
        .map(|state| state.acceleration)
        .unwrap_or([0.0; 3]);
    let magnitude = (final_acceleration[0].powi(2)
        + final_acceleration[1].powi(2)
        + final_acceleration[2].powi(2))
    .sqrt();
    assert!(
        magnitude < 10.0,
        "the final truth sample reports {magnitude:.2} m/s^2 at a standstill"
    );
}

#[test]
fn the_manifest_names_the_backend_that_ran() {
    // `Backend::Auto` falls back to the CPU without failing, so the configured
    // policy alone cannot say what produced a run. The manifest records what ran.
    let output = runner(DVec2::new(40.0, 60.0), DVec2::new(200.0, 120.0))
        .run()
        .expect("run");
    println!("manifest backend: {}", output.manifest.backend);
    assert!(
        !output.manifest.backend.is_empty(),
        "the backend must be recorded"
    );
    if output.manifest.backend.starts_with("wgpu:") {
        assert!(
            output.manifest.backend.len() > "wgpu:".len(),
            "adapter name"
        );
    } else {
        assert_eq!(output.manifest.backend, "cpu-rayon");
    }
}

#[test]
fn metrics_report_the_documented_quantities() {
    let output = runner(DVec2::new(40.0, 60.0), DVec2::new(220.0, 130.0))
        .run()
        .expect("run");
    let metrics = output.metrics.as_ref().expect("metrics");

    assert!(metrics.length_m > 0.0);
    assert!(metrics.duration_s > 0.0);
    assert!(metrics.speed.count > 0);
    assert_eq!(metrics.speed_acf.len(), 101);
    assert_eq!(metrics.position_acf.len(), 101);
    assert!(metrics.turn_rate.count > 0);
    assert!(metrics.gnss.is_some());
    // The accelerometer and barometer spectra must have been analysed.
    assert!(metrics.accel_spectrum.is_some());
    assert!(metrics.baro_spectrum.is_some());
    assert!(metrics.baro_bounce_m > 0.0);
    assert!(metrics.mean_abs_curvature >= 0.0);
    assert!(metrics.to_json().contains("path_ratio"));
}

#[test]
fn a_distant_goal_is_rejected_rather_than_guessed() {
    let simulator = runner(DVec2::new(30.0, 100.0), DVec2::new(5000.0, 5000.0));
    let result = simulator.run();
    assert!(
        result.is_err(),
        "a goal far outside the map must be rejected"
    );
}

#[test]
fn person_parameters_are_validated_before_the_run() {
    let mut person = PersonParams::preset(Preset::Moderate);
    person.target_speed = -1.0;
    let result = Simulator::builder()
        .map(map())
        .person(person)
        .standard(StandardRequest::new(
            DVec2::new(30.0, 100.0),
            DVec2::new(200.0, 100.0),
        ))
        .build();
    assert!(
        result.is_err(),
        "an invalid individual must be rejected at build time"
    );
}
