//! Randomized end-to-end sweep.
//!
//! The other suites pin one route and one individual per test, which is what
//! makes their bounds tight. This suite does the opposite: it draws the
//! individual, the origin/destination pair, the sensor configuration and the
//! planning mode from a stream seeded with a fixed constant, and checks that the
//! same invariants and the same realism bands hold for every draw.
//!
//! The case list is reproducible from [`CASE_SEED`], so a failure here names a
//! case that can be regenerated exactly. What varies across cases is what the
//! fixed-route tests cannot cover: that a fast individual on a hilly detour, a
//! slow one on a short straight, and a loop session all still produce
//! kinematically feasible, physically consistent, plausible output.

mod fixtures;

use glam::DVec2;

use ourealis_core::eval::MetricsReport;
use ourealis_core::field::{CostModelParams, CostWeights};
use ourealis_core::person::{PersonParams, PersonSampler, Preset};
use ourealis_core::plan::{LoopRequest, StandardRequest, Waypoint};
use ourealis_core::rng::Rng;
use ourealis_core::sim::{MapSource, SimulationConfig, SimulationOutput, Simulator};
use ourealis_map_format::Map;
use ourealis_map_format::synthetic::{self, SyntheticMapSpec};

/// Seed of the case list. Change it to explore; keep it fixed to reproduce.
const CASE_SEED: u64 = 0x5EED_1234;

/// Number of cases. Each one is a full simulation, so this trades runtime for
/// coverage; twelve cases run in a few tens of seconds in a debug build.
const CASES: usize = 12;

/// Seed of the individual sweep that runs several draws through one simulator.
const SWEEP_SEED: u64 = 0x5EED_5678;

fn presets() -> [Preset; 3] {
    [Preset::Jog, Preset::Moderate, Preset::Race]
}

/// One drawn case.
struct Case {
    index: usize,
    preset: Preset,
    person: PersonParams,
    start: DVec2,
    goal: DVec2,
    waypoints: Vec<Waypoint>,
    loop_laps: Option<usize>,
    seed: u64,
    individual: u32,
}

impl Case {
    /// Description used in assertion messages, so a failure names its draw.
    fn label(&self) -> String {
        format!(
            "case {} (preset {:?}, target {:.2} m/s, start ({:.0},{:.0}), goal ({:.0},{:.0}), \
             seed {}, individual {})",
            self.index,
            self.preset,
            self.person.target_speed,
            self.start.x,
            self.start.y,
            self.goal.x,
            self.goal.y,
            self.seed,
            self.individual
        )
    }
}

/// Builds the map image once; the cases reuse it instead of regenerating.
fn map_image() -> Vec<u8> {
    synthetic::build(&SyntheticMapSpec::compact()).expect("synthetic map")
}

/// Builds the environment once; it is the passability oracle for every case.
fn build_environment(image: &[u8]) -> ourealis_core::environment::Environment {
    let map = Map::from_bytes(image.to_vec()).expect("open");
    let dimension = map
        .feature_schema()
        .ok()
        .flatten()
        .map(|schema| schema.dim() as usize)
        .unwrap_or(1)
        .max(1);
    let weights = CostWeights::uniform(dimension);
    ourealis_core::environment::Environment::load(
        &map,
        &weights,
        &CostModelParams::default(),
        Default::default(),
    )
    .expect("environment")
}

fn passable(environment: &ourealis_core::environment::Environment, point: DVec2) -> bool {
    environment.hard.is_passable(point)
}

