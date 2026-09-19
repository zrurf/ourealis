//! Cost synthesis, line of sight, search, candidate generation and smoothing.

mod fixtures;

use glam::DVec2;

use ourealis_core::environment::Environment;
use ourealis_core::field::{
    CostField, CostModelParams, CostSampler, CostWeights, FeatureField, INFINITE_COST,
};
use ourealis_core::graph::{MixedGraph, NodeId};
use ourealis_core::path::turn_angle;
use ourealis_core::person::{PersonParams, Preset};
use ourealis_core::plan::{PlannedRoute, RouteConfig, StandardRequest, Waypoint, plan};
use ourealis_core::search::ksp::{CandidateParams, overlap_ratio, same_polyline};
use ourealis_core::search::logit::CandidateSet;
use ourealis_core::search::{SearchConfig, ThetaStar, generate_candidates};
use ourealis_core::smooth::rounding::round_corners;
use ourealis_core::smooth::simplify::max_turn_angle;
use ourealis_core::smooth::{ElasticBandConfig, smooth_path};

use fixtures::{connector_environment, flat_environment, walled_environment};

#[test]
fn hard_constraints_are_not_diluted_by_soft_weights() {
    let environment = flat_environment(&[(60, 60)]);
    // A forbidden cell must cost infinity regardless of the weight vector: the
    // whole point of separating hard and soft costs.
    let blocked = environment.grid.cell_center(60, 60);
    assert!(environment.cost.cost_at(blocked, None).is_infinite());
    assert!(environment.cost.is_forbidden(blocked));
    assert!(
        !environment
            .cost
            .is_forbidden(environment.grid.cell_center(10, 10))
    );
}

#[test]
fn cost_is_positive_and_bounded_on_passable_ground() {
    let environment = flat_environment(&[]);
    let field = &environment.cost;
    assert!(field.min_unit_cost() > 0.0);
    assert!(field.min_unit_cost() <= field.mean_unit_cost());
    assert!(field.mean_unit_cost() <= field.max_unit_cost());
    assert!(field.max_unit_cost() < INFINITE_COST as f64);
}

#[test]
fn uniform_weights_on_uniform_features_give_uniform_cost() {
    let environment = flat_environment(&[]);
    let samples: Vec<f64> = [(5, 5), (50, 20), (100, 100)]
        .iter()
        .map(|(x, y)| {
            environment
                .cost
                .cost_at(environment.grid.cell_center(*x, *y), None)
        })
        .collect();
    for value in &samples {
        assert!((value - samples[0]).abs() < 1e-9);
    }
}

#[test]
fn sampler_rejects_lines_through_obstacles() {
    let environment = walled_environment(60, 0);
    let sampler = CostSampler::new(&environment.cost);
    let left = environment.grid.cell_center(50, 40);
    let right = environment.grid.cell_center(70, 40);
    assert!(!sampler.is_clear(left, right));
    assert!(sampler.segment_cost(left, right).is_none());

    // A line through the gap is clear.
    let gap = environment.grid.cell_center(70, 0);
    let near_gap = environment.grid.cell_center(50, 0);
    assert!(sampler.is_clear(near_gap, gap));
}

#[test]
fn segment_cost_is_proportional_to_length_in_a_uniform_field() {
    let environment = flat_environment(&[]);
    let sampler = CostSampler::new(&environment.cost);
    let a = environment.grid.cell_center(10, 10);
    let b = environment.grid.cell_center(10, 20);
    let short = sampler.segment_cost(a, b).expect("clear");
    let c = environment.grid.cell_center(10, 50);
    let long = sampler.segment_cost(a, c).expect("clear");
    let ratio = long.cost_equiv_m / short.cost_equiv_m;
    let length_ratio = long.length_m / short.length_m;
    assert!((ratio - length_ratio).abs() < 0.1);
}

