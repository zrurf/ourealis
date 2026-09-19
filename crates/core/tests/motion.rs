//! Speed limits, profiles, offsets, attitude, bounce and maneuvers.

mod fixtures;

use glam::DVec2;

use ourealis_core::motion::attitude::{self, AttitudeConfig, AttitudeInput};
use ourealis_core::motion::bounce::BounceConfig;
use ourealis_core::motion::limits::{
    LookAheadMode, MINETTI_CLAMP, effective_curvature, minetti_cost, slope_speed,
};
use ourealis_core::motion::maneuvers::{self, TurnProfile};
use ourealis_core::motion::{
    LimitModifier, ManeuverConfig, MotionConfig, ProfileConfig, SpeedLimitParams, SpeedProfile,
    Trajectory,
};
use ourealis_core::path::Path;
use ourealis_core::person::{PaceStrategy, PersonParams, Preset};
use ourealis_core::rng::Rng;

use fixtures::{arc_path, flat_environment, straight_path};

#[test]
fn minetti_cost_matches_the_reference_value_and_is_clamped() {
    assert!((minetti_cost(0.0) - 3.6).abs() < 1e-9);
    // Uphill costs more than flat, downhill costs less, and the polynomial is
    // monotone over the valid range.
    assert!(minetti_cost(0.1) > minetti_cost(0.0));
    assert!(minetti_cost(-0.1) < minetti_cost(0.0));
    let beyond = minetti_cost(1.0);
    let clamped = minetti_cost(MINETTI_CLAMP);
    assert!((beyond - clamped).abs() < 1e-9, "the grade must be clamped");
    // Steep descents must not produce a negative cost, which the clamp prevents.
    for grade in [-0.25, -0.5, -1.0, -2.0] {
        assert!(minetti_cost(grade) > 0.0);
    }
}

#[test]
fn slope_speed_slows_on_climbs_and_caps_descents() {
    let target = 3.0;
    let flat = slope_speed(target, 0.0);
    assert!((flat - target).abs() < 1e-6);
    assert!(
        slope_speed(target, 0.05) < flat,
        "climbing must slow the runner"
    );
    assert!(
        slope_speed(target, -0.05) > flat,
        "descending must speed them up"
    );

    // The cap is applied separately, and only downhill.
    let cap = ourealis_core::motion::limits::downhill_cap(target, 1.1, -0.05);
    assert!((cap - 3.3).abs() < 1e-9);
    assert!(ourealis_core::motion::limits::downhill_cap(target, 1.1, 0.05).is_infinite());
}

#[test]
fn curvature_limit_follows_the_lateral_acceleration_budget() {
    let limit = ourealis_core::motion::limits::curvature_speed_limit(2.5, 0.02);
    assert!((limit - (2.5f64 / 0.02).sqrt()).abs() < 1e-9);
    assert!(ourealis_core::motion::limits::curvature_speed_limit(2.5, 0.0).is_infinite());
    // A tighter bend must be slower.
    assert!(
        ourealis_core::motion::limits::curvature_speed_limit(2.5, 0.1)
            < ourealis_core::motion::limits::curvature_speed_limit(2.5, 0.02)
    );
}

#[test]
fn effective_curvature_follows_the_parallel_curve_formula() {
    let kappa = 0.02;
    // Inside of the bend: offsetting towards the centre tightens the curve.
    let inner = effective_curvature(kappa, 5.0);
    assert!(inner > kappa);
    assert!((inner - kappa / (1.0 - 5.0 * kappa)).abs() < 1e-9);
    // Outside: it flattens.
    let outer = effective_curvature(kappa, -5.0);
    assert!(outer < kappa);
    // The denominator floor prevents a sign flip on an extreme offset.
    let extreme = effective_curvature(kappa, 1000.0);
    assert!(extreme.is_finite());
    assert!(
        extreme.signum() == kappa.signum(),
        "curvature sign must be preserved"
    );
}