/// Draws the case list from a fixed seed.
fn draw_cases() -> Vec<Case> {
    let image = map_image();
    let environment = build_environment(&image);
    let map = Map::from_bytes(image.clone()).expect("open");
    let bounds = map.header().bounds;
    drop(map);

    let mut rng = Rng::from_seed(CASE_SEED);
    let mut cases = Vec::with_capacity(CASES);
    for index in 0..CASES {
        let preset = presets()[index % presets().len()];
        let sampler = PersonSampler::preset(preset);
        let individual = rng.uniform_range(0.0, 64.0) as u32;
        let person = sampler
            .sample(&mut rng, individual)
            .expect("population sample");

        // Endpoints are drawn from the interior and kept only when they are
        // passable with some clearance, so a case never fails on a goal that sits
        // inside a building.
        let draw_point = |rng: &mut Rng| -> DVec2 {
            let inset_x = bounds.width() * 0.06;
            let inset_y = bounds.height() * 0.06;
            DVec2::new(
                rng.uniform_range(bounds.min_x + inset_x, bounds.max_x - inset_x),
                rng.uniform_range(bounds.min_y + inset_y, bounds.max_y - inset_y),
            )
        };
        let start = (0..2000)
            .map(|_| draw_point(&mut rng))
            .find(|point| !environment.hard.is_forbidden(*point))
            .expect("a passable start exists");
        let goal = (0..2000)
            .map(|_| draw_point(&mut rng))
            .find(|point| {
                !environment.hard.is_forbidden(*point) && (*point - start).length() > 60.0
            })
            .expect("a passable goal exists");

        // A waypoint on some cases, to exercise the segment stitching and the
        // `slow` / `dwell` semantics on random geometry.
        let mut waypoints = Vec::new();
        if index % 3 == 0 {
            let semantics = match index % 6 {
                0 => ourealis_core::plan::ViaSemantics::Pass,
                3 => ourealis_core::plan::ViaSemantics::Slow,
                _ => ourealis_core::plan::ViaSemantics::Dwell { duration_s: 4.0 },
            };
            if let Some(point) = (0..2000)
                .map(|_| draw_point(&mut rng))
                .find(|point| !environment.hard.is_forbidden(*point))
            {
                let mut waypoint = Waypoint::new(point);
                waypoint.semantics = semantics;
                waypoints.push(waypoint);
            }
        }

        let loop_laps = if index % 4 == 1 { Some(2) } else { None };
        cases.push(Case {
            index,
            preset,
            person,
            start,
            goal: if loop_laps.is_some() { start } else { goal },
            waypoints,
            loop_laps,
            seed: rng.uniform_range(0.0, 1e6) as u64,
            individual,
        });
    }
    cases
}

/// Runs one case with the configuration it asks for.
fn run_case(case: &Case, image: &[u8], metrics: bool) -> SimulationOutput {
    let mut config = SimulationConfig::deterministic();
    config.with_metrics = metrics;
    // Every third case turns the event and jitter layers on, so the region path
    // and the position jitter are exercised on random geometry too.
    if case.index % 3 == 2 {
        config.sensors.multipath_enabled = true;
        config.sensors.magnetic_disturbance_enabled = true;
        config.sensors.jitter_enabled = true;
    }
    let builder = Simulator::builder()
        .map(MapSource::bytes(image.to_vec()))
        .person(case.person.clone())
        .config(config)
        .seed(case.seed)
        .individual(case.individual);
    let simulator = match case.loop_laps {
        Some(laps) => {
            let request = LoopRequest::new(case.start, laps);
            builder.looped(request).build()
        }
        None => {
            let mut request = StandardRequest::new(case.start, case.goal);
            request.waypoints = case.waypoints.clone();
            builder.standard(request).build()
        }
    }
    .expect("simulator builds");
    simulator.run().expect("run completes")
}