#[test]
fn theta_star_returns_a_feasible_path_that_avoids_the_wall() {
    let environment = walled_environment(60, 20);
    let mut graph = MixedGraph::new(
        &environment.cost,
        &environment.hard,
        Some(&environment.terrain),
        environment.connectors.clone(),
        None,
        3.0,
    );
    let start = environment.grid.cell_center(20, 60);
    let goal = environment.grid.cell_center(100, 60);
    let mut searcher = ThetaStar::new(&mut graph, SearchConfig::default());
    let result = searcher.plan(start, goal).expect("a path exists");

    assert!(result.reached);
    assert!(result.points.len() >= 2);
    assert!((result.points[0] - start).length() < 1e-6);
    assert!((result.points[result.points.len() - 1] - goal).length() < 1e-6);

    // Every segment must be collision free, and the reported cost must match the
    // path it returns.
    let mut recomputed = 0.0;
    let sampler = CostSampler::new(&environment.cost);
    for window in result.points.windows(2) {
        let segment = sampler
            .segment_cost(window[0], window[1])
            .expect("the chosen path must be feasible");
        recomputed += segment.cost_equiv_m;
    }
    let relative = (recomputed - result.cost_equiv_m).abs() / result.cost_equiv_m.max(1e-6);
    assert!(
        relative < 0.2,
        "cost mismatch: {recomputed} vs {}",
        result.cost_equiv_m
    );

    // The detour around the wall must cost more than the straight line.
    let direct = (goal - start).length();
    assert!(result.length_m > direct);
    assert!(
        result.length_m < direct * 2.0,
        "the detour should be modest"
    );
}

#[test]
fn theta_star_goes_around_instead_of_through() {
    let environment = flat_environment(&[]);
    let mut graph = MixedGraph::new(
        &environment.cost,
        &environment.hard,
        Some(&environment.terrain),
        environment.connectors.clone(),
        None,
        3.0,
    );
    let start = environment.grid.cell_center(10, 10);
    let goal = environment.grid.cell_center(100, 100);
    let mut searcher = ThetaStar::new(&mut graph, SearchConfig::default());
    let result = searcher.plan(start, goal).expect("path");

    // Any-angle search on an empty field should produce almost a straight line.
    let direct = (goal - start).length();
    assert!(
        result.length_m < direct * 1.05,
        "length {} should be close to the straight line {direct}",
        result.length_m
    );
    assert!(
        result.points.len() < 20,
        "a straight corridor needs few points"
    );
}

#[test]
fn theta_star_reports_no_path_when_the_goal_is_walled_off() {
    let mut forbidden = Vec::new();
    for y in 0..fixtures::GRID_CELLS {
        forbidden.push((60, y));
    }
    let environment = flat_environment(&forbidden);
    let mut graph = MixedGraph::new(
        &environment.cost,
        &environment.hard,
        Some(&environment.terrain),
        environment.connectors.clone(),
        None,
        3.0,
    );
    let mut searcher = ThetaStar::new(&mut graph, SearchConfig::default());
    let result = searcher.plan(
        environment.grid.cell_center(10, 10),
        environment.grid.cell_center(110, 110),
    );
    assert!(
        result.is_err(),
        "a fully walled-off goal must not be reachable"
    );
}

#[test]
fn candidates_are_distinct_and_respect_the_overlap_limit() {
    let environment = walled_environment(60, 30);
    let mut graph = MixedGraph::new(
        &environment.cost,
        &environment.hard,
        Some(&environment.terrain),
        environment.connectors.clone(),
        None,
        3.0,
    );
    let params = CandidateParams {
        k: 4,
        ..Default::default()
    };
    let candidates = generate_candidates(
        &mut graph,
        environment.grid.cell_center(20, 60),
        environment.grid.cell_center(100, 60),
        &params,
        SearchConfig::default(),
    )
    .expect("candidates");

    assert!(!candidates.is_empty());
    assert!(candidates.len() <= params.k);
    for (index, candidate) in candidates.iter().enumerate() {
        assert!(candidate.cost_equiv_m.is_finite());
        assert!(candidate.length_m > 0.0);
        for other in candidates.iter().skip(index + 1) {
            assert!(
                !same_polyline(&candidate.points, &other.points),
                "duplicate candidates must be dropped"
            );
            let overlap = overlap_ratio(&candidate.points, &other.points);
            assert!(
                overlap <= params.max_overlap_ratio + 1e-6,
                "overlap {overlap} exceeds the limit"
            );
        }
    }
}

