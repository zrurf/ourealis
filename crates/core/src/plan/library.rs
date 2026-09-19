//! Route candidates from a stored K-path library.
//!
//! A map may carry pre-generated candidate sets for common origin/destination
//! pairs, keyed by a quantised OD cell. Using one is not simply a table lookup:
//! the stored path was generated for the *key's* endpoints, which sit up to a cell
//! away from the query, so the path has to be attached at both ends. The design
//! calls this the last-mile contract and fixes three rules, all implemented here:
//!
//! * the stored path's first and last samples are its **exact** endpoints, and the
//!   attach segments run between those and the query's own endpoints — nothing is
//!   snapped to a cell centre, which would tilt the whole route;
//! * if either endpoint is further than `d_attach` from the query, the library
//!   entry is **not** used at all: a long attach segment distorts the route more
//!   than generating it on line would;
//! * the attach segments are planned in a **local window** and their cost is added
//!   to the candidate's, so the Logit choice compares the cost of getting from
//!   *this* origin to *this* goal, not the cost of the stored path.
//!
//! The parameter set the library was generated with is compared against the live
//! one and a mismatch is reported rather than silently mixed, because the design's
//! path-frequency metric is only comparable when stored and on-line candidates
//! come from the same generator.

use glam::DVec2;

use ourealis_map_format::graph::kpath::KPathLibrary;

use crate::error::Result as CoreResult;
use crate::graph::MixedGraph;
use crate::search::SearchConfig;
use crate::search::ksp::CandidateParams;
use crate::search::logit::{Candidate, CandidateSet};
use crate::search::theta::ThetaStar;

/// Longest attach segment a library entry may need, metres.
pub const DEFAULT_D_ATTACH_M: f64 = 30.0;

/// Why a library entry was not used.
#[derive(Debug, Clone, PartialEq)]
pub enum LibraryMiss {
    /// The map carries no library.
    NoLibrary,
    /// No stored set for this OD cell.
    NoEntry,
    /// The stored endpoints are further from the query than the attach threshold.
    TooFar {
        /// Distance from the query's start to the stored path's start, metres.
        start_m: f64,
        /// Distance from the stored path's end to the query's goal, metres.
        goal_m: f64,
    },
    /// The stored set was generated with a different parameter set.
    ParametersDiffer {
        /// Identifier recorded in the library.
        stored: u32,
        /// Identifier of the live parameters.
        live: u32,
    },
}

