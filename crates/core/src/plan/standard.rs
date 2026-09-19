//! Standard route planning: start → waypoints → goal.
//!
//! Each leg is planned independently with its own candidate set and Logit draw,
//! which is what produces route diversity between individuals, and the legs are
//! then joined and smoothed as one path. The junction between two legs is where
//! a naive implementation leaves a visible kink, so the smoothing pass runs on
//! the merged polyline rather than per leg.
//!
//! Candidate generation uses one implementation for stored and on-line paths
//! ([`crate::search::ksp`]), so the path-choice statistics do not depend on
//! whether the map happened to carry a library.

use glam::DVec2;

use crate::environment::Environment;
use crate::error::Result;
use crate::graph::MixedGraph;
use crate::motion::LimitModifier;
use crate::motion::profile::StopHold;
use crate::path::Path;
use crate::person::PersonParams;
use crate::rng::{Rng, Stream};
use crate::search::SearchConfig;
use crate::search::ksp::{CandidateParams, generate_candidates};
use crate::search::logit::CandidateSet;
use crate::smooth::{ElasticBandConfig, smooth_path_anchored};

use super::request::{StandardRequest, ViaSemantics};

/// One planned leg.
#[derive(Debug, Clone, PartialEq)]
pub struct Leg {
    /// Requested start of the leg.
    pub from: DVec2,
    /// Requested end of the leg.
    pub to: DVec2,
    /// Sampled candidate paths.
    pub candidates: CandidateSet,
    /// Index of the candidate that was chosen.
    pub chosen: usize,
}

/// A planned and smoothed route.
#[derive(Debug, Clone, PartialEq)]
pub struct PlannedRoute {
    /// Smoothed path of the whole route.
    pub path: Path,
    /// Per-leg planning records.
    pub legs: Vec<Leg>,
    /// Waypoint speed modifiers for the motion stage.
    pub modifiers: Vec<LimitModifier>,
    /// Forced stops for the motion stage.
    pub stops: Vec<StopHold>,
    /// Cost of the chosen route in equivalent metres.
    pub cost_equiv_m: f64,
    /// Geometric length in metres.
    pub length_m: f64,
}

/// Tunables of route planning.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RouteConfig {
    /// Candidate generation parameters.
    pub candidates: CandidateParams,
    /// Multi-leg search parameters: `search.epsilon` is the inflation used as
    /// soon as the request carries a waypoint.
    pub search: SearchConfig,
    /// Heuristic inflation used when the request is a single leg.
    ///
    /// With one leg the search is the whole cost of planning, and the inflated
    /// heuristic costs nothing in path length: the measured campus route is the
    /// same length at 1.5 as at 1.2 while expanding fifteen times fewer nodes.
    /// With several legs every later leg is planned against the same graph and
    /// the inflation starts to bend the route — a waypointed route came out
    /// about 14 % longer at 1.5 — so multi-leg requests keep
    /// [`SearchConfig::epsilon`].
    pub single_leg_epsilon: f64,
    /// Smoothing parameters.
    pub smoothing: ElasticBandConfig,
    /// Arc-length spacing of the smoothed path, metres.
    pub sample_spacing_m: f64,
    /// Whether smoothing runs at all.
    pub smooth: bool,
    /// Longest attach segment a stored candidate path may need, metres.
    ///
    /// The design's last-mile threshold: a stored path whose endpoints are
    /// further than this from the query is not used, because the attach segment
    /// would distort the route more than generating it on line would.
    pub d_attach_m: f64,
    /// Radius a corner is rounded to where the geometry allows, metres.
    ///
    /// The motion stage estimates curvature from three consecutive samples, so a
    /// sharp vertex is a zero-radius turn there whatever the adjacent segment
    /// lengths are. The fillet spreads the turn over a metre or two of arc; see
    /// [`crate::smooth::rounding::round_corners`].
    pub corner_radius_m: f64,
}

impl Default for RouteConfig {
    fn default() -> Self {
        Self {
            candidates: CandidateParams::default(),
            search: SearchConfig::default(),
            single_leg_epsilon: 1.5,
            smoothing: ElasticBandConfig::default(),
            sample_spacing_m: 1.0,
            smooth: true,
            d_attach_m: crate::plan::library::DEFAULT_D_ATTACH_M,
            corner_radius_m: 1.5,
        }
    }
}

/// Plans a route for one individual.
///
/// The individual's rationality temperature drives the Logit draw on every leg,
/// so two people given the same start and goal can take different routes.
pub fn plan(
    environment: &Environment,
    graph: &mut MixedGraph<'_>,
    request: &StandardRequest,
    person: &PersonParams,
    config: &RouteConfig,
    seed: u64,
    individual: u32,
) -> Result<PlannedRoute> {
    plan_with_library(
        environment,
        graph,
        request,
        person,
        config,
        seed,
        individual,
        true,
    )
}

