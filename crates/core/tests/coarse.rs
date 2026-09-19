//! Coarse granularity: the map's quadtree leaves as search nodes.
//!
//! The format partitions space into uniform blocks and marks them with a
//! drill-down hint where they are not uniform. Those blocks are the map's own
//! statement about granularity, and the graph uses them as nodes. What has to hold
//! is that the two granularities agree: a block may only stand in for its area
//! when it really is traversable throughout, paths must stay feasible with the
//! layer on, and the layer must actually reduce the node count.

mod fixtures;

use glam::DVec2;

use ourealis_core::environment::{Environment, PrmOptions};
use ourealis_core::field::{CostModelParams, CostWeights};
use ourealis_core::graph::{CoarseGrid, CoarseOptions, MixedGraph, NodeId, NodeKind};
use ourealis_core::person::{PersonParams, Preset};
use ourealis_core::plan::StandardRequest;
use ourealis_core::search::ThetaStar;
use ourealis_core::sim::{MapSource, SimulationConfig, Simulator};
use ourealis_map_format::Map;
use ourealis_map_format::synthetic::{self, SyntheticMapSpec};

fn load(spec: SyntheticMapSpec) -> (Vec<u8>, Map) {
    let image = synthetic::build(&spec).expect("map image");
    let map = Map::from_bytes(image.clone()).expect("open");
    (image, map)
}

fn environment(map: &Map) -> Environment {
    let dimension = map
        .feature_schema()
        .ok()
        .flatten()
        .map(|schema| schema.dim() as usize)
        .unwrap_or(1)
        .max(1);
    let weights = CostWeights::uniform(dimension);
    Environment::load(map, &weights, &CostModelParams::default(), PrmOptions::None)
        .expect("environment")
}

#[test]
fn coarse_blocks_are_traversable_throughout() {
    // The admission rule is the format's: a block is usable at coarse granularity
    // only when its aggregate maximum is below the passable threshold. Otherwise
    // one node would stand for a block containing a wall, and the search would
    // walk through it.
    let (_, map) = load(SyntheticMapSpec::compact());
    let environment = environment(&map);
    assert!(
        !environment.coarse.is_empty(),
        "the synthetic map should contribute coarse blocks"
    );

    let grid = &environment.grid;
    for cell in environment.coarse.cells() {
        assert!(
            cell.aggr_max <= 0.0,
            "a block with a non-zero aggregate maximum was admitted"
        );
        // Every cell centre inside the block is passable.
        let first_x = ((cell.bounds.min_x - environment.bounds.min_x) / grid.resolution).round();
        let first_y = ((cell.bounds.min_y - environment.bounds.min_y) / grid.resolution).round();
        let count = (cell.size_m() / grid.resolution).round().max(1.0) as i64;
        for iy in 0..count {
            for ix in 0..count {
                let point = DVec2::new(
                    environment.bounds.min_x + (first_x + ix as f64 + 0.5) * grid.resolution,
                    environment.bounds.min_y + (first_y + iy as f64 + 0.5) * grid.resolution,
                );
                if !environment.bounds.contains(point.x, point.y) {
                    continue;
                }
                assert!(
                    environment.hard.is_passable(point),
                    "coarse block at ({:.1}, {:.1}) covers forbidden ground at ({:.1}, {:.1})",
                    cell.center.x,
                    cell.center.y,
                    point.x,
                    point.y
                );
            }
        }
    }
}