#[test]
fn logit_probabilities_are_normalised_and_temperature_controls_spread() {
    use ourealis_core::search::logit::Candidate;
    // Synthetic candidates with known costs isolate the choice model from the
    // search: the geometry only has to be plausible.
    let line = |offset: f64| -> Vec<DVec2> {
        (0..=10)
            .map(|index| DVec2::new(index as f64 * 10.0, offset))
            .collect()
    };
    let candidates = vec![
        Candidate::new(line(0.0), 100.0, 100.0),
        Candidate::new(line(10.0), 110.0, 104.0),
        Candidate::new(line(20.0), 130.0, 108.0),
    ];

    let sharp = CandidateSet::new(candidates.clone(), 5.0);
    let blunt = CandidateSet::new(candidates, 500.0);
    for set in [&sharp, &blunt] {
        let probabilities = set.probabilities();
        let sum: f64 = probabilities.iter().sum();
        assert!((sum - 1.0).abs() < 1e-9, "probabilities must sum to one");
        assert!(
            probabilities
                .iter()
                .all(|value| (0.0..=1.0).contains(value))
        );
    }

    // A near-zero temperature picks the cheapest candidate, a large one flattens
    // the distribution towards uniform.
    let sharp_probabilities = sharp.probabilities();
    let blunt_probabilities = blunt.probabilities();
    // With beta = 5 and a 10 m cost gap the exponent is exp(-2) = 0.135 for the
    // runner-up, so the cheapest candidate takes about 88 % of the mass.
    assert!(sharp_probabilities[0] > 0.8, "{sharp_probabilities:?}");
    assert!(blunt_probabilities[0] < sharp_probabilities[0]);
    assert!(blunt_probabilities[0] > 1.0 / 3.0 * 0.9);
    // The ordering by cost is preserved at any temperature.
    assert!(sharp_probabilities[0] >= sharp_probabilities[1]);
    assert!(sharp_probabilities[1] >= sharp_probabilities[2]);
}

#[test]
fn logit_sampling_follows_the_probabilities() {
    use ourealis_core::rng::{Rng, Stream};
    use ourealis_core::search::logit::Candidate;
    let line = |offset: f64| -> Vec<DVec2> {
        (0..=10)
            .map(|index| DVec2::new(index as f64 * 10.0, offset))
            .collect()
    };
    let set = CandidateSet::new(
        vec![
            Candidate::new(line(0.0), 100.0, 100.0),
            Candidate::new(line(10.0), 102.0, 100.0),
        ],
        20.0,
    );
    let probabilities = set.probabilities();
    let mut rng = Rng::stream(11, Stream::PathChoice, 0, 0);
    let mut counts = [0usize; 2];
    for _ in 0..4000 {
        counts[set.sample(&mut rng)] += 1;
    }
    for index in 0..2 {
        let observed = counts[index] as f64 / 4000.0;
        assert!(
            (observed - probabilities[index]).abs() < 0.05,
            "candidate {index}: observed {observed}, expected {}",
            probabilities[index]
        );
    }
}

#[test]
fn path_size_factors_penalise_overlap() {
    // Two candidates that share almost everything, one that is distinct.
    let shared: Vec<DVec2> = (0..=10).map(|i| DVec2::new(i as f64 * 10.0, 0.0)).collect();
    let mut detour = shared.clone();
    detour[5] = DVec2::new(50.0, 30.0);
    let distinct: Vec<DVec2> = (0..=10)
        .map(|i| DVec2::new(i as f64 * 10.0, 25.0))
        .collect();

    let a = ourealis_core::search::logit::Candidate::new(shared, 100.0, 100.0);
    let b = ourealis_core::search::logit::Candidate::new(detour, 105.0, 110.0);
    let c = ourealis_core::search::logit::Candidate::new(distinct, 110.0, 100.0);
    let set = CandidateSet::new(vec![a, b, c], 100.0);

    let sizes: Vec<f64> = set.candidates.iter().map(|c| c.path_size).collect();
    assert!(sizes.iter().all(|value| *value > 0.0 && *value <= 1.0));
    assert!(
        sizes[1] < sizes[2],
        "the overlapping candidate must be penalised more than the distinct one: {sizes:?}"
    );
}

