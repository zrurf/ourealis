//! K-shortest candidate generation by edge penalisation.
//!
//! The library of candidates for an origin-destination pair is produced by a
//! single algorithm used by both the preprocessor and the runtime, because the
//! evaluation metrics compare path-choice frequencies across runs that may hit
//! the stored library or generate on the fly. Two implementations would drift
//! and make those frequencies incomparable.
//!
//! The method: search the shortest path, then penalise its edges and search
//! again, repeating until K candidates exist. Candidates that duplicate an
//! earlier one, or that overlap it beyond the allowed ratio, are dropped and
//! the penalty is strengthened.

use glam::DVec2;

use ourealis_map_format::graph::kpath::KPathParams;

use crate::error::{CoreError, Result};

use super::logit::Candidate;
use super::theta::{PenaltySet, SearchConfig, ThetaStar};
use crate::graph::MixedGraph;

/// Generation parameters, mirroring the map's stored parameter block.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CandidateParams {
    /// Number of candidates to produce.
    pub k: usize,
    /// Multiplier applied to the edges of an accepted candidate.
    pub penalty_mu: f64,
    /// Reject a candidate overlapping an accepted one by more than this share.
    pub max_overlap_ratio: f64,
    /// Additional penalty steps applied before giving up.
    pub max_attempts: usize,
}

impl Default for CandidateParams {
    fn default() -> Self {
        Self {
            k: 5,
            penalty_mu: 1.6,
            max_overlap_ratio: 0.8,
            max_attempts: 3,
        }
    }
}

/// Stable identifier of a parameter combination.
///
/// Written into a stored library's header and compared on load, so a map built
/// with one set of candidate parameters is not silently mixed with a run that uses
/// another: the design's path-frequency metric is only comparable when stored and
/// on-line candidates come from the same generator. The value is derived from the
/// parameters themselves, so it changes whenever they do.
pub fn param_set_id(params: &CandidateParams) -> u32 {
    let mut hash = 0x811C_9DC5u32;
    let mut mix = |value: u32| {
        hash ^= value;
        hash = hash.wrapping_mul(0x0100_0193);
    };
    mix(params.k as u32);
    mix((params.penalty_mu * 1000.0).round() as u32);
    mix((params.max_overlap_ratio * 1000.0).round() as u32);
    mix(params.max_attempts as u32);
    hash
}

impl CandidateParams {
    /// Derives the parameters from a map's stored parameter block.
    pub fn from_stored(params: &KPathParams) -> Self {
        Self {
            k: params.k.max(1) as usize,
            penalty_mu: params.penalty_mu.max(1.0) as f64,
            max_overlap_ratio: params.max_overlap_ratio.clamp(0.1, 1.0) as f64,
            max_attempts: 3,
        }
    }

    /// Parameter hash used to detect a mismatched stored library.
    pub fn hash(&self) -> u64 {
        use std::hash::Hasher;
        let mut hasher = twox_hash::XxHash64::with_seed(0);
        hasher.write(&(self.k as u64).to_le_bytes());
        hasher.write(&self.penalty_mu.to_le_bytes());
        hasher.write(&self.max_overlap_ratio.to_le_bytes());
        hasher.finish()
    }
}

/// Generates up to `k` distinct candidates between two positions.
pub fn generate_candidates(
    graph: &mut MixedGraph<'_>,
    start: DVec2,
    goal: DVec2,
    params: &CandidateParams,
    search: SearchConfig,
) -> Result<Vec<Candidate>> {
    let mut accepted: Vec<Candidate> = Vec::new();
    let mut penalties = PenaltySet::new();
    let mut attempts = 0usize;
    let mut rejections = 0usize;
    let max_iterations = params.k * (params.max_attempts + 1);

    while accepted.len() < params.k && attempts < max_iterations {
        attempts += 1;
        let mut searcher = ThetaStar::new(graph, search);
        let Ok(result) = searcher.plan_with_penalty(start, goal, &penalties) else {
            break;
        };
        if result.points.len() < 2 {
            break;
        }

        let rejected = if accepted
            .iter()
            .any(|existing| same_polyline(&existing.points, &result.points))
        {
            // Identical node sequence: strengthen the penalty and retry.
            true
        } else {
            let overlap = accepted
                .iter()
                .map(|existing| overlap_ratio(&existing.points, &result.points))
                .fold(0.0f64, f64::max);
            overlap > params.max_overlap_ratio
        };
        if rejected {
            penalise(&mut penalties, &result.nodes, params.penalty_mu);
            rejections += 1;
            // A leg with no room for `k` mutually distinct routes — a short one
            // between two obstacles, where every alternative shares almost all of
            // its length — never satisfies the overlap limit. Past the configured
            // number of penalty steps the attempts are escalating the penalty and
            // searching further afield without ever accepting anything, so the
            // candidates found so far are returned: fewer than `k` is a fact about
            // the geometry, and the choice that follows works on any non-empty set.
            if rejections > params.max_attempts {
                break;
            }
            continue;
        }

        rejections = 0;
        // Penalise the accepted candidate before searching again: without it the
        // next search returns the same polyline, which is then rejected and
        // penalised, so every candidate after the first costs two searches.
        penalise(&mut penalties, &result.nodes, params.penalty_mu);
        accepted.push(Candidate::new(
            result.points.clone(),
            result.cost_equiv_m,
            result.length_m,
        ));
    }

    if accepted.is_empty() {
        return Err(CoreError::NoPath {
            from_x: start.x,
            from_y: start.y,
            to_x: goal.x,
            to_y: goal.y,
        });
    }
    Ok(accepted)
}

/// Applies the penalty multiplier to every edge of a searched node sequence.
fn penalise(penalties: &mut PenaltySet, nodes: &[crate::graph::NodeId], multiplier: f64) {
    for window in nodes.windows(2) {
        penalties.add(window[0], window[1], multiplier);
    }
}

/// True when two polylines visit the same sample points in the same order.
pub fn same_polyline(a: &[DVec2], b: &[DVec2]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter()
        .zip(b.iter())
        .all(|(left, right)| (*left - *right).length() <= 0.05)
}

/// Share of the shorter polyline's length that the two routes have in common.
pub fn overlap_ratio(a: &[DVec2], b: &[DVec2]) -> f64 {
    let length_a = polyline_length(a);
    let length_b = polyline_length(b);
    let shorter = length_a.min(length_b);
    if shorter <= f64::EPSILON {
        return 1.0;
    }
    let keys_b: Vec<(i64, i64, i64, i64)> = segment_keys(b);
    let mut shared = 0.0;
    for (index, key) in segment_keys(a).iter().enumerate() {
        if keys_b.contains(key) {
            shared += (a[index + 1] - a[index]).length();
        }
    }
    (shared / shorter).min(1.0)
}

fn polyline_length(points: &[DVec2]) -> f64 {
    points
        .windows(2)
        .map(|window| (window[1] - window[0]).length())
        .sum()
}

fn segment_keys(points: &[DVec2]) -> Vec<(i64, i64, i64, i64)> {
    const SCALE: f64 = 2.0;
    points
        .windows(2)
        .map(|window| {
            let a = window[0];
            let b = window[1];
            let mut key = (
                (a.x * SCALE).round() as i64,
                (a.y * SCALE).round() as i64,
                (b.x * SCALE).round() as i64,
                (b.y * SCALE).round() as i64,
            );
            if key.0 > key.2 || (key.0 == key.2 && key.1 > key.3) {
                key = (key.2, key.3, key.0, key.1);
            }
            key
        })
        .collect()
}