/// Plans a route, optionally without consulting the map's stored library.
///
/// Loop mode calls this with `use_library = false`: its halves come from the
/// reference-point split and a stored OD set does not describe their candidate
/// semantics, which the implementation doc makes the rule for loop mode.
#[allow(clippy::too_many_arguments)]
pub(crate) fn plan_with_library(
    environment: &Environment,
    graph: &mut MixedGraph<'_>,
    request: &StandardRequest,
    person: &PersonParams,
    config: &RouteConfig,
    seed: u64,
    individual: u32,
    use_library: bool,
) -> Result<PlannedRoute> {
    let mut rng = Rng::stream(seed, Stream::PathChoice, individual, 0);
    let points = request.legs();
    // One leg means the search is the whole cost of planning, so the inflated
    // heuristic is free; see `RouteConfig::single_leg_epsilon`. The legs below
    // all share this config, which is also what keeps a leg's stored-library
    // attach searches consistent with the on-line candidate search.
    let search = if points.len() == 2 {
        SearchConfig {
            epsilon: config.single_leg_epsilon,
            ..config.search
        }
    } else {
        config.search
    };
    let mut legs: Vec<Leg> = Vec::new();
    let mut merged: Vec<DVec2> = Vec::new();
    let mut anchors: Vec<usize> = Vec::new();
    let mut cost_total = 0.0;

    for window in points.windows(2) {
        let from = environment.snap_to_passable(window[0], 25.0)?;
        let to = environment.snap_to_passable(window[1], 25.0)?;
        // A stored set is preferred when the map carries one for this OD cell and
        // its endpoints are close enough to attach; otherwise the route is
        // generated on line with the same parameters, which is what makes the
        // path-frequency metric comparable between the two sources.
        let stored = if use_library {
            crate::plan::library::from_library(
                environment.kpath.as_ref(),
                graph,
                from,
                to,
                person.beta_logit,
                &config.candidates,
                &search,
                config.d_attach_m,
            )?
        } else {
            Err(crate::plan::library::LibraryMiss::NoLibrary)
        };
        let set = match stored {
            Ok(set) => set,
            Err(miss) => {
                if use_library {
                    tracing::debug!("planning leg on line: {miss:?}");
                }
                let candidates = generate_candidates(graph, from, to, &config.candidates, search)?;
                let set = CandidateSet::new(candidates, person.beta_logit);
                set.validate()?;
                set
            }
        };
        let chosen = set.sample(&mut rng);
        cost_total += set.candidates[chosen].cost_equiv_m;

        // The geometry keeps the snapped endpoints the search used. Substituting
        // the raw request back in would start or end the path inside a building
        // whenever the request was unusable, and the hard constraints would be
        // violated before the first metre. `Leg::from`/`to` still record what was
        // asked for, so the caller can see the difference.
        let leg_points = set.candidates[chosen].points.clone();
        if merged.is_empty() {
            merged.extend(leg_points.iter().copied());
        } else {
            merged.extend(leg_points.iter().skip(1).copied());
        }
        // The joint of two legs is a waypoint the caller asked for, so it is
        // recorded as an anchor: the band's contraction would otherwise pull the
        // detour out of the route entirely, and the shipped path would run
        // straight from start to goal with the waypoints left beside it.
        if !merged.is_empty() {
            anchors.push(merged.len() - 1);
        }
        legs.push(Leg {
            from: window[0],
            to: window[1],
            candidates: set,
            chosen,
        });
    }

    // Smoothing is a candidate rather than the answer: the passes below touch the
    // geometry again, and the final validation decides which stage is shipped.
    let smoothed: Option<Vec<DVec2>> = if config.smooth {
        let outcome = smooth_path_anchored(
            &merged,
            &environment.cost,
            &environment.distance,
            &environment.hard,
            &config.smoothing,
            &anchors,
        )?;
        if outcome.feasible {
            Some(outcome.path.points().to_vec())
        } else {
            // The search produced a traversable path; a smoothing pass that could
            // not keep it traversable is discarded rather than shipped.
            tracing::warn!(
                "elastic band did not reach a feasible path in {} iterations; using the searched path",
                outcome.iterations
            );
            None
        }
    } else {
        None
    };
    let base: &[DVec2] = smoothed.as_deref().unwrap_or(&merged);

    // Resample, then simplify once more: a shortcut taken just before resampling
    // can leave a step where it rejoins the old route, and that step is exactly
    // the kind of cusp the motion stage reads as a zero-radius turn.
    let resampled = Path::resampled(base.to_vec(), config.sample_spacing_m)?;
    let sampler = crate::field::sampler::CostSampler::new(&environment.cost);
    let cleaned = crate::smooth::simplify::simplify(
        resampled.points(),
        &sampler,
        &environment.hard,
        config.smoothing.simplify_tolerance_m,
        config.smoothing.simplify_max_span_m,
    );
    // Four passes have touched the geometry since the search cleared it, and each
    // of them can only check what it produces against a local or sampled model of
    // the environment. A hard constraint is not negotiable, so the finished path is
    // validated against the exact cell traversal here, unwinding the stages until
    // one is clear: the simplified path, then the resampled one, then the smoothed
    // one, then the searched one, which the line-of-sight test already cleared.
    let path = crate::plan::clearance::first_clear_path(
        &[
            cleaned.as_slice(),
            resampled.points(),
            base,
            merged.as_slice(),
        ],
        &environment.hard,
        "standard route",
    )?;
    // Whatever stage produced the geometry, the finished path must not double back
    // on itself: the motion stage advances arc length monotonically, so a spike of
    // a few centimetres makes the runner's position reverse while its arc keeps
    // growing, and the accelerometer reads that as hundreds of metres per second
    // squared. The shortened geometry has to be traversable as well, so it goes
    // back through the same validation, with the unshortened path as the fallback.
    let despiked = crate::smooth::simplify::remove_spikes(
        path.points(),
        &sampler,
        &environment.hard,
        crate::smooth::simplify::SPIKE_MAX_EXCURSION_M,
    );
    let path = if despiked.len() < path.points().len() {
        crate::plan::clearance::first_clear_path(
            &[despiked.as_slice(), path.points()],
            &environment.hard,
            "despiked route",
        )?
    } else {
        path
    };
    // Corners are rounded after the folds are gone: a spike is a near-reversal,
    // and a fillet cut across one would shortcut the detour instead of rounding a
    // real turn. The fillet pass tests every vertex and segment it inserts and
    // keeps the original vertex where the arc is not traversable; the exact
    // traversal check below is the same backstop the despike uses, so a rounded
    // path that still fails it is discarded rather than shipped.
    let rounded = crate::smooth::rounding::round_corners(
        path.points(),
        &sampler,
        &environment.hard,
        config.corner_radius_m,
    );
    let path = if rounded.as_slice() != path.points() {
        crate::plan::clearance::first_clear_path(
            &[rounded.as_slice(), path.points()],
            &environment.hard,
            "rounded route",
        )?
    } else {
        path
    };
    // The curvature is a three-point estimate on a resampled polyline, so it
    // carries the resampling's noise. Everything downstream reads it — the speed
    // ceiling, the lateral-acceleration clamp on the offset, the roll — and an
    // unsmoothed estimate makes the offset clamp chatter, which shows up as
    // thousands of m/s^3 of jerk in a bend. The window is short on purpose: a
    // metre of arc is one sample, so three samples suppress the noise without
    // flattening a bend the runner has to slow for.
    // The curvature is deliberately *not* low-passed here, although the design
    // suggests a short window for it. The three-point estimate over-estimates the
    // curvature of a resampled polyline at its vertices, and low-passing it raises
    // the speed ceiling in a bend — while the clamp that keeps the offset inside
    // the lateral-acceleration bound still sees the raw curvature. The two then
    // fight: the runner is allowed a speed the turn cannot support, the offset is
    // pulled in every other sample, and the measured peak jerk rises from 7.7e3 to
    // 3.0e4 m/s^3. What the window is meant to fix belongs in the path — a band
    // that rounds its corners — so `Path::with_smoothed_curvature` stays available
    // for a caller that knows its path is already smooth at that scale.
    let length_total = path.total_length();
    // Cost and length are taken from the path that will actually be run: the
    // candidate sums describe the search result, not the smoothed route.
    let route_cost = crate::graph::polyline_cost(
        &crate::field::sampler::CostSampler::new(&environment.cost),
        &environment.connectors,
        person.target_speed,
        path.points(),
    )
    .unwrap_or(cost_total);

    let mut modifiers = Vec::new();
    let mut stops = Vec::new();
    for waypoint in &request.waypoints {
        let Some(arc) = project_onto_path(&path, waypoint.position) else {
            continue;
        };
        match waypoint.semantics {
            ViaSemantics::Pass => {}
            ViaSemantics::Slow => modifiers.push(LimitModifier::new(
                (arc - waypoint.radius_m).max(0.0),
                (arc + waypoint.radius_m).min(length_total),
                ViaSemantics::Slow.speed_factor(),
            )),
            ViaSemantics::Dwell { duration_s } => stops.push(StopHold {
                s: arc,
                duration_s: duration_s.max(0.0),
            }),
        }
    }

    Ok(PlannedRoute {
        path,
        legs,
        modifiers,
        stops,
        cost_equiv_m: route_cost,
        length_m: length_total,
    })
}

/// Arc length of the closest point of a path to a position.
pub fn project_onto_path(path: &Path, position: DVec2) -> Option<f64> {
    let mut best: Option<(f64, f64)> = None;
    for (index, point) in path.points().iter().enumerate() {
        let distance = (*point - position).length();
        if best.map(|(_, value)| distance < value).unwrap_or(true) {
            best = Some((path.cumulative()[index], distance));
        }
    }
    best.map(|(arc, _)| arc)
}

/// Distance from a path to a position, used for validation.
pub fn distance_to_path(path: &Path, position: DVec2) -> f64 {
    path.points()
        .iter()
        .map(|point| (*point - position).length())
        .fold(f64::INFINITY, f64::min)
}