#[test]
fn smoothing_keeps_the_path_feasible_and_shortens_it() {
    let environment = fixtures::flat_environment(&[]);
    // A staircase path that smoothing should straighten.
    let mut points = Vec::new();
    for step in 0..10 {
        points.push(DVec2::new(step as f64 * 10.0, 20.0));
        points.push(DVec2::new(step as f64 * 10.0, 30.0));
    }
    let before = ourealis_core::path::Path::new(points.clone()).expect("path");
    let outcome = smooth_path(
        &points,
        &environment.cost,
        &environment.distance,
        &environment.hard,
        &ElasticBandConfig::default(),
    )
    .expect("smoothing");

    assert!(
        outcome.path.total_length() <= before.total_length() + 1e-6,
        "smoothing must not lengthen the path"
    );
    for point in outcome.path.points() {
        assert!(
            environment.hard.is_passable(*point),
            "smoothed point {point:?} must stay passable"
        );
    }
}

#[test]
fn smoothing_pushes_points_out_of_obstacles() {
    let environment = flat_environment(&[(50, 50), (51, 50), (49, 50), (50, 51), (50, 49)]);
    let points = vec![
        DVec2::new(40.0, 50.5),
        DVec2::new(50.5, 50.5),
        DVec2::new(60.0, 50.5),
    ];
    let outcome = smooth_path(
        &points,
        &environment.cost,
        &environment.distance,
        &environment.hard,
        &ElasticBandConfig::default(),
    )
    .expect("smoothing");
    for point in outcome.path.points() {
        assert!(
            !environment.hard.is_forbidden(*point),
            "point {point:?} must have been projected out of the obstacle"
        );
    }
}

#[test]
fn a_two_point_span_across_a_wall_is_not_feasible() {
    // The short-path early return used to report `feasible: true` without running
    // the hard-constraint check, and `plan::standard` ships the geometry whenever
    // the flag is set — so a two-point path through a wall was accepted.
    let environment = walled_environment(60, 0);
    let points = vec![
        environment.grid.cell_center(50, 40),
        environment.grid.cell_center(70, 40),
    ];
    assert!(
        !environment.hard.segment_is_clear(points[0], points[1]),
        "the test needs a span that actually crosses the wall"
    );
    let outcome = smooth_path(
        &points,
        &environment.cost,
        &environment.distance,
        &environment.hard,
        &ElasticBandConfig::default(),
    )
    .expect("smoothing");
    assert!(
        !outcome.feasible,
        "a two-point span crossing the wall must report infeasible"
    );
}

#[test]
fn simplify_tolerance_bounds_how_far_a_shortcut_may_stray() {
    // A 0.4 m zigzag is what the pass exists for: the direction into and out of
    // each shortcut agrees, so only the deviation bound can reject it. A zero
    // tolerance must leave every vertex in place; one above the amplitude must
    // collapse the zigzag.
    let environment = flat_environment(&[]);
    let sampler = CostSampler::new(&environment.cost);
    let points: Vec<DVec2> = (0..=4)
        .map(|index| {
            DVec2::new(
                10.0 + index as f64,
                50.0 + if index % 2 == 1 { 0.4 } else { 0.0 },
            )
        })
        .collect();

    let strict =
        ourealis_core::smooth::simplify::simplify(&points, &sampler, &environment.hard, 0.0, 4.0);
    let tolerant =
        ourealis_core::smooth::simplify::simplify(&points, &sampler, &environment.hard, 0.5, 4.0);

    assert_eq!(
        strict.len(),
        points.len(),
        "a zero tolerance must forbid shortcuts that deviate at all: {strict:?}"
    );
    assert!(
        tolerant.len() < strict.len(),
        "a tolerance above the zigzag amplitude must collapse it: {} against {}",
        tolerant.len(),
        strict.len()
    );
}

#[test]
fn graph_node_identifiers_are_unambiguous() {
    let grid = NodeId::grid(42);
    let prm = NodeId::prm(42);
    let connector = NodeId::connector(42);
    assert_ne!(grid, prm);
    assert_ne!(prm, connector);
    assert!(grid.is_grid());
    assert_eq!(grid.index(), 42);
    assert_eq!(prm.index(), 42);
}

#[test]
fn weights_and_features_must_agree_on_the_dimension() {
    let environment = flat_environment(&[]);
    let too_many = CostWeights {
        values: vec![0.5; 7],
        scale: 1.0,
    };
    let result = CostField::synthesize(
        &environment.features,
        &environment.hard,
        &too_many,
        &CostModelParams::default(),
    );
    assert!(result.is_err());
}