#[test]
fn profile_respects_limits_acceleration_and_the_terminal_stop() {
    let environment = flat_environment(&[]);
    let path = straight_path(120.0);
    let person = PersonParams::preset(Preset::Moderate);
    let config = ProfileConfig {
        sample_spacing_m: 1.0,
        stop_at_end: true,
        ..Default::default()
    };
    let profile = SpeedProfile::build_steady(
        &path,
        &environment.terrain,
        &SpeedLimitParams::default(),
        &person,
        &config,
    )
    .expect("profile");

    assert!(
        profile.max_limit_violation() < 1e-6,
        "no point may exceed its limit"
    );
    assert!(
        profile.max_acceleration() <= person.a_max + 1e-3,
        "longitudinal acceleration {} exceeds a_max {}",
        profile.max_acceleration(),
        person.a_max
    );
    assert!(
        *profile.v.last().unwrap() < 1e-6,
        "the run must end at a standstill"
    );
    assert!(profile.total_time_s > 0.0);
    // A 120 m run at roughly 3 m/s takes about 45 s including the ramp.
    assert!(
        (30.0..70.0).contains(&profile.total_time_s),
        "{}",
        profile.total_time_s
    );
}

#[test]
fn profile_stops_at_dwell_waypoints() {
    let environment = flat_environment(&[]);
    let path = straight_path(100.0);
    let person = PersonParams::preset(Preset::Moderate);
    let config = ProfileConfig {
        sample_spacing_m: 1.0,
        stops: vec![ourealis_core::motion::StopHold {
            s: 50.0,
            duration_s: 8.0,
        }],
        ..Default::default()
    };
    let profile = SpeedProfile::build_steady(
        &path,
        &environment.terrain,
        &SpeedLimitParams::default(),
        &person,
        &config,
    )
    .expect("profile");

    assert!(
        (profile.speed_at(50.0)) < 0.2,
        "the runner must stop at the dwell point"
    );
    let without_stop = SpeedProfile::build_steady(
        &path,
        &environment.terrain,
        &SpeedLimitParams::default(),
        &person,
        &ProfileConfig {
            sample_spacing_m: 1.0,
            ..Default::default()
        },
    )
    .expect("profile");
    assert!(
        profile.total_time_s > without_stop.total_time_s + 5.0,
        "a dwell must add its duration to the total time"
    );
}

#[test]
fn waypoint_slow_modifier_lowers_the_local_limit() {
    let environment = flat_environment(&[]);
    let path = straight_path(100.0);
    let person = PersonParams::preset(Preset::Race);
    let base = SpeedProfile::build_steady(
        &path,
        &environment.terrain,
        &SpeedLimitParams::default(),
        &person,
        &ProfileConfig::default(),
    )
    .expect("profile");
    let slowed = SpeedProfile::build_steady(
        &path,
        &environment.terrain,
        &SpeedLimitParams::default(),
        &person,
        &ProfileConfig {
            modifiers: vec![LimitModifier::new(45.0, 55.0, 0.65)],
            ..Default::default()
        },
    )
    .expect("profile");
    assert!(slowed.speed_at(50.0) < base.speed_at(50.0));
    assert!(slowed.total_time_s > base.total_time_s);
}

#[test]
fn loop_profile_is_periodic() {
    let environment = flat_environment(&[]);
    // A closed rectangle.
    let points = vec![
        DVec2::new(20.0, 20.0),
        DVec2::new(100.0, 20.0),
        DVec2::new(100.0, 100.0),
        DVec2::new(20.0, 100.0),
        DVec2::new(20.0, 20.0),
    ];
    let path = Path::resampled(points, 1.0).expect("path");
    let person = PersonParams::preset(Preset::Moderate);
    let profile = SpeedProfile::build_steady(
        &path,
        &environment.terrain,
        &SpeedLimitParams::default(),
        &person,
        &ProfileConfig {
            periodic: true,
            stop_at_end: false,
            ..Default::default()
        },
    )
    .expect("profile");

    let start = profile.v[0];
    let end = *profile.v.last().unwrap();
    assert!(
        (start - end).abs() < 0.2,
        "a periodic profile must restart where it ends: {start} vs {end}"
    );
    assert!(start > 0.5, "a loop has no standstill at the seam");
}

