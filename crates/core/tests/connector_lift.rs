//! Z-axis connector elevation: the path channel, the lift and the motion stage.
//!
//! The fixture environment is the geometry-level half — plan, lift, inspect. The
//! fixture map drives the same geometry through the whole [`Simulator`], which
//! is what proves the lift is wired into `finish_run` and reaches the barometer.

mod fixtures;

use glam::DVec2;

use ourealis_core::environment::{Environment, PrmOptions};
use ourealis_core::graph::MixedGraph;
use ourealis_core::path::Path;
use ourealis_core::person::{PersonParams, Preset};
use ourealis_core::plan::{PlannedRoute, RouteConfig, StandardRequest, plan};
use ourealis_core::sim::connector_lift::lift_connector_elevations;
use ourealis_core::sim::{SimulationConfig, Simulator};

/// Plans one route across the fixture wall, which forces the stair link.
fn connector_route(environment: &Environment) -> PlannedRoute {
    connected_route(
        environment,
        environment.grid.cell_center(8, 30),
        environment.grid.cell_center(32, 30),
    )
}

/// Plans one route between two positions of the fixture environment.
fn connected_route(environment: &Environment, start: DVec2, goal: DVec2) -> PlannedRoute {
    let mut graph = MixedGraph::new(
        &environment.cost,
        &environment.hard,
        Some(&environment.terrain),
        environment.connectors.clone(),
        None,
        3.0,
    );
    let request = StandardRequest::new(start, goal);
    plan(
        environment,
        &mut graph,
        &request,
        &PersonParams::preset(Preset::Moderate),
        &RouteConfig::default(),
        7,
        0,
    )
    .expect("plan")
}