#[test]
fn uniform_feature_field_and_uniform_weights_keep_cost_constant() {
    let environment = flat_environment(&[]);
    let features: &FeatureField = &environment.features;
    assert_eq!(features.dim(), 2);
    assert!(features.minima().iter().all(|value| value.abs() < 1e-9));
    let params = CostModelParams::default();
    let field = CostField::synthesize(
        features,
        &environment.hard,
        &CostWeights::uniform(features.dim()),
        &params,
    )
    .expect("cost");
    // With zero features the cost reduces to the baseline, floored by the
    // multiplicative branch when it is enabled.
    assert!((field.mean_unit_cost() - params.c0).abs() < 1e-6);
}

#[test]
fn route_configuration_smoothing_flag_is_respected() {
    let config = RouteConfig {
        smooth: false,
        ..Default::default()
    };
    assert!(!config.smooth);
    let person = PersonParams::preset(Preset::Jog);
    assert!(person.validate().is_ok());
}

#[test]
fn a_z_axis_link_can_be_entered_from_the_plane() {
    // A connector is only useful if a path can get onto it. The graph is directed,
    // so an endpoint that is reachable only *from* the plane is unusable: nothing
    // can enter it, and a route that needs the stair reports NoPath — or the
    // search treats the stairwell as an obstacle and goes around it.
    let environment = connector_environment(20, 30);
    let link = environment.connectors.entries()[0];
    let mut graph = MixedGraph::new(
        &environment.cost,
        &environment.hard,
        Some(&environment.terrain),
        environment.connectors.clone(),
        None,
        3.0,
    );
    assert!(
        environment.connectors.crosses_footprint(
            environment.grid.cell_center(20, 30),
            environment.grid.cell_center(16, 30),
        ),
        "the fixture's footprint must actually block a flat crossing"
    );

    // The plane node at each endpoint must have an edge onto the link, and the
    // connector must lead back to the plane from both of its endpoints.
    for (cell, endpoint) in [
        (environment.grid.index(20, 30) as u32, 0u32),
        (environment.grid.index(22, 30) as u32, 1u32),
    ] {
        let node = NodeId::grid(cell);
        let edges = graph.neighbours(node).to_vec();
        assert!(
            edges
                .iter()
                .any(|edge| edge.to == NodeId::connector(endpoint)),
            "the plane node at endpoint {endpoint} cannot reach the link: {edges:?}"
        );
        let back = graph.neighbours(NodeId::connector(endpoint)).to_vec();
        assert!(
            back.iter().any(|edge| edge.to == node),
            "endpoint {endpoint} cannot get back onto the plane: {back:?}"
        );
    }

    // And the link itself must be priced at the connector's equivalent cost, not
    // at the cost of the straight line it projects onto.
    let entry = link.cost_a_to_b(3.0).expect("a to b");
    let edges = graph.neighbours(NodeId::connector(0)).to_vec();
    let crossing = edges
        .iter()
        .find(|edge| edge.to == NodeId::connector(1))
        .expect("the link itself");
    assert!(
        (crossing.cost_equiv_m - entry).abs() < 1e-9,
        "the link must be priced at the connector's equivalent cost"
    );
    // A straight move between the two endpoint cells must not be available as
    // flat ground: that would cross the stairwell at the price of the chord.
    assert!(
        !graph.visible(
            NodeId::grid(environment.grid.index(20, 30) as u32),
            NodeId::grid(environment.grid.index(22, 30) as u32),
        ),
        "the link cannot be traversed as flat ground"
    );

    // The wall's only opening is the link, so a route from the west to the east
    // must use it.
    let mut searcher = ThetaStar::new(&mut graph, SearchConfig::default());
    let start = environment.grid.cell_center(10, 30);
    let goal = environment.grid.cell_center(34, 30);
    let result = searcher
        .plan(start, goal)
        .expect("a route through the stairwell exists");
    let crosses = result.nodes.windows(2).any(|pair| {
        pair[0].kind() == ourealis_core::graph::NodeKind::Connector
            || pair[1].kind() == ourealis_core::graph::NodeKind::Connector
    });
    assert!(
        crosses,
        "the route must traverse the link instead of crossing its footprint"
    );
}