/// Invariants that must hold for every draw, independent of the individual.
fn check_invariants(
    case: &Case,
    output: &SimulationOutput,
    environment: &ourealis_core::environment::Environment,
) {
    let label = case.label();
    let samples = &output.trajectory.samples;
    assert!(!samples.is_empty(), "{label}: no trajectory samples");

    // Time is strictly increasing, and the recording spans a real duration.
    for window in samples.windows(2) {
        assert!(
            window[1].time_s > window[0].time_s,
            "{label}: time did not increase at t = {}",
            window[0].time_s
        );
    }
    assert!(
        output.duration_s() > 5.0,
        "{label}: duration {} is implausibly short",
        output.duration_s()
    );

    // No sample moves further than a runner can. This is the invariant that
    // catches a hold that begins a few centimetres short of its stop and then
    // jumps onto it, or a reversal whose offset is restored on the far side in one
    // sample: both are position steps of tens of centimetres, and neither breaks
    // any earlier assertion — the path is still feasible and the clock still
    // advances.
    const MAX_STEP_M: f64 = 0.12;
    for window in samples.windows(2) {
        let step = window[1].position.distance(window[0].position);
        assert!(
            step <= MAX_STEP_M,
            "{label}: {step:.3} m of movement in one sample at t = {:.2} (speed {:.2} -> {:.2},              standing {} -> {}, turning {} -> {})",
            window[0].time_s,
            window[0].speed,
            window[1].speed,
            window[0].standing,
            window[1].standing,
            window[0].turning,
            window[1].turning
        );
    }
    // The distance along the path must agree with the speed the sample reports. The
    // comparison is on the *centre* line, which is where the two are directly
    // related — the reported position also carries the lateral offset, which moves
    // sideways at up to a metre per second by design, so it is checked by the step
    // bound above instead. At the boundaries of a hold or a turn the body rotates
    // and the arc may stand still while the position sweeps, so only steady running
    // is compared.
    let settled = |index: usize| -> bool {
        let from = index.saturating_sub(50);
        let to = (index + 50).min(samples.len());
        samples[from..to]
            .iter()
            .all(|sample| sample.is_moving() && sample.speed > 0.5)
    };
    for (index, window) in samples.windows(2).enumerate() {
        if !settled(index) || !settled(index + 1) {
            continue;
        }
        let dt = window[1].time_s - window[0].time_s;
        let rate = window[1].center.distance(window[0].center) / dt;
        let reported = window[0].speed.max(window[1].speed);
        assert!(
            rate <= reported + 0.6,
            "{label}: sample {index} advanced along the path at {rate:.2} m/s while              reporting {reported:.2} m/s"
        );
    }

    // Every truth position is on passable ground.
    // The ground-truth trajectory stays on passable ground. The *reported* truth
    // position is deliberately not held to that: the design's high-frequency
    // position jitter (sigma_h = 2-5 cm) enters the reported position and not the
    // accelerometer's centroid path, so a report a few centimetres inside a wall
    // corner is the intended behaviour and has to be tolerated — but only within
    // the jitter scale, and the centroid path must be exact.
    assert_eq!(
        output.trajectory.samples.len(),
        output.truth.len(),
        "{label}: trajectory and truth differ in length"
    );
    for (sample, state) in output.trajectory.samples.iter().zip(output.truth.iter()) {
        assert!(
            passable(environment, sample.position),
            "{label}: the ground-truth trajectory left passable ground at ({:.2}, {:.2})              (t = {:.2} s, offset {:.3} m)",
            sample.position.x,
            sample.position.y,
            sample.time_s,
            sample.offset_m
        );
        assert_eq!(
            state.position_low, sample.position,
            "{label}: the centroid path is not the trajectory position"
        );
        let jitter = (state.position - sample.position).length();
        assert!(
            jitter <= 8.0 * 0.05 + 1e-9,
            "{label}: reported position is {jitter:.3} m from the trajectory, far beyond              the jitter scale"
        );
    }

    // Sensor streams exist, share the time base and are strictly monotonic.
    assert!(!output.sensors.gnss.is_empty(), "{label}: no GNSS fixes");
    assert!(
        !output.sensors.baro.is_empty(),
        "{label}: no barometer samples"
    );
    assert_eq!(
        output.sensors.imu.accel.len(),
        output.truth.len(),
        "{label}: accelerometer and truth lengths differ"
    );
    assert_eq!(
        output.sensors.imu.gyro.len(),
        output.truth.len(),
        "{label}: gyroscope and truth lengths differ"
    );
    for stream in [
        output
            .sensors
            .gnss
            .iter()
            .map(|s| s.time_s)
            .collect::<Vec<_>>(),
        output
            .sensors
            .imu
            .accel
            .iter()
            .map(|s| s.time_s)
            .collect::<Vec<_>>(),
        output
            .sensors
            .baro
            .iter()
            .map(|s| s.time_s)
            .collect::<Vec<_>>(),
    ] {
        for window in stream.windows(2) {
            assert!(
                window[1] > window[0],
                "{label}: a sensor stream is not strictly monotonic"
            );
        }
    }
    assert!(
        output.sensors.gnss_availability() > 0.5,
        "{label}: only {:.2} of GNSS epochs produced a fix",
        output.sensors.gnss_availability()
    );
}