/// Arc length of the path vertex nearest to a position.
fn arc_at(path: &Path, position: DVec2) -> f64 {
    let index = path
        .points()
        .iter()
        .enumerate()
        .min_by(|a, b| {
            (*a.1 - position)
                .length()
                .partial_cmp(&(*b.1 - position).length())
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .map(|(index, _)| index)
        .expect("a non-empty path");
    path.cumulative()[index]
}

#[test]
fn path_elevation_channel_interpolates_and_stops_at_its_edges() {
    // No channel: the terrain always answers.
    let plain = Path::new(vec![
        DVec2::new(0.0, 0.0),
        DVec2::new(1.0, 0.0),
        DVec2::new(2.0, 0.0),
    ])
    .expect("path");
    assert_eq!(plain.elevation_at(0.5), None);
    assert_eq!(plain.grade_at(0.5), None);

    // A ramp between two overrides, and nothing beyond them: the segment that
    // leaves the ramp has a neighbour without an override and cannot be
    // interpolated.
    let ramp = Path::with_elevation(
        vec![
            DVec2::new(0.0, 0.0),
            DVec2::new(1.0, 0.0),
            DVec2::new(2.0, 0.0),
        ],
        vec![Some(0.0), Some(1.0), None],
    )
    .expect("path");
    assert_eq!(ramp.elevation_at(0.25), Some(0.25));
    assert_eq!(ramp.elevation_at(0.75), Some(0.75));
    assert_eq!(ramp.grade_at(0.5), Some(1.0));
    assert_eq!(ramp.elevation_at(1.25), None);
    assert_eq!(ramp.grade_at(1.5), None);

    // A lone override pins nothing: both of its segments have a neighbour
    // without an override, so it cannot smear over the terrain.
    let lone = Path::with_elevation(
        vec![
            DVec2::new(0.0, 0.0),
            DVec2::new(1.0, 0.0),
            DVec2::new(2.0, 0.0),
        ],
        vec![None, Some(5.0), None],
    )
    .expect("path");
    assert_eq!(lone.elevation_at(0.5), None);
    assert_eq!(lone.elevation_at(1.0), None);
    assert_eq!(lone.elevation_at(1.5), None);
    assert_eq!(lone.grade_at(1.0), None);

    // The channel must have one entry per point.
    assert!(Path::with_elevation(vec![DVec2::ZERO, DVec2::X], vec![Some(1.0)]).is_err());

    // Duplicates are removed from the points and their overrides in lockstep,
    // so the surviving point keeps its own value.
    let deduplicated = Path::with_elevation(
        vec![
            DVec2::new(0.0, 0.0),
            DVec2::new(0.0, 0.0),
            DVec2::new(1.0, 0.0),
        ],
        vec![Some(4.0), Some(9.0), Some(6.0)],
    )
    .expect("path");
    assert_eq!(deduplicated.len(), 2);
    assert_eq!(deduplicated.elevation_at(0.5), Some(5.0));
}

#[test]
fn lift_stamps_the_traversed_link_and_blends_back_into_the_terrain() {
    let environment = fixtures::connector_environment(20, 30);
    let link = environment.connectors.entries()[0];
    let route = connector_route(&environment);
    let lifted =
        lift_connector_elevations(&route.path, &environment.connectors, &environment.terrain)
            .expect("lift");

    // The path reaches both endpoints; without that the rest of the test would
    // be checking the wrong span.
    let arc_a = arc_at(&route.path, link.a);
    let arc_b = arc_at(&route.path, link.b);
    assert!((route.path.position_at(arc_a) - link.a).length() < 0.5);
    assert!((route.path.position_at(arc_b) - link.b).length() < 0.5);
    assert!(arc_b > arc_a + 1.0);

    assert_eq!(lifted.elevation_at(arc_a), Some(10.0));
    assert_eq!(lifted.elevation_at(arc_b), Some(13.0));
    let middle = 0.5 * (arc_a + arc_b);
    assert!((lifted.elevation_at(middle).expect("mid ramp") - 11.5).abs() < 1e-9);
    assert!((lifted.grade_at(middle).expect("mid grade") - 0.75).abs() < 1e-9);

    // Outside the link and its two terrain-pinning neighbours the channel is
    // silent again.
    assert_eq!(lifted.elevation_at(arc_a - 5.0), None);
    assert_eq!(lifted.elevation_at(arc_b + 5.0), None);
    assert_eq!(lifted.grade_at(arc_a - 5.0), None);
    assert_eq!(lifted.grade_at(arc_b + 5.0), None);

    // A route on the western side uses no connector and comes out untouched.
    let west = connected_route(
        &environment,
        environment.grid.cell_center(5, 10),
        environment.grid.cell_center(15, 10),
    );
    let west_lifted =
        lift_connector_elevations(&west.path, &environment.connectors, &environment.terrain)
            .expect("lift");
    assert_eq!(west_lifted, west.path);

    // The other direction is the same link with the endpoints swapped.
    let reversed = connected_route(
        &environment,
        environment.grid.cell_center(32, 30),
        environment.grid.cell_center(8, 30),
    );
    let reversed_lifted = lift_connector_elevations(
        &reversed.path,
        &environment.connectors,
        &environment.terrain,
    )
    .expect("lift");
    let reversed_b = arc_at(&reversed.path, link.b);
    let reversed_a = arc_at(&reversed.path, link.a);
    assert!(reversed_b < reversed_a);
    assert_eq!(reversed_lifted.elevation_at(reversed_b), Some(13.0));
    assert_eq!(reversed_lifted.elevation_at(reversed_a), Some(10.0));
    assert!(
        (reversed_lifted
            .grade_at(0.5 * (reversed_a + reversed_b))
            .expect("grade")
            + 0.75)
            .abs()
            < 1e-9
    );
}

#[test]
fn a_simulated_route_climbs_the_stair_into_the_barometer() {
    let reference = fixtures::connector_environment(20, 30);
    let link = reference.connectors.entries()[0];
    // The coarse and roadmap layers are off: this fixture has a single crossing,
    // and both layers would move the plan off the link's endpoints, which is
    // what the test needs to observe.
    let mut config = SimulationConfig::deterministic();
    config.prm = PrmOptions::None;
    config.coarse.enabled = false;
    let output = Simulator::builder()
        .map(fixtures::connector_map_source(20, 30))
        .person(PersonParams::preset(Preset::Moderate))
        .standard(StandardRequest::new(
            reference.grid.cell_center(10, 30),
            reference.grid.cell_center(32, 30),
        ))
        .setup(config, 7)
        .build()
        .expect("simulator")
        .run()
        .expect("run");

    // The lifted path is the one the trajectory was built from: the elevation
    // reached the motion stage through `finish_run`.
    let path = &output.trajectory.path;
    let arc_a = arc_at(path, link.a);
    let arc_b = arc_at(path, link.b);
    assert!((path.position_at(arc_a) - link.a).length() < 0.5);
    assert!((path.position_at(arc_b) - link.b).length() < 0.5);
    assert_eq!(path.elevation_at(arc_a), Some(10.0));
    assert_eq!(path.elevation_at(arc_b), Some(13.0));
    assert!((path.grade_at(0.5 * (arc_a + arc_b)).expect("grade") - 0.75).abs() < 1e-9);

    let sample_at = |arc: f64| {
        output
            .trajectory
            .samples
            .iter()
            .min_by(|a, b| {
                (a.arc_s - arc)
                    .abs()
                    .partial_cmp(&(b.arc_s - arc).abs())
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .expect("samples")
    };
    let baro_at = |time_s: f64| {
        output
            .sensors
            .baro
            .iter()
            .min_by(|a, b| {
                (a.time_s - time_s)
                    .abs()
                    .partial_cmp(&(b.time_s - time_s).abs())
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .expect("barometer samples")
    };

    // The truth altitude at the far endpoint is the connector's elevation, and
    // the barometer has followed the three metres up.
    let top = sample_at(arc_b);
    assert!(
        (top.terrain_z - 13.0).abs() < 0.1,
        "far endpoint truth altitude {}",
        top.terrain_z
    );
    let bottom = sample_at(arc_a);
    assert!((bottom.terrain_z - 10.0).abs() < 0.1);
    let climb = baro_at(top.time_s).altitude_m - baro_at(bottom.time_s).altitude_m;
    assert!(
        (climb - 3.0).abs() < 0.5,
        "barometric climb {climb} m over the stair"
    );

    // Grade and pitch see the ramp: the body leans forward by the ramp angle on
    // top of the speed-proportional lean.
    let middle = sample_at(0.5 * (arc_a + arc_b));
    assert!((middle.grade - 0.75).abs() < 1e-9);
    assert!(
        middle.pitch > 0.75_f64.atan() && middle.pitch < 0.9,
        "pitch {} on the stair",
        middle.pitch
    );

    // The altitude profile is a ramp, not a step: vertical velocity is positive
    // over the whole link and no single sample carries a jump.
    let window: Vec<_> = output
        .trajectory
        .samples
        .iter()
        .filter(|sample| sample.arc_s > arc_a + 0.1 && sample.arc_s < arc_b - 0.1)
        .collect();
    let mut climbing = 0usize;
    let mut largest_step: f64 = 0.0;
    for pair in window.windows(2) {
        let velocity = (pair[1].terrain_z - pair[0].terrain_z) / (pair[1].time_s - pair[0].time_s);
        if velocity > 0.1 {
            climbing += 1;
        }
        largest_step = largest_step.max((pair[1].terrain_z - pair[0].terrain_z).abs());
    }
    assert!(
        climbing > 100,
        "only {climbing} of {} samples in the link climb",
        window.len().saturating_sub(1)
    );
    assert!(
        largest_step < 0.05,
        "the altitude jumped {largest_step} m in one sample"
    );

    // The vertical acceleration stays within a budget a body can produce: a
    // one-sample step of three metres would read as thousands of m/s^2.
    let worst = output
        .truth
        .iter()
        .filter(|state| state.time_s >= bottom.time_s && state.time_s <= top.time_s)
        .map(|state| state.acceleration[2].abs())
        .fold(0.0f64, f64::max);
    assert!(
        worst < 60.0,
        "vertical acceleration {worst} m/s^2 on the stair"
    );
}

#[test]
fn a_route_that_uses_no_connector_keeps_the_terrain_profile() {
    let reference = fixtures::connector_environment(20, 30);
    let mut config = SimulationConfig::deterministic();
    config.prm = PrmOptions::None;
    config.coarse.enabled = false;
    let simulator = Simulator::builder()
        .map(fixtures::connector_map_source(20, 30))
        .person(PersonParams::preset(Preset::Moderate))
        .standard(StandardRequest::new(
            reference.grid.cell_center(5, 10),
            reference.grid.cell_center(15, 10),
        ))
        .setup(config, 7)
        .build()
        .expect("simulator");
    let environment = simulator.environment().expect("environment");
    let output = simulator.run().expect("run");

    // No vertex carries an override, and every sample's altitude is exactly the
    // terrain height under the position the runner actually occupies.
    for arc in output.trajectory.path.cumulative() {
        assert_eq!(output.trajectory.path.elevation_at(*arc), None);
        assert_eq!(output.trajectory.path.grade_at(*arc), None);
    }
    assert!(!output.truth.is_empty());
    for state in &output.truth {
        assert_eq!(
            state.terrain_z,
            environment.terrain.height_at(state.position),
            "an altitude override leaked into a connector-free route"
        );
    }
}