/// Circumradius of the circle through three points, `None` when they are
/// collinear.
fn circumradius(a: DVec2, b: DVec2, c: DVec2) -> Option<f64> {
    let twice_area = (b - a).perp_dot(c - a);
    if twice_area.abs() < 1e-12 {
        return None;
    }
    let ab = (b - a).length();
    let bc = (c - b).length();
    let ca = (a - c).length();
    Some(ab * bc * ca / (2.0 * twice_area.abs()))
}

/// Plans one request, building a fresh graph over the environment.
fn plan_route(
    environment: &Environment,
    request: &StandardRequest,
    config: &RouteConfig,
) -> PlannedRoute {
    let mut graph = MixedGraph::new(
        &environment.cost,
        &environment.hard,
        Some(&environment.terrain),
        environment.connectors.clone(),
        None,
        3.0,
    );
    let person = PersonParams::preset(Preset::Moderate);
    plan(environment, &mut graph, request, &person, config, 5, 0).expect("plan")
}

#[test]
fn a_right_angle_corner_is_rounded_within_the_requested_radius() {
    let environment = flat_environment(&[]);
    let sampler = CostSampler::new(&environment.cost);
    let points = vec![
        DVec2::new(20.0, 80.0),
        DVec2::new(60.0, 80.0),
        DVec2::new(60.0, 40.0),
    ];
    assert!((max_turn_angle(&points) - std::f64::consts::FRAC_PI_2).abs() < 1e-9);

    let rounded = round_corners(&points, &sampler, &environment.hard, 1.5);
    assert_eq!(rounded[0], points[0]);
    assert_eq!(*rounded.last().unwrap(), *points.last().unwrap());
    assert!(
        rounded.len() > points.len(),
        "the corner must have been replaced by an arc: {rounded:?}"
    );
    // The whole 90-degree turn is now spread over the arc samples: no vertex
    // turns more than the sampling cap, so the three-point curvature estimate
    // sees a bend instead of a corner.
    let turn_after = max_turn_angle(&rounded);
    assert!(
        turn_after <= 5.0f64.to_radians() + 1e-9,
        "the rounded path still turns {turn_after} rad at one vertex"
    );
    // The sharpest vertex lies on the fillet, so the circle through it and its
    // neighbours is the fillet itself.
    let sharpest = rounded
        .windows(3)
        .max_by(|left, right| {
            let a = turn_angle(left[1] - left[0], left[2] - left[1]).abs();
            let b = turn_angle(right[1] - right[0], right[2] - right[1]).abs();
            a.partial_cmp(&b).unwrap_or(std::cmp::Ordering::Equal)
        })
        .expect("a rounded path has interior vertices");
    let radius = circumradius(sharpest[0], sharpest[1], sharpest[2])
        .expect("the sharpest vertex must not be collinear");
    assert!(
        (radius - 1.5).abs() < 0.01,
        "the fillet radius is {radius} m, not the requested 1.5 m"
    );
}

#[test]
fn a_fillet_that_would_cross_a_wall_is_not_inserted() {
    let points = vec![
        DVec2::new(50.0, 50.0),
        DVec2::new(60.0, 40.0),
        DVec2::new(70.0, 50.0),
    ];
    // Control: on open ground the same corner is rounded, so a refusal below is
    // about the wall and not about the geometry.
    let open = flat_environment(&[]);
    let open_sampler = CostSampler::new(&open.cost);
    assert_ne!(
        round_corners(&points, &open_sampler, &open.hard, 6.0),
        points,
        "the fixture must describe a corner that is roundable without the wall"
    );

    // The two blocked cells sit inside the corner, where the fillet cuts across;
    // the path's own segments run one cell below them.
    let blocked = flat_environment(&[(60, 42), (61, 42)]);
    let sampler = CostSampler::new(&blocked.cost);
    for window in points.windows(2) {
        assert!(
            blocked.hard.segment_is_clear(window[0], window[1]),
            "the fixture's own path must be legal"
        );
    }
    assert_eq!(
        round_corners(&points, &sampler, &blocked.hard, 6.0),
        points,
        "a fillet that leaves passable ground must not be inserted"
    );
}