/// Tries to build a candidate set from a stored library entry.
///
/// `start` and `goal` are the query's own endpoints (already snapped to passable
/// ground). `beta` is the individual's rationality temperature.
#[allow(clippy::too_many_arguments)]
pub fn from_library(
    library: Option<&KPathLibrary>,
    graph: &mut MixedGraph<'_>,
    start: DVec2,
    goal: DVec2,
    person_beta: f64,
    params: &CandidateParams,
    search: &SearchConfig,
    d_attach_m: f64,
) -> crate::error::Result<Result<CandidateSet, LibraryMiss>> {
    let Some(library) = library else {
        return Ok(Err(LibraryMiss::NoLibrary));
    };
    let live = crate::search::ksp::param_set_id(params);
    if library.params.param_set_id != live {
        tracing::warn!(
            "the map's candidate library was built with parameter set {} and this run uses {}; \
             generating the route on line, because the path-frequency metric is only comparable \
             when both come from the same generator",
            library.params.param_set_id,
            live
        );
        return Ok(Err(LibraryMiss::ParametersDiffer {
            stored: library.params.param_set_id,
            live,
        }));
    }
    let Some(paths) = library.find(
        library.params.key_of(start.x, start.y),
        library.params.key_of(goal.x, goal.y),
    ) else {
        return Ok(Err(LibraryMiss::NoEntry));
    };
    if paths.is_empty() {
        return Ok(Err(LibraryMiss::NoEntry));
    }

    // The attach threshold is the design's distortion bound. It applies to both
    // ends, and to each candidate separately: a candidate whose own endpoints are
    // far from the query is as unusable as a mismatched key.
    let mut candidates = Vec::with_capacity(paths.len());
    let mut attach_length = 0.0f64;
    for path in paths {
        let (Some(stored_start), Some(stored_end)) = (
            path.start_point(&library.nodes),
            path.end_point(&library.nodes),
        ) else {
            continue;
        };
        let stored_start = DVec2::new(stored_start[0] as f64, stored_start[1] as f64);
        let stored_end = DVec2::new(stored_end[0] as f64, stored_end[1] as f64);
        let to_start = (stored_start - start).length();
        let to_goal = (goal - stored_end).length();
        if to_start > d_attach_m || to_goal > d_attach_m {
            return Ok(Err(LibraryMiss::TooFar {
                start_m: to_start,
                goal_m: to_goal,
            }));
        }

        // Attach at both ends, in a window around the pair so the search stays
        // local: the stored path already covers the middle.
        let window = 2.0 * d_attach_m;
        let mut points = attach(graph, start, stored_start, window, search)?;
        let body: Vec<DVec2> = path
            .points(&library.nodes)
            .iter()
            .map(|point| DVec2::new(point[0] as f64, point[1] as f64))
            .collect();
        points.extend(body.iter().copied());
        let tail = attach(graph, stored_end, goal, window, search)?;
        points.extend(tail.iter().copied());
        attach_length += (stored_start - start).length() + (goal - stored_end).length();

        // The attached polyline is what would be run, so it is what gets priced;
        // the stored cost describes the stored geometry without the two attach
        // segments. A polyline whose attach segment turns out to cross a hard
        // constraint is dropped rather than priced with the stored path's number:
        // that number belongs to another route, and the candidate itself would be
        // infeasible.
        let Some(cost) = graph.polyline_cost(&points) else {
            tracing::debug!("a stored candidate's attach segment is not passable; dropped");
            continue;
        };
        let length: f64 = points
            .windows(2)
            .map(|window| (window[1] - window[0]).length())
            .sum();
        candidates.push(Candidate {
            points,
            cost_equiv_m: cost,
            length_m: length,
            path_size: path.path_size as f64,
        });
    }
    if candidates.is_empty() {
        return Ok(Err(LibraryMiss::NoEntry));
    }
    // The path-size factors came from the stored set and must be kept: the attach
    // segments only extend the same candidate routes, so the overlap relationships
    // between them — and hence `PS_j` — are unchanged (implementation doc 6.6 and
    // 12.2.3). `CandidateSet::new` would recompute them from the attached geometry,
    // so the set is built without that step.
    let set = CandidateSet {
        candidates,
        beta: person_beta.max(1e-3),
    };
    set.validate()?;
    tracing::debug!(
        "using a stored candidate set: {} path(s), {} m of attach",
        set.candidates.len(),
        attach_length
    );
    Ok(Ok(set))
}

/// Plans one attach segment, or returns the straight connection when the search
/// cannot improve on it.
fn attach(
    graph: &mut MixedGraph<'_>,
    from: DVec2,
    to: DVec2,
    window_m: f64,
    search: &SearchConfig,
) -> CoreResult<Vec<DVec2>> {
    if (from - to).length() < 1e-3 {
        return Ok(vec![from]);
    }
    let config = SearchConfig {
        search_window: Some(window_m),
        ..*search
    };
    let mut planner = ThetaStar::new(graph, config);
    match planner.plan(from, to) {
        Ok(result) => Ok(result.points),
        Err(error) => {
            // The attach segment is short and lies between two passable points; a
            // search failure here means the window is too tight or the pair really
            // is unreachable, in which case the straight line keeps the route
            // connected and the line-of-sight check downstream will reject it.
            tracing::debug!(
                "attach segment {from:?} -> {to:?} fell back to a straight line: {error}"
            );
            Ok(vec![from, to])
        }
    }
}