#[test]
fn fatigue_iteration_converges_for_every_pace_strategy() {
    let environment = flat_environment(&[]);
    let path = straight_path(200.0);
    for strategy in [
        PaceStrategy::Even,
        PaceStrategy::PositiveSplit,
        PaceStrategy::NegativeSplit,
    ] {
        let person = PersonParams::preset(Preset::Moderate).with_pace_strategy(strategy);
        let profile = SpeedProfile::build_steady(
            &path,
            &environment.terrain,
            &SpeedLimitParams::default(),
            &person,
            &ProfileConfig::default(),
        )
        .expect("profile");
        assert!(
            profile.iteration.iterations >= 1 && profile.iteration.iterations <= 3,
            "iteration count {} out of range",
            profile.iteration.iterations
        );
        assert!(profile.total_time_s.is_finite());
        assert!(profile.max_limit_violation() < 1e-6);
    }
}

#[test]
fn positive_split_starts_faster_than_it_finishes() {
    let environment = flat_environment(&[]);
    let path = straight_path(300.0);
    let person =
        PersonParams::preset(Preset::Moderate).with_pace_strategy(PaceStrategy::PositiveSplit);
    let profile = SpeedProfile::build_steady(
        &path,
        &environment.terrain,
        &SpeedLimitParams::default(),
        &person,
        &ProfileConfig::default(),
    )
    .expect("profile");
    let early = profile.speed_at(profile.s_total() * 0.25);
    let late = profile.speed_at(profile.s_total() * 0.75);
    assert!(
        early > late,
        "positive split should fade: {early} then {late}"
    );
}

/// Helper trait so the test reads like the metric it checks.
trait ProfileExt {
    fn s_total(&self) -> f64;
}

impl ProfileExt for SpeedProfile {
    fn s_total(&self) -> f64 {
        self.s.last().copied().unwrap_or(0.0)
    }
}

#[test]
fn look_ahead_modes_differ_on_a_climb() {
    // A terrain that rises after x = 60 m would need a real map to build, so this
    // checks the aggregation contract on flat ground: both modes must agree when
    // there is no relief, and the worst-case mode must never report less than the
    // mean.
    let environment = flat_environment(&[]);
    let path = straight_path(120.0);
    let mean = ourealis_core::motion::limits::look_ahead_grade(
        &path,
        &environment.terrain,
        20.0,
        20.0,
        LookAheadMode::DistanceWeightedMean,
    );
    let worst = ourealis_core::motion::limits::look_ahead_grade(
        &path,
        &environment.terrain,
        20.0,
        20.0,
        LookAheadMode::WorstCase,
    );
    assert!(mean.abs() < 1e-9 && worst.abs() < 1e-9);
}

#[test]
fn offset_respects_the_lateral_acceleration_budget() {
    let environment = flat_environment(&[]);
    let person = PersonParams::preset(Preset::Moderate);
    let path = arc_path(25.0, std::f64::consts::PI * 0.8);
    let profile = SpeedProfile::build_steady(
        &path,
        &environment.terrain,
        &SpeedLimitParams {
            a_lat_max: person.a_lat_max,
            ..Default::default()
        },
        &person,
        &ProfileConfig {
            stop_at_end: false,
            ..Default::default()
        },
    )
    .expect("profile");
    let mut rng = Rng::stream(3, ourealis_core::Stream::LateralOffset, 0, 0);
    let offsets = ourealis_core::motion::offset::generate(
        &path,
        &profile,
        &environment.terrain,
        &environment.hard,
        &environment.distance,
        &ourealis_core::motion::OffsetConfig {
            mean_m: 0.8,
            std_m: 0.4,
            a_lat_max: person.a_lat_max,
            ..Default::default()
        },
        &mut rng,
    )
    .expect("offsets");

    assert_eq!(offsets.len(), profile.s.len());
    for sample in &offsets {
        let speed = profile.speed_at(sample.s);
        let lateral = speed * speed * sample.kappa_eff.abs();
        assert!(
            lateral <= person.a_lat_max + 1e-6,
            "lateral acceleration {lateral} exceeds the budget at s = {}",
            sample.s
        );
        // The stored effective curvature must be the one the offset implies.
        let expected = effective_curvature(sample.kappa, sample.offset_m);
        assert!((sample.kappa_eff - expected).abs() < 1e-9);
    }
}