/// Physical consistency between the streams and the truth they came from.
fn check_consistency(case: &Case, output: &SimulationOutput) {
    let label = case.label();
    let person = &output.manifest.person;

    // The accelerometer reads specific force: while the runner stands still it
    // must read gravity on its own axis.
    let standing: Vec<usize> = output
        .truth
        .iter()
        .enumerate()
        .filter(|(_, state)| state.standing)
        .map(|(index, _)| index)
        .collect();
    if let Some(index) = standing.get(standing.len() / 2) {
        let sample = &output.sensors.imu.accel[*index];
        let magnitude = (sample.x * sample.x + sample.y * sample.y + sample.z * sample.z).sqrt();
        assert!(
            (magnitude - 9.81).abs() < 0.8,
            "{label}: a standing accelerometer reads {magnitude:.2} m/s^2 instead of gravity"
        );
    }

    // Integrating the measured yaw rate must recover the heading change: the
    // gyroscope and the trajectory have to describe the same rotation.
    let dt = 1.0 / output.manifest.rates_hz[1];
    let integrated: f64 = output
        .sensors
        .imu
        .gyro
        .iter()
        .skip(1)
        .map(|sample| sample.z * dt)
        .sum();
    let first = output.truth.first().expect("first truth").heading;
    let last = output.truth.last().expect("last truth").heading;
    let turned = last - first;
    let bias_bound = person.sensors.gyro_bias_sigma + person.sensors.gyro_white_sigma * 20.0;
    assert!(
        (integrated - turned).abs() < 0.5 + bias_bound * output.duration_s(),
        "{label}: the gyroscope integrates to {integrated:.3} rad but the heading turned {turned:.3} rad"
    );

    // The barometer tracks the truth altitude with the instrument's own noise
    // scale, measured as spread rather than as a per-sample bound.
    let mut errors = Vec::new();
    for sample in &output.sensors.baro {
        if let Some(state) = ourealis_core::sensor::state_at(&output.truth, sample.time_s) {
            errors.push(sample.altitude_m - state.z);
        }
    }
    if errors.len() > 100 {
        let count = errors.len() as f64;
        let mean = errors.iter().sum::<f64>() / count;
        let rms = (errors.iter().map(|e| e * e).sum::<f64>() / count).sqrt();
        let sigma_m = person.sensors.baro_white_sigma_pa / 101_325.0 * 8_434.0;
        assert!(
            mean.abs() < sigma_m * 2.0,
            "{label}: barometric bias {mean:.3} m"
        );
        assert!(
            rms < sigma_m * 3.0,
            "{label}: barometric error rms {rms:.3} m against an instrument sigma of {sigma_m:.3} m"
        );
    }
}

/// Realism bands, widened from the fixed-route suite because the draws vary.
fn check_realism(case: &Case, metrics: &MetricsReport) {
    let label = case.label();
    let target = case.person.target_speed;

    assert!(
        metrics.passes_plausibility(),
        "{label}: the report failed its own plausibility checks"
    );
    assert!(
        (1.0..=3.0).contains(&metrics.path_ratio),
        "{label}: path ratio {:.2} is outside the plausible range",
        metrics.path_ratio
    );
    // The realised mean sits below the fresh target once fatigue, terrain and
    // stops are accounted for, but a run that finished must have run.
    assert!(
        metrics.speed.mean > target * 0.35 && metrics.speed.mean < target * 1.15,
        "{label}: mean speed {:.2} m/s against a target of {target:.2} m/s",
        metrics.speed.mean
    );
    // A runner's cadence is a narrow band regardless of the route.
    assert!(
        (2.0..=3.6).contains(&output_step_frequency(case)),
        "{label}: cadence {:.2} Hz is outside the human band",
        output_step_frequency(case)
    );
    // Noise with the designed correlation time stays correlated over 0.1 s.
    let acf_10 = metrics.speed_acf.get(10).copied().unwrap_or(0.0);
    assert!(
        acf_10 > 0.4,
        "{label}: the speed residual decorrelates within 0.1 s (ACF {acf_10:.3}), so the \
         noise looks white"
    );
    // The step signature must survive into the accelerometer spectrum.
    let summary = metrics
        .accel_spectrum
        .as_ref()
        .expect("an accelerometer spectrum");
    assert!(
        summary.step_peak.is_some(),
        "{label}: no spectral line at the step frequency"
    );
    assert!(
        metrics.baro_bounce_m > 0.003 && metrics.baro_bounce_m < 0.3,
        "{label}: the barometric step ripple of {:.4} m is not a human bounce",
        metrics.baro_bounce_m
    );
    // Turn rates stay inside what a person does while running.
    assert!(
        metrics.turn_rate.p95.abs() < 3.6,
        "{label}: p95 turn rate {:.2} rad/s is faster than an on-the-spot turn",
        metrics.turn_rate.p95
    );
    // Path geometry: a campus route is mostly gentle.
    assert!(
        metrics.mean_abs_curvature > 1e-5 && metrics.mean_abs_curvature < 0.25,
        "{label}: mean |kappa_eff| {:.4} suggests a degenerate or pathological path",
        metrics.mean_abs_curvature
    );
}

fn output_step_frequency(case: &Case) -> f64 {
    case.person.step_frequency
}

