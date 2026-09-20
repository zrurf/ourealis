//! Stored candidate libraries and the last-mile attach contract.
//!
//! A map may carry pre-generated candidate sets keyed by a quantised OD cell.
//! Using one is a table lookup only in appearance: the stored path was generated
//! for the key's endpoints, which sit up to a cell away from the query, so it has
//! to be attached at both ends. The design fixes the rules — exact stored
//! endpoints, a distance threshold, one parameter set for both sources — and these
//! tests pin them.

mod fixtures;

use glam::DVec2;

use ourealis_core::environment::{Environment, PrmOptions};
use ourealis_core::field::{CostModelParams, CostWeights};
use ourealis_core::graph::MixedGraph;
use ourealis_core::person::{PersonParams, Preset};
use ourealis_core::plan::library::{self, LibraryMiss};
use ourealis_core::plan::{LoopConfig, LoopRequest, plan_loop};
use ourealis_core::search::SearchConfig;
use ourealis_core::search::ksp::CandidateParams;
use ourealis_core::search::logit::CandidateSet;
use ourealis_map_format::Map;
use ourealis_map_format::graph::kpath::{KPath, KPathLibrary, KPathParams, OdEntry};
use ourealis_map_format::synthetic::{self, SyntheticMapSpec};

/// Corners of the synthetic map's ring road, which the library covers.
fn corners(spec: &SyntheticMapSpec) -> [DVec2; 4] {
    let (x0, y0) = (spec.width_m * 0.12, spec.height_m * 0.12);
    let (x1, y1) = (spec.width_m * 0.88, spec.height_m * 0.88);
    [
        DVec2::new(x0, y0),
        DVec2::new(x1, y0),
        DVec2::new(x1, y1),
        DVec2::new(x0, y1),
    ]
}

/// Loads an environment from an already-open map.
fn environment_of(map: &Map) -> Environment {
    let dimension = map
        .feature_schema()
        .ok()
        .flatten()
        .map(|schema| schema.dim() as usize)
        .unwrap_or(1)
        .max(1);
    Environment::load(
        map,
        &CostWeights::uniform(dimension),
        &CostModelParams::default(),
        PrmOptions::None,
    )
    .expect("environment")
}

/// Loads a map with the candidate library attached.
///
/// The fixture is opt-in (see `SyntheticMapSpec::with_kpath_library`), because its
/// one-candidate-per-OD sets are a contract fixture rather than a realistic cache.
fn load(spec: &SyntheticMapSpec) -> (Vec<u8>, Map, Environment) {
    let mut spec = *spec;
    spec.with_kpath_library = true;
    let spec = &spec;
    let image = synthetic::build(spec).expect("map image");
    let map = Map::from_bytes(image.clone()).expect("open");
    let dimension = map
        .feature_schema()
        .ok()
        .flatten()
        .map(|schema| schema.dim() as usize)
        .unwrap_or(1)
        .max(1);
    let environment = Environment::load(
        &map,
        &CostWeights::uniform(dimension),
        &CostModelParams::default(),
        PrmOptions::None,
    )
    .expect("environment");
    (image, map, environment)
}

fn graph<'a>(environment: &'a Environment) -> MixedGraph<'a> {
    MixedGraph::with_coarse(
        &environment.cost,
        &environment.hard,
        Some(&environment.terrain),
        environment.connectors.clone(),
        environment.prm.clone(),
        environment.coarse.clone(),
        3.0,
    )
}

/// Looks a route up in a library, with default parameters.
fn lookup(
    graph: &mut MixedGraph<'_>,
    library: Option<&KPathLibrary>,
    start: DVec2,
    goal: DVec2,
    params: &CandidateParams,
) -> Result<CandidateSetOrMiss, String> {
    lookup_within(
        graph,
        library,
        start,
        goal,
        params,
        library::DEFAULT_D_ATTACH_M,
    )
}

/// Looks a route up with an explicit attach threshold.
fn lookup_within(
    graph: &mut MixedGraph<'_>,
    library: Option<&KPathLibrary>,
    start: DVec2,
    goal: DVec2,
    params: &CandidateParams,
    d_attach_m: f64,
) -> Result<CandidateSetOrMiss, String> {
    library::from_library(
        library,
        graph,
        start,
        goal,
        100.0,
        params,
        &SearchConfig::default(),
        d_attach_m,
    )
    .map_err(|error| error.to_string())
}

type CandidateSetOrMiss = Result<ourealis_core::search::logit::CandidateSet, LibraryMiss>;