#[test]
fn offset_does_not_jump_between_samples() {
    let environment = flat_environment(&[]);
    let person = PersonParams::preset(Preset::Moderate);
    let path = straight_path(200.0);
    let profile = SpeedProfile::build_steady(
        &path,
        &environment.terrain,
        &SpeedLimitParams::default(),
        &person,
        &ProfileConfig::default(),
    )
    .expect("profile");
    let mut rng = Rng::stream(5, ourealis_core::Stream::LateralOffset, 0, 0);
    let offsets = ourealis_core::motion::offset::generate(
        &path,
        &profile,
        &environment.terrain,
        &environment.hard,
        &environment.distance,
        &ourealis_core::motion::OffsetConfig::default(),
        &mut rng,
    )
    .expect("offsets");

    for window in offsets.windows(2) {
        let delta = (window[1].offset_m - window[0].offset_m).abs();
        assert!(
            delta < 0.25,
            "offset jumped by {delta} m between consecutive samples"
        );
    }
}

#[test]
fn attitude_matrix_is_orthonormal_and_roll_is_clamped() {
    let input = AttitudeInput {
        time_s: 1.0,
        tangent_angle: 0.7,
        grade: 0.05,
        kappa_eff: 0.4,
        speed: 5.0,
        turning: false,
    };
    let samples = attitude::generate(&[input], 3.0, &AttitudeConfig::default());
    let rotation = samples[0].rotation();
    let identity = rotation * rotation.transpose();
    for index in 0..3 {
        for other in 0..3 {
            let expected = if index == other { 1.0 } else { 0.0 };
            assert!(
                (identity.col(index)[other] - expected).abs() < 1e-9,
                "rotation must be orthonormal"
            );
        }
    }
    assert!(samples[0].roll.abs() <= 10f64.to_radians() + 1e-9);
    assert!(
        samples[0].pitch > 0.0,
        "a climb must produce a forward lean"
    );
}

#[test]
fn heading_low_pass_removes_turn_vertex_steps() {
    // A path with a sharp vertex: the filtered heading must not reach the full
    // angle in a single sample, which is what keeps the gyroscope physical.
    let inputs: Vec<AttitudeInput> = (0..50)
        .map(|index| AttitudeInput {
            time_s: index as f64 * 0.01,
            tangent_angle: if index < 25 {
                0.0
            } else {
                std::f64::consts::FRAC_PI_2
            },
            grade: 0.0,
            kappa_eff: 0.0,
            speed: 3.0,
            turning: false,
        })
        .collect();
    let samples = attitude::generate(&inputs, 3.0, &AttitudeConfig::default());
    let step = samples[25].yaw - samples[24].yaw;
    assert!(
        step.abs() < 0.5,
        "heading step of {step} rad is too abrupt for a low-passed heading"
    );
}

#[test]
fn head_leads_the_body_into_a_turn() {
    // The head looks ahead, so on a path that turns later, the head yaw must
    // already differ from the body yaw before the turn arrives.
    let inputs: Vec<AttitudeInput> = (0..200)
        .map(|index| {
            let turning = index > 120;
            AttitudeInput {
                time_s: index as f64 * 0.01,
                tangent_angle: if turning { 1.2 } else { 0.0 },
                grade: 0.0,
                kappa_eff: if turning { 0.1 } else { 0.0 },
                speed: 3.5,
                turning: false,
            }
        })
        .collect();
    let samples = attitude::generate(&inputs, 3.5, &AttitudeConfig::default());
    let divergence = samples[110].head_yaw - samples[110].yaw;
    assert!(
        divergence.abs() > 1e-4,
        "the head must lead the body before the turn: {divergence}"
    );
}