#[test]
fn flagged_blocks_are_refined_into_their_passable_quarters() {
    // A block the map flagged needs refinement: it contains something the partition
    // could not treat as one uniform area, typically one building corner. Dropping
    // it hands the whole block to the metre grid; the design asks for refinement,
    // which keeps the parts that *are* uniform at a coarse node. Disabling the
    // refinement through its size floor gives the baseline to compare against.
    let (_, map) = load(SyntheticMapSpec::default());
    let environment = environment(&map);
    let connectors = ourealis_core::graph::ConnectorSet::new(
        &map.connectors().expect("connectors").unwrap_or_default(),
    );
    let plain = CoarseGrid::from_map_refined(
        &map,
        &CoarseOptions {
            min_drill_size_m: 1.0e9,
            ..Default::default()
        },
        &environment.hard,
        &connectors,
    )
    .expect("plain layer");
    let refined = CoarseGrid::from_map_refined(
        &map,
        &CoarseOptions::default(),
        &environment.hard,
        &connectors,
    )
    .expect("refined layer");

    let area = |layer: &CoarseGrid| -> f64 {
        layer
            .cells()
            .iter()
            .map(|cell| cell.size_m() * cell.size_m())
            .sum()
    };
    let smallest = |layer: &CoarseGrid| -> f64 {
        layer
            .cells()
            .iter()
            .map(|cell| cell.size_m())
            .fold(f64::INFINITY, f64::min)
    };
    println!(
        "refinement: {} blocks ({:.0} m^2, smallest {:.1} m) against {} ({:.0} m^2, {:.1} m)",
        refined.len(),
        area(&refined),
        smallest(&refined),
        plain.len(),
        area(&plain),
        smallest(&plain)
    );
    assert!(
        area(&refined) >= area(&plain) - 1e-6,
        "refinement must not lose ground"
    );
    assert!(
        smallest(&refined) <= smallest(&plain) + 1e-6,
        "refinement must reach a finer granularity"
    );
    // And every block it produced is still on passable ground.
    for cell in refined.cells() {
        let grid = &environment.grid;
        let first_x = ((cell.bounds.min_x - environment.bounds.min_x) / grid.resolution)
            .round()
            .max(0.0);
        let first_y = ((cell.bounds.min_y - environment.bounds.min_y) / grid.resolution)
            .round()
            .max(0.0);
        let count = (cell.size_m() / grid.resolution).round().max(1.0) as i64;
        for iy in 0..count {
            for ix in 0..count {
                let point = DVec2::new(
                    environment.bounds.min_x + (first_x + ix as f64 + 0.5) * grid.resolution,
                    environment.bounds.min_y + (first_y + iy as f64 + 0.5) * grid.resolution,
                );
                if !environment.bounds.contains(point.x, point.y) {
                    continue;
                }
                assert!(
                    environment.hard.is_passable(point),
                    "a refined block at ({:.1}, {:.1}) covers forbidden ground at ({:.1}, {:.1})",
                    cell.center.x,
                    cell.center.y,
                    point.x,
                    point.y
                );
            }
        }
    }
}

#[test]
fn fine_cells_inside_a_block_are_replaced_by_it() {
    // A block is one node; the metre cells it covers must not be reachable, or the
    // two granularities would both be present and the layer would only add nodes.
    let (_, map) = load(SyntheticMapSpec::compact());
    let environment = environment(&map);
    let mut graph = MixedGraph::with_coarse(
        &environment.cost,
        &environment.hard,
        Some(&environment.terrain),
        environment.connectors.clone(),
        None,
        environment.coarse.clone(),
        3.0,
    );

    let mut interior_checked = 0usize;
    let mut boundary_checked = 0usize;
    for cell in environment.coarse.cells() {
        // A cell whose centre sits inside the block must have no edges.
        let (cx, cy) = environment.grid.cell_of(cell.center);
        let interior = NodeId::grid(environment.grid.index(cx, cy) as u32);
        assert!(
            graph.neighbours(interior).is_empty(),
            "a fine cell inside a block still has edges"
        );
        interior_checked += 1;

        // The block itself must connect to the fine grid around it, or the
        // granularities would be two disconnected graphs.
        let index = environment
            .coarse
            .cell_at(cell.center)
            .expect("the block is indexed");
        let block = NodeId::coarse(index);
        let edges = graph.neighbours(block);
        assert!(
            edges.iter().any(|edge| edge.to.kind() == NodeKind::Grid)
                || edges.iter().any(|edge| edge.to.kind() == NodeKind::Coarse),
            "the block at ({:.2}, {:.2}), size {:.1} m depth {}, has no neighbours",
            cell.center.x,
            cell.center.y,
            cell.size_m(),
            cell.depth
        );
        boundary_checked += 1;
    }
    assert!(interior_checked > 20 && boundary_checked > 20);
}

#[test]
fn the_coarse_layer_shrinks_the_search() {
    // The point of the layer is fewer nodes. Measure it as expansions, which is
    // what the search actually pays.
    let (_, map) = load(SyntheticMapSpec::default());
    let environment = environment(&map);
    let start = DVec2::new(40.0, 40.0);
    let goal = DVec2::new(560.0, 360.0);

    let expansions = |coarse: CoarseOptions| -> usize {
        let layer = ourealis_core::graph::CoarseGrid::from_map(&map, &coarse).expect("layer");
        let mut graph = MixedGraph::with_coarse(
            &environment.cost,
            &environment.hard,
            Some(&environment.terrain),
            environment.connectors.clone(),
            None,
            layer,
            3.0,
        );
        let mut search = ThetaStar::new(&mut graph, Default::default());
        search.plan(start, goal).expect("path").expanded
    };

    let without = expansions(CoarseOptions {
        enabled: false,
        ..Default::default()
    });
    let with = expansions(CoarseOptions::default());
    println!("expansions: {without} without coarse nodes, {with} with them");
    assert!(
        with < without,
        "the coarse layer expanded {with} nodes against {without} without it"
    );
}