#[test]
fn the_synthetic_map_carries_a_library_the_simulator_reads() {
    let spec = SyntheticMapSpec::compact();
    let (_, map, environment) = load(&spec);
    let library = map
        .kpath_library()
        .expect("the layer reads")
        .expect("the map carries one");
    assert!(!library.nodes.is_empty(), "the library has no samples");
    assert!(
        !library.od_index.is_empty(),
        "the library has no OD entries"
    );
    assert!(
        environment.kpath.is_some(),
        "the environment should expose the library"
    );

    // The stored sets cover the ring's edges, in both directions.
    let corners = corners(&spec);
    for side in 0..4 {
        let from = corners[side];
        let to = corners[(side + 1) % 4];
        let found = library
            .find(
                library.params.key_of(from.x, from.y),
                library.params.key_of(to.x, to.y),
            )
            .unwrap_or_else(|| panic!("no stored set for side {side}"));
        assert_eq!(found.len(), 1);
        let path = &found[0];
        let start = path.start_point(&library.nodes).expect("a start sample");
        let end = path.end_point(&library.nodes).expect("an end sample");
        // The format requires the stored endpoints to be the *exact* ones the set
        // was generated for; the attach contract compares against them.
        assert!((start[0] as f64 - from.x).abs() < 1e-3);
        assert!((start[1] as f64 - from.y).abs() < 1e-3);
        assert!((end[0] as f64 - to.x).abs() < 1e-3);
        assert!((end[1] as f64 - to.y).abs() < 1e-3);
    }
}

#[test]
fn a_query_at_the_stored_endpoints_uses_the_library() {
    let spec = SyntheticMapSpec::compact();
    let (_, _, environment) = load(&spec);
    let corners = corners(&spec);
    let mut graph = graph(&environment);
    let params = CandidateParams::default();

    let stored = lookup(
        &mut graph,
        environment.kpath.as_ref(),
        corners[0],
        corners[1],
        &params,
    )
    .expect("no error")
    .expect("the library covers this OD pair");
    assert!(!stored.candidates.is_empty());
    // The candidate follows the stored edge, so its length is the edge's.
    let expected = (corners[1] - corners[0]).length();
    let length = stored.candidates[0].length_m;
    assert!(
        (length - expected).abs() < expected * 0.05,
        "the stored candidate is {length:.1} m for a {expected:.1} m edge"
    );
    // And it is priced, not free: the cost integral of the polyline.
    assert!(stored.candidates[0].cost_equiv_m > 0.0);

    // Planning the same leg end to end works with and without the library.
    let with = run(&spec, corners[0], corners[1], true);
    let without = run(&spec, corners[0], corners[1], false);
    println!(
        "corner to corner: {:.1} m with the library, {:.1} m without",
        with.route.length_m, without.route.length_m
    );
    assert!(
        (with.route.length_m - without.route.length_m).abs() < without.route.length_m * 0.25,
        "the stored route and the on-line route should describe the same journey"
    );
}

#[test]
fn a_query_too_far_from_the_stored_endpoints_is_generated_on_line() {
    // The design's distortion bound: past `d_attach` the attach segment would
    // dominate the route, so the library entry is not used at all.
    let spec = SyntheticMapSpec::compact();
    let (_, _, environment) = load(&spec);
    let corners = corners(&spec);
    let mut graph = graph(&environment);
    let params = CandidateParams::default();

    // A query inside the same OD key cell as the stored start — so the key matches
    // — but further from the stored endpoint than the threshold allows. With the
    // default 30 m threshold and a 25 m key cell the two are nearly the same
    // distance, so the threshold is tightened here to make the branch reachable;
    // that is also how a caller tunes it.
    let stored_start = corners[0];
    let off_cell_start = stored_start + DVec2::new(-6.0, -14.0);
    let library_params = environment.kpath.as_ref().expect("library").params;
    assert_eq!(
        library_params.key_of(stored_start.x, stored_start.y),
        library_params.key_of(off_cell_start.x, off_cell_start.y),
        "the two points must share an OD key for this to test the threshold"
    );

    let miss = lookup_within(
        &mut graph,
        environment.kpath.as_ref(),
        off_cell_start,
        corners[1],
        &params,
        5.0,
    )
    .expect("no error")
    .expect_err("the stored start is too far from this query");
    match miss {
        LibraryMiss::TooFar { start_m, .. } => {
            assert!(
                (10.0..20.0).contains(&start_m),
                "the reported distance {start_m} should be the gap to the stored start"
            );
        }
        other => panic!("expected a distance miss, got {other:?}"),
    }

    // The same query passes with a threshold that covers the gap.
    let inside = lookup_within(
        &mut graph,
        environment.kpath.as_ref(),
        off_cell_start,
        corners[1],
        &params,
        25.0,
    )
    .expect("no error");
    assert!(
        inside.is_ok(),
        "a 15 m gap should be attachable at a 25 m threshold: {inside:?}"
    );
}