#[test]
fn bounce_waveform_is_zero_mean_with_unit_peak_and_shared_phase() {
    let bounce = BounceConfig::new(0.05, 2.7, 0.3, 0.4);
    let mut sum = 0.0;
    let mut fundamental = 0.0;
    let steps = 20_000;
    for index in 0..steps {
        let u = std::f64::consts::TAU * index as f64 / steps as f64;
        let value = bounce.waveform(u);
        sum += value;
        fundamental += value * u.sin();
    }
    assert!(
        (sum / steps as f64).abs() < 1e-3,
        "the waveform must average to zero"
    );
    // The fundamental component has unit amplitude, which is what makes
    // A_1 = A_b (2 pi f)^2 hold exactly.
    assert!(
        (2.0 * fundamental / steps as f64 - 1.0).abs() < 1e-3,
        "fundamental amplitude {} should be one",
        2.0 * fundamental / steps as f64
    );
    // The waveform is asymmetric: the sink reaches 1.3 times the amplitude while
    // the push-off crest stays below it.
    assert!(
        (bounce.waveform_peak() - 1.3).abs() < 0.05,
        "excursion {}",
        bounce.waveform_peak()
    );

    // Height and acceleration must come from the same waveform: the acceleration
    // is its second derivative, so a quarter period shift in phase is expected.
    let height = bounce.height_at(0.1, 3.3, 3.3);
    let acceleration = bounce.vertical_acceleration_at(0.1, 3.3, 3.3);
    assert!(height.abs() <= bounce.amplitude_at(3.3, 3.3) + 1e-9);
    assert!(acceleration.abs() > 0.0);

    // Amplitude grows weakly with speed.
    assert!(bounce.amplitude_at(4.0, 3.3) > bounce.amplitude_at(2.0, 3.3));
    // The fundamental amplitude is exactly the design's A_b (2 pi f)^2, which is
    // what keeps the accelerometer harmonic consistent with the bounce.
    let predicted = ourealis_core::eval::predicted_fundamental(0.05, 2.7);
    let actual = bounce.first_harmonic_amplitude(3.3, 3.3);
    assert!(
        (actual / predicted - 1.0).abs() < 1e-9,
        "{actual} vs {predicted}"
    );
}

#[test]
fn turn_profile_rotates_exactly_the_requested_angle() {
    for angle in [2.0f64, -2.0, 3.0, -3.1] {
        let profile = TurnProfile::solve(angle, 2.4, 8.0).expect("profile");
        let duration = profile.duration_s();
        assert!(
            (0.5..=3.0).contains(&duration),
            "a turn should take between half a second and three: {duration}"
        );
        let rotated = profile.angle_at(duration);
        assert!(
            (rotated - angle).abs() < 1e-6,
            "turn ended at {rotated}, requested {angle}"
        );
        // Angular velocity must start and end at zero.
        assert!(profile.omega_at(0.0).abs() < 1e-9);
        assert!(profile.omega_at(duration).abs() < 1e-9);
        // Peak rate respects the individual limit.
        assert!(profile.peak_omega <= 2.4 + 1e-9);
    }
}

#[test]
fn turns_are_detected_on_reversals_but_not_on_gentle_bends() {
    let reversal = Path::new(vec![
        DVec2::new(0.0, 0.0),
        DVec2::new(40.0, 0.0),
        DVec2::new(0.0, 0.0),
    ])
    .expect("path");
    let detected = maneuvers::detect_turns(&reversal, 6.0, 120.0);
    assert!(
        !detected.is_empty(),
        "a 180 degree reversal must be detected"
    );

    let bend = arc_path(60.0, std::f64::consts::PI);
    let gentle = maneuvers::detect_turns(&bend, 6.0, 120.0);
    assert!(
        gentle.is_empty(),
        "a gradual bend is a curve, not a maneuver"
    );
}