#[test]
fn paths_stay_feasible_with_and_without_the_coarse_layer() {
    let (image, map) = load(SyntheticMapSpec::compact());
    let environment = environment(&map);
    let pairs = [
        (DVec2::new(30.0, 100.0), DVec2::new(260.0, 100.0)),
        (DVec2::new(40.0, 40.0), DVec2::new(250.0, 150.0)),
        (DVec2::new(100.0, 20.0), DVec2::new(120.0, 180.0)),
    ];

    for (start, goal) in pairs {
        let run = |enabled: bool| {
            let mut config = SimulationConfig::deterministic();
            config.coarse = CoarseOptions {
                enabled,
                ..Default::default()
            };
            let output = Simulator::builder()
                .map(MapSource::bytes(image.clone()))
                .person(PersonParams::preset(Preset::Moderate))
                .standard(StandardRequest::new(start, goal))
                .config(config)
                .seed(7)
                .build()
                .expect("simulator")
                .run()
                .expect("run");
            // Feasibility of the geometry and of the ground truth.
            for window in output.trajectory.path.points().windows(2) {
                assert!(
                    environment.hard.segment_is_clear(window[0], window[1]),
                    "the path crosses forbidden ground (coarse = {enabled})"
                );
            }
            for sample in &output.trajectory.samples {
                assert!(
                    environment.hard.is_passable(sample.position),
                    "the trajectory left passable ground (coarse = {enabled})"
                );
            }
            output.route.length_m
        };
        let without = run(false);
        let with = run(true);
        println!(
            "({:.0},{:.0}) -> ({:.0},{:.0}): {without:.0} m without coarse, {with:.0} m with",
            start.x, start.y, goal.x, goal.y
        );
    }
}

#[test]
fn the_size_cap_bounds_the_admitted_blocks() {
    // `max_size_m` is the documented granularity bound: a leaf larger than it is
    // described at a scale the lateral offset and safety radius cannot refine, so
    // it must not become one node. The default layer exercises the cap; the
    // capped layer must not admit anything above it.
    let (image, map) = load(SyntheticMapSpec::compact());
    let environment = environment(&map);
    let default_layer = CoarseGrid::from_map(&map, &CoarseOptions::default()).expect("layer");
    let capped = CoarseGrid::from_map(
        &map,
        &CoarseOptions {
            max_size_m: 12.0,
            ..Default::default()
        },
    )
    .expect("layer");

    let largest = |layer: &CoarseGrid| {
        layer
            .cells()
            .iter()
            .map(|cell| cell.size_m())
            .fold(0.0f64, f64::max)
    };
    assert!(
        largest(&default_layer) > 12.0,
        "the default layer must contain a block above the test cap to exercise it"
    );
    assert!(
        !capped.is_empty(),
        "the capped layer must still admit the smaller blocks"
    );
    assert!(
        largest(&capped) <= 12.0 + 1e-9,
        "a block larger than max_size_m was admitted: {:.1} m",
        largest(&capped)
    );
    assert!(
        capped.len() < default_layer.len(),
        "the cap must change the admitted set: {} against {}",
        capped.len(),
        default_layer.len()
    );

    // Dropping a block hands its area back to the fine grid, so routes must stay
    // feasible with the cap in place.
    let mut config = SimulationConfig::deterministic();
    config.coarse = CoarseOptions {
        max_size_m: 12.0,
        ..Default::default()
    };
    let output = Simulator::builder()
        .map(MapSource::bytes(image))
        .person(PersonParams::preset(Preset::Moderate))
        .standard(StandardRequest::new(
            DVec2::new(30.0, 100.0),
            DVec2::new(260.0, 100.0),
        ))
        .config(config)
        .seed(7)
        .build()
        .expect("simulator")
        .run()
        .expect("run");
    for window in output.trajectory.path.points().windows(2) {
        assert!(
            environment.hard.segment_is_clear(window[0], window[1]),
            "the capped route crosses forbidden ground"
        );
    }
    for sample in &output.trajectory.samples {
        assert!(
            environment.hard.is_passable(sample.position),
            "the capped trajectory left passable ground"
        );
    }
}

#[test]
fn the_layer_can_be_turned_off() {
    // The switch has to reach the graph: with the layer disabled the fine grid
    // describes the whole map again.
    let (image, map) = load(SyntheticMapSpec::compact());
    assert!(!environment(&map).coarse.is_empty());

    let mut config = SimulationConfig::deterministic();
    config.coarse.enabled = false;
    let environment = Environment::load_with_coarse(
        &map,
        &CostWeights::uniform(
            map.feature_schema()
                .ok()
                .flatten()
                .map(|schema| schema.dim() as usize)
                .unwrap_or(1)
                .max(1),
        ),
        &CostModelParams::default(),
        PrmOptions::None,
        config.coarse,
        None,
    )
    .expect("environment");
    assert!(environment.coarse.is_empty());

    let output = Simulator::builder()
        .map(MapSource::bytes(image))
        .person(PersonParams::preset(Preset::Moderate))
        .standard(StandardRequest::new(
            DVec2::new(30.0, 100.0),
            DVec2::new(260.0, 100.0),
        ))
        .config(config)
        .seed(3)
        .build()
        .expect("simulator")
        .run()
        .expect("run");
    assert!(!output.trajectory.samples.is_empty());
}