#[test]
fn a_parameter_mismatch_falls_back_instead_of_mixing_sources() {
    // A library built with one candidate parameter set must not be mixed with a
    // run using another: the path-frequency metric would drift depending on which
    // source a query happened to hit.
    let spec = SyntheticMapSpec::compact();
    let (_, _, environment) = load(&spec);
    let corners = corners(&spec);
    let mut graph = graph(&environment);
    let altered = CandidateParams {
        k: 3,
        penalty_mu: 2.5,
        ..CandidateParams::default()
    };
    let miss = lookup(
        &mut graph,
        environment.kpath.as_ref(),
        corners[0],
        corners[1],
        &altered,
    )
    .expect("no error")
    .expect_err("a different parameter set must not match");
    match miss {
        LibraryMiss::ParametersDiffer { stored, live } => assert_ne!(stored, live),
        other => panic!("expected a parameter mismatch, got {other:?}"),
    }
}

/// Builds a library with one OD entry whose paths carry the given stored path
/// sizes; the two paths share their geometry, so a recomputation would give each
/// of them 0.5.
fn library_with_stored_sizes(
    params: KPathParams,
    start: DVec2,
    goal: DVec2,
    sizes: [f32; 2],
) -> KPathLibrary {
    let mut library = KPathLibrary {
        params,
        ..Default::default()
    };
    let mid = (start + goal) * 0.5;
    let range = library.push_nodes(&[
        [start.x as f32, start.y as f32, 0.0],
        [mid.x as f32, mid.y as f32, 0.0],
        [goal.x as f32, goal.y as f32, 0.0],
    ]);
    let paths = sizes
        .iter()
        .map(|size| KPath {
            total_cost_equiv_m: 100.0,
            length_m: (goal - start).length() as f32,
            path_size: *size,
            node_range: range,
        })
        .collect();
    library.insert_set(
        OdEntry {
            start_key: params.key_of(start.x, start.y),
            goal_key: params.key_of(goal.x, goal.y),
            set_index: 0,
        },
        paths,
    );
    library
}

#[test]
fn a_far_candidate_does_not_discard_the_rest_of_its_stored_set() {
    // The attach threshold applies to each candidate separately (implementation doc
    // 6.6). Aborting the lookup on the first candidate that is too far loses a usable
    // neighbour stored under the same OD key, and the leg is regenerated online even
    // though the library had a path for it.
    let spec = SyntheticMapSpec::compact();
    let (_, _, environment) = load(&spec);
    let corners = corners(&spec);
    let (start, goal) = (corners[0], corners[1]);
    let params = CandidateParams::default();
    // Shares the query's OD key cell but sits well beyond a tight threshold.
    let far_start = start + DVec2::new(-6.0, -14.0);
    let library_params = KPathParams {
        param_set_id: ourealis_core::search::ksp::param_set_id(&params),
        ..KPathParams::default()
    };
    assert_eq!(
        library_params.key_of(start.x, start.y),
        library_params.key_of(far_start.x, far_start.y),
        "both candidates must share the query's OD key"
    );

    let mut library = KPathLibrary {
        params: library_params,
        ..Default::default()
    };
    let near = library.push_nodes(&[
        [start.x as f32, start.y as f32, 0.0],
        [goal.x as f32, goal.y as f32, 0.0],
    ]);
    let far = library.push_nodes(&[
        [far_start.x as f32, far_start.y as f32, 0.0],
        [goal.x as f32, goal.y as f32, 0.0],
    ]);
    let path = |range, length: f64| KPath {
        total_cost_equiv_m: 100.0,
        length_m: length as f32,
        path_size: 1.0,
        node_range: range,
    };
    library.insert_set(
        OdEntry {
            start_key: library_params.key_of(start.x, start.y),
            goal_key: library_params.key_of(goal.x, goal.y),
            set_index: 0,
        },
        // The unusable candidate comes first, so a lookup that gives up on it never
        // reaches the usable one.
        vec![
            path(far, (goal - far_start).length()),
            path(near, (goal - start).length()),
        ],
    );

    let mut graph = graph(&environment);
    let set = lookup_within(&mut graph, Some(&library), start, goal, &params, 5.0)
        .expect("no error")
        .expect("the near candidate is inside the threshold");
    assert_eq!(
        set.candidates.len(),
        1,
        "only the candidate whose endpoints are close enough may be attached"
    );

    // When no candidate of the set is within range the lookup still reports the
    // distance rather than an empty set, which is what tells the caller to fall back
    // to an online route.
    let elsewhere = start + DVec2::new(2.0, -3.0);
    assert_eq!(
        library_params.key_of(elsewhere.x, elsewhere.y),
        library_params.key_of(start.x, start.y),
        "the query must still hit the stored set"
    );
    let miss = lookup_within(&mut graph, Some(&library), elsewhere, goal, &params, 1.0)
        .expect("no error")
        .expect_err("no candidate can be attached at this threshold");
    match miss {
        LibraryMiss::TooFar { start_m, .. } => assert!(
            (3.0..4.0).contains(&start_m),
            "the reported distance {start_m} should be the gap to the nearest start"
        ),
        other => panic!("expected a distance miss, got {other:?}"),
    }
}