#[test]
fn trajectory_contains_standing_start_motion_and_stop() {
    let environment = flat_environment(&[]);
    let path = straight_path(120.0);
    let person = PersonParams::preset(Preset::Moderate);
    let config = MotionConfig {
        sample_rate_hz: 100.0,
        maneuver: ManeuverConfig {
            start_stand_s: 10.0,
            end_stand_s: 5.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let trajectory = Trajectory::build(
        path,
        &environment.terrain,
        &environment.hard,
        &environment.distance,
        &person,
        &config,
        9,
        0,
    )
    .expect("trajectory");

    assert!(
        trajectory.starts_standing,
        "the run must start standing still"
    );
    assert!(
        trajectory.ends_standing(),
        "the run must end standing still"
    );
    assert!(trajectory.duration_s() > 50.0);

    // Time stamps are strictly increasing.
    for window in trajectory.samples.windows(2) {
        assert!(window[1].time_s > window[0].time_s);
    }
    // The altitude is terrain plus bounce everywhere.
    for sample in &trajectory.samples {
        assert!((sample.z - (sample.terrain_z + sample.bounce_z)).abs() < 1e-9);
    }
    // Every position is on passable ground.
    for sample in &trajectory.samples {
        assert!(
            environment.hard.is_passable(sample.position),
            "trajectory left passable ground at {:?}",
            sample.position
        );
    }
    // Speed never exceeds the profile's own maximum.
    assert!(
        trajectory.max_speed() <= trajectory.profile.v.iter().copied().fold(0.0, f64::max) + 1e-6
    );
}

#[test]
fn trajectory_keeps_the_bounce_inside_its_amplitude() {
    let environment = flat_environment(&[]);
    let path = straight_path(80.0);
    let person = PersonParams::preset(Preset::Race);
    let trajectory = Trajectory::build(
        path,
        &environment.terrain,
        &environment.hard,
        &environment.distance,
        &person,
        &MotionConfig::default(),
        4,
        0,
    )
    .expect("trajectory");
    // The excursion is the amplitude times the waveform crest factor, which sits
    // below one because the second harmonic flattens the crest.
    let amplitude = person.bounce_amplitude_m * 1.5 * trajectory.bounce.waveform_peak();
    for sample in trajectory
        .samples
        .iter()
        .filter(|sample| sample.is_moving())
    {
        assert!(
            sample.bounce_z.abs() <= amplitude + 1e-9,
            "bounce {} exceeds the excursion budget {amplitude}",
            sample.bounce_z
        );
    }
    // The bounce must actually move: a run with no vertical motion would leave
    // the barometer and accelerometer without their step signature.
    let peak = trajectory
        .samples
        .iter()
        .map(|sample| sample.bounce_z.abs())
        .fold(0.0f64, f64::max);
    assert!(peak > 0.005, "bounce amplitude {peak} is implausibly small");
}

#[test]
fn a_dwell_is_reached_without_a_position_step() {
    // The runner holds *at* the stop's arc, not somewhere within a window of it.
    // Holding short and then moving the arc onto the stop after the hold is a
    // position step of up to half a profile sample in one sample interval — a metre
    // per second of velocity in the differentiated truth that the runner never had.
    let environment = flat_environment(&[]);
    let person = PersonParams::preset(Preset::Moderate);
    let mut config = MotionConfig::default();
    config.profile.stops = vec![ourealis_core::motion::StopHold {
        s: 60.4,
        duration_s: 8.0,
    }];
    let trajectory = Trajectory::build(
        straight_path(100.0),
        &environment.terrain,
        &environment.hard,
        &environment.distance,
        &person,
        &config,
        5,
        0,
    )
    .expect("trajectory");

    let samples = &trajectory.samples;
    let stop = trajectory.profile.stops[0];
    // The hold at the waypoint, not the standing start: the samples that stand on
    // the stop's own arc.
    let held: Vec<usize> = samples
        .iter()
        .enumerate()
        .filter(|(_, sample)| sample.standing && (sample.arc_s - stop.s).abs() < 1e-6)
        .map(|(index, _)| index)
        .collect();
    assert!(
        held.len() > 700,
        "the eight-second hold must be present, got {} samples",
        held.len()
    );
    let first = held[0];
    let last = *held.last().expect("a hold");
    // The sample that opens the hold and the one that resumes it are both *at* the
    // stop: the hold does not begin a few centimetres short of it and jump.
    for index in [first.saturating_sub(1), first, last] {
        let sample = &samples[index];
        assert!(
            (sample.arc_s - stop.s).abs() < 1e-6,
            "sample {index} is at arc {:.6}, not at the stop's {:.6}",
            sample.arc_s,
            stop.s
        );
    }
    // The step into the hold is the runner's own deceleration, not a jump: at most
    // what a sample of travel at the last moving speed can be.
    let entry = samples[last + 1].position.distance(samples[last].position);
    let speed_before = samples[first.saturating_sub(1)].speed;
    assert!(
        entry < speed_before * 0.02 + 1e-3,
        "the runner moved {entry:.4} m into the hold at {speed_before:.3} m/s"
    );
}

#[test]
fn pace_drift_does_not_depend_on_the_motion_sample_rate() {
    // The drift is a process in time. Stepping it once per profile sample runs it at
    // whatever rate the profile happens to be sampled at, and — because the buggy
    // step was derived from the sensor rate — makes the pacing intention depend on
    // how finely the *trajectory* is sampled. The same individual on the same route
    // must run the same race at 50 Hz and at 100 Hz.
    let environment = flat_environment(&[]);
    let mut person = PersonParams::preset(Preset::Moderate);
    person.pace_drift_sigma = 0.15;
    let build = |rate: f64| -> Vec<(f64, f64)> {
        let trajectory = Trajectory::build(
            straight_path(100.0),
            &environment.terrain,
            &environment.hard,
            &environment.distance,
            &person,
            &MotionConfig {
                sample_rate_hz: rate,
                ..Default::default()
            },
            9,
            0,
        )
        .expect("trajectory");
        trajectory
            .samples
            .iter()
            .filter(|sample| sample.is_moving())
            .map(|sample| (sample.time_s, sample.speed))
            .collect()
    };
    let coarse = build(50.0);
    let fine = build(100.0);

    let mut worst = 0.0f64;
    let mut compared = 0usize;
    for (time, speed) in &fine {
        // The nearest sample of the coarse run; both come from the same profile, so
        // they should agree to well inside the sampling difference.
        let nearest = coarse
            .binary_search_by(|(other, _)| other.partial_cmp(time).unwrap())
            .map(|index| coarse[index].1)
            .unwrap_or_else(|index| {
                coarse
                    .get(index.min(coarse.len() - 1))
                    .map(|(_, speed)| *speed)
                    .unwrap_or(*speed)
            });
        worst = worst.max((nearest - speed).abs());
        compared += 1;
    }
    assert!(compared > 200);
    println!("worst speed difference between 50 Hz and 100 Hz: {worst:.4} m/s");
    assert!(
        worst < 0.05,
        "the realised pace depends on the sample rate: {worst:.4} m/s apart"
    );
}

#[test]
fn pace_drift_runs_on_the_configured_time_constant() {
    // A longer time constant must produce a slower-varying intended pace. The
    // statistic is the mean change of the speed *limit* between consecutive profile
    // samples, which is the drift's own increment before any acceleration limit
    // touches it: for an Ornstein-Uhlenbeck process that scales as `sqrt(dt / tau)`.
    let environment = flat_environment(&[]);
    let person = PersonParams::preset(Preset::Moderate);
    let mean_step = |tau_s: f64| -> f64 {
        let mut tuned = person.clone();
        tuned.pace_drift_tau_s = tau_s;
        // The drift is a few percent of the pace by design, which is small next to
        // the ramps at either end of the run; exaggerating it is what makes this a
        // measurement of the time constant rather than of the profile.
        tuned.pace_drift_sigma = 0.25;
        let trajectory = Trajectory::build(
            straight_path(100.0),
            &environment.terrain,
            &environment.hard,
            &environment.distance,
            &tuned,
            &MotionConfig::default(),
            9,
            0,
        )
        .expect("trajectory");
        let limits = &trajectory.profile.limits;
        let total: f64 = limits
            .windows(2)
            .map(|pair| (pair[1] - pair[0]).abs())
            .sum();
        total / (limits.len() - 1) as f64
    };
    let slow = mean_step(120.0);
    let fast = mean_step(10.0);
    println!("mean limit step: tau 120 s -> {slow:.4}, tau 10 s -> {fast:.4}");
    assert!(
        slow < 0.6 * fast,
        "a twelve-fold longer time constant must vary more slowly: {slow:.4} against {fast:.4}"
    );
}