#[test]
fn random_cases_hold_their_invariants_and_stay_plausible() {
    let image = map_image();
    let environment = build_environment(&image);
    let mut cases = draw_cases();
    assert_eq!(cases.len(), CASES);

    for case in cases.iter_mut() {
        // A loop request must start and end at the same point.
        if case.loop_laps.is_some() {
            case.goal = case.start;
        }
        let output = run_case(case, &image, true);
        let label = case.label();
        check_invariants(case, &output, &environment);
        check_consistency(case, &output);
        let metrics = output.metrics.as_ref().expect("metrics");
        check_realism(case, metrics);
        println!(
            "{label}: {:.0} m in {:.0} s, mean {:.2} m/s, PR {:.2}, turns p95 {:.2} rad/s",
            output.route.length_m,
            output.duration_s(),
            metrics.speed.mean,
            metrics.path_ratio,
            metrics.turn_rate.p95
        );
    }
}

#[test]
fn random_cases_are_reproducible_and_distinct() {
    let image = map_image();
    let cases = draw_cases();

    for case in cases.iter().take(4) {
        let first = run_case(case, &image, false);
        let second = run_case(case, &image, false);
        assert_eq!(
            first.truth.len(),
            second.truth.len(),
            "{}: two runs of the same case produced different lengths",
            case.label()
        );
        for (a, b) in first.truth.iter().zip(second.truth.iter()) {
            assert_eq!(a.position, b.position, "{}: truth differs", case.label());
            assert_eq!(a.speed, b.speed, "{}: speed differs", case.label());
        }
        for (a, b) in first.sensors.gnss.iter().zip(second.sensors.gnss.iter()) {
            assert_eq!(a.x, b.x, "{}: GNSS differs", case.label());
            assert_eq!(a.y, b.y, "{}: GNSS differs", case.label());
        }
    }
}

#[test]
fn a_population_sweep_keeps_the_physical_relations() {
    // One simulator, many individuals: this is the shape the design's population
    // work takes, and it is where a per-individual bug (a shared stream, a
    // parameter that does not propagate) shows up as a broken relation rather
    // than as a bad single run.
    let image = map_image();
    let environment = build_environment(&image);
    let cases = draw_cases();
    let reference = &cases[0];

    let sampler = PersonSampler::new(
        Preset::Moderate,
        ourealis_core::person::PopulationSpread {
            target_speed: 0.6,
            ..Default::default()
        },
    );
    let people: Vec<PersonParams> = sampler
        .sample_population(SWEEP_SEED, 8)
        .expect("population");
    let runner = Simulator::builder()
        .map(MapSource::bytes(image.clone()))
        .person(people[0].clone())
        .standard(StandardRequest::new(reference.start, reference.goal))
        .config(SimulationConfig::deterministic())
        .seed(SWEEP_SEED)
        .build()
        .expect("simulator");
    let batch = ourealis_core::sim::BatchRunner::new(runner);
    let outputs = batch.run(&people).expect("batch");
    assert_eq!(outputs.len(), people.len());

    let mut distinct_paths = 0usize;
    let mut previous: Option<&PersonParams> = None;
    for (person, output) in people.iter().zip(outputs.iter()) {
        let label = format!(
            "population sweep, target {:.2} m/s (previous {:?})",
            person.target_speed,
            previous.map(|p| p.target_speed)
        );
        assert!(!output.trajectory.samples.is_empty(), "{label}: empty run");
        for state in &output.truth {
            assert!(
                passable(&environment, state.position),
                "{label}: left passable ground at ({:.1}, {:.1})",
                state.position.x,
                state.position.y
            );
        }
        let metrics = output.metrics.as_ref().expect("metrics");
        assert!(
            metrics.speed.mean > 0.8 && metrics.speed.mean < person.target_speed * 1.15,
            "{label}: mean speed {:.2} m/s is not plausible",
            metrics.speed.mean
        );
        if previous.is_some() {
            distinct_paths += 1;
        }
        previous = Some(person);
    }
    assert!(distinct_paths > 0);

    // Cadence and speed are related across the population; the fit must come out
    // weakly positive rather than flat or negative.
    let fit = batch
        .cadence_speed_fit(&outputs)
        .expect("a cadence-speed fit");
    println!(
        "cadence-speed fit: slope {:.3}, intercept {:.3}",
        fit.slope, fit.intercept
    );
    assert!(
        fit.slope > -5.0,
        "cadence falls with speed (slope {:.3}), which contradicts the parameter model",
        fit.slope
    );
}