#[test]
fn stored_path_sizes_survive_the_attach_segments() {
    // The implementation doc requires `PS_j` to come from the stored set (6.6,
    // 12.2.3): the attach segments extend the same routes, so the overlap
    // relationships do not change. The fabricated set makes the distinction
    // visible — a recomputation would flatten the two identical geometries to 0.5.
    let spec = SyntheticMapSpec::compact();
    let (_, _, environment) = load(&spec);
    let corners = corners(&spec);
    let (start, goal) = (corners[0], corners[1]);
    let params = CandidateParams::default();
    let library = library_with_stored_sizes(
        KPathParams {
            param_set_id: ourealis_core::search::ksp::param_set_id(&params),
            ..KPathParams::default()
        },
        start,
        goal,
        [0.9, 0.4],
    );

    let mut graph = graph(&environment);
    let set = lookup(&mut graph, Some(&library), start, goal, &params)
        .expect("no error")
        .expect("the fabricated entry matches the query");
    let sizes: Vec<f64> = set.candidates.iter().map(|c| c.path_size).collect();
    assert!(
        (sizes[0] - 0.9).abs() < 1e-6 && (sizes[1] - 0.4).abs() < 1e-6,
        "the stored path sizes must be kept: {sizes:?}"
    );

    // The same candidates, recomputed, are what the buggy path returned.
    let recomputed = CandidateSet::new(set.candidates.clone(), set.beta);
    assert!(
        (recomputed.candidates[0].path_size - 0.5).abs() < 1e-6,
        "the fixture must have stored factors that differ from a recomputation"
    );
}

#[test]
fn loop_planning_does_not_consult_the_library() {
    // Implementation doc 6.6: loop mode never queries the library, because its
    // halves come from the reference-point split and a stored OD set does not
    // describe their candidate semantics. This corner pair *does* have a stored
    // set, so a query would hit it if the rule were not enforced.
    let spec = SyntheticMapSpec::compact();
    let (_, _, environment) = load(&spec);
    let mut without = environment.clone();
    without.kpath = None;
    let corners = corners(&spec);
    let request = LoopRequest::new(corners[0], 1).with_reference(corners[1]);
    let person = PersonParams::preset(Preset::Moderate);
    let config = LoopConfig::default();

    let with = plan_loop(
        &environment,
        &mut graph(&environment),
        &request,
        &person,
        &config,
        17,
        0,
    )
    .expect("loop with a library");
    let bare = plan_loop(
        &without,
        &mut graph(&without),
        &request,
        &person,
        &config,
        17,
        0,
    )
    .expect("loop without a library");
    assert_eq!(
        with.path.points(),
        bare.path.points(),
        "the stored library must not influence a loop plan"
    );
}

#[test]
fn a_map_without_a_library_reports_it_rather_than_failing() {
    let spec = SyntheticMapSpec::compact();
    let image = synthetic::build(&spec).expect("map image");
    let map = Map::from_bytes(image.clone()).expect("open");
    let environment = environment_of(&map);
    assert!(map.kpath_library().expect("layer reads").is_none());
    let corners = corners(&spec);
    let mut graph = graph(&environment);
    let miss = lookup(
        &mut graph,
        environment.kpath.as_ref(),
        corners[0],
        corners[1],
        &CandidateParams::default(),
    )
    .expect("no error")
    .expect_err("no library");
    assert_eq!(miss, LibraryMiss::NoLibrary);
    // And the run still works, on line.
    let output = run(&spec, corners[0], corners[1], false);
    assert!(!output.trajectory.samples.is_empty());
}

/// Runs a corner-to-corner route, optionally stripping the library from the map.
fn run(
    spec: &SyntheticMapSpec,
    start: DVec2,
    goal: DVec2,
    with_library: bool,
) -> ourealis_core::sim::SimulationOutput {
    let mut spec = *spec;
    spec.with_kpath_library = with_library;
    let image = synthetic::build(&spec).expect("map image");
    ourealis_core::sim::Simulator::builder()
        .map(ourealis_core::sim::MapSource::bytes(image))
        .person(ourealis_core::person::PersonParams::preset(
            ourealis_core::person::Preset::Moderate,
        ))
        .standard(ourealis_core::plan::StandardRequest::new(start, goal))
        .config(ourealis_core::sim::SimulationConfig::deterministic())
        .seed(5)
        .build()
        .expect("simulator")
        .run()
        .expect("run")
}