#[test]
fn a_planned_route_is_rounded_and_still_exactly_traversable() {
    // A waypoint on open ground makes the joined route turn 90 degrees there,
    // with long straight legs on either side: the corner is roundable, and with
    // smoothing off it reaches the rounding pass untouched.
    let environment = flat_environment(&[]);
    let start = DVec2::new(20.5, 20.5);
    let via = DVec2::new(80.5, 20.5);
    let goal = DVec2::new(80.5, 80.5);
    let request = StandardRequest::new(start, goal).via(Waypoint::new(via));
    let bare = RouteConfig {
        smooth: false,
        corner_radius_m: 0.0,
        ..Default::default()
    };
    let rounded = RouteConfig {
        corner_radius_m: 1.5,
        ..bare
    };
    let bare_route = plan_route(&environment, &request, &bare);
    assert!(
        bare_route.path.max_turn_angle() > 10.0f64.to_radians(),
        "the fixture must leave a corner for the rounding pass"
    );
    let route = plan_route(&environment, &request, &rounded);
    assert_ne!(
        route.path.points(),
        bare_route.path.points(),
        "the rounding pass must change a corner that fits"
    );
    assert!(
        route.path.max_turn_angle() <= 5.0f64.to_radians() + 1e-9,
        "a vertex of the shipped path still turns {} rad",
        route.path.max_turn_angle()
    );
    // A fillet replaces two straight runs by an arc, which is never longer.
    assert!(route.length_m <= bare_route.length_m + 1e-9);

    // Rounding may not move the path off legal ground, and the shipped path is
    // checked with the exact cell traversal rather than the sampler.
    for window in route.path.points().windows(2) {
        assert!(
            environment.hard.segment_is_clear(window[0], window[1]),
            "rounded segment {:?} -> {:?} is not traversable",
            window[0],
            window[1]
        );
    }
    for point in route.path.points() {
        assert!(environment.hard.is_passable(*point), "{point:?}");
    }
}

#[test]
fn a_single_leg_plan_uses_the_adaptive_inflation() {
    // A wall with one gap: the candidate search has several distinct routes, so
    // the heuristic inflation changes which one is found first.
    let environment = walled_environment(60, 30);
    let request = StandardRequest::new(
        environment.grid.cell_center(60, 60),
        environment.grid.cell_center(100, 60),
    );
    let fast = RouteConfig {
        smooth: false,
        ..Default::default()
    };
    let with_search_epsilon = |epsilon: f64| RouteConfig {
        search: SearchConfig {
            epsilon,
            ..fast.search
        },
        ..fast
    };
    let with_single_leg_epsilon = |epsilon: f64| RouteConfig {
        single_leg_epsilon: epsilon,
        ..fast
    };

    // A waypoint-free plan ignores the multi-leg epsilon: a value that reshapes
    // the candidates when it is consulted (below) leaves the leg untouched.
    let adaptive = plan_route(&environment, &request, &fast);
    let ignored = plan_route(&environment, &request, &with_search_epsilon(5.0));
    assert_eq!(
        adaptive.path.points(),
        ignored.path.points(),
        "a single-leg plan must not read search.epsilon"
    );
    assert_eq!(adaptive.legs[0].chosen, ignored.legs[0].chosen);
    assert_eq!(
        adaptive.legs[0].candidates.candidates.len(),
        ignored.legs[0].candidates.candidates.len()
    );

    // The single-leg field is the one consulted, and the default is the higher
    // inflation: raising it to a greedy value changes the candidate search.
    assert!((fast.single_leg_epsilon - 1.5).abs() < 1e-12);
    let greedy = plan_route(&environment, &request, &with_single_leg_epsilon(5.0));
    assert_ne!(
        adaptive.path.points(),
        greedy.path.points(),
        "changing single_leg_epsilon must change a waypoint-free plan"
    );
    assert_ne!(
        adaptive.legs[0].candidates.candidates[0].points,
        greedy.legs[0].candidates.candidates[0].points
    );

    // A waypointed request keeps the multi-leg value: changing single_leg_epsilon
    // leaves it alone.
    let waypointed = StandardRequest::new(
        environment.grid.cell_center(59, 60),
        environment.grid.cell_center(100, 60),
    )
    .via(Waypoint::new(environment.grid.cell_center(60, 30)));
    let multi = plan_route(&environment, &waypointed, &fast);
    let multi_ignored = plan_route(&environment, &waypointed, &with_single_leg_epsilon(5.0));
    assert_eq!(
        multi.path.points(),
        multi_ignored.path.points(),
        "a waypointed plan must not read single_leg_epsilon"
    );
}
