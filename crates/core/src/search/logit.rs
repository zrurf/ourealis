//! Discrete path choice.
//!
//! Real runners do not all take the theoretical optimum. The choice among the
//! K candidate paths follows a Logit model with a path-size correction: without
//! it, a set of near-identical routes would each carry the same probability and
//! the model would double-count the same corridor.
//!
//! Costs are equivalent metres and `beta` is a rationality temperature in the
//! same unit, so the probabilities stay meaningful across maps and modes.

use ourealis_map_format::graph::kpath::{KPath, KPathLibrary};

use crate::error::{CoreError, Result};
use crate::rng::Rng;

/// One candidate path with its cost accounting.
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    /// Positions from start to goal.
    pub points: Vec<glam::DVec2>,
    /// Total cost in equivalent metres, attach segments included.
    pub cost_equiv_m: f64,
    /// Geometric length in metres.
    pub length_m: f64,
    /// Path-size factor penalising overlap with the other candidates.
    pub path_size: f64,
}

impl Candidate {
    /// Creates a candidate with a path-size factor of one.
    pub fn new(points: Vec<glam::DVec2>, cost_equiv_m: f64, length_m: f64) -> Self {
        Self {
            points,
            cost_equiv_m,
            length_m,
            path_size: 1.0,
        }
    }
}

/// Candidate set of one origin-destination pair.
#[derive(Debug, Clone, PartialEq)]
pub struct CandidateSet {
    /// Candidates in generation order; the first is the shortest.
    pub candidates: Vec<Candidate>,
    /// Rationality temperature used for the choice probabilities.
    pub beta: f64,
}

impl CandidateSet {
    /// Builds a set, computing the path-size factors.
    pub fn new(candidates: Vec<Candidate>, beta: f64) -> Self {
        let mut set = Self {
            candidates,
            beta: beta.max(1e-3),
        };
        set.recompute_path_sizes();
        set
    }

    /// Recomputes the path-size factor of every candidate.
    ///
    /// The factor is the length-weighted share of each edge that is not shared
    /// with the other candidates, which is the standard correction for
    /// overlapping routes.
    pub fn recompute_path_sizes(&mut self) {
        let count = self.candidates.len();
        if count <= 1 {
            for candidate in self.candidates.iter_mut() {
                candidate.path_size = 1.0;
            }
            return;
        }
        // Discretised edge keys: consecutive point pairs rounded to 0.5 m, which
        // makes geometric overlap visible without storing edge ids.
        let keys: Vec<Vec<(i64, i64, i64, i64)>> = self
            .candidates
            .iter()
            .map(|candidate| edge_keys(&candidate.points))
            .collect();

        for (index, candidate) in self.candidates.iter_mut().enumerate() {
            let total_length = candidate.length_m.max(1e-6);
            let mut factor = 0.0;
            for (position, key) in keys[index].iter().enumerate() {
                let appearances = keys
                    .iter()
                    .filter(|other| other.contains(key))
                    .count()
                    .max(1);
                let segment_length = if position + 1 < candidate.points.len() {
                    (candidate.points[position + 1] - candidate.points[position]).length()
                } else {
                    0.0
                };
                factor += (segment_length / total_length) / appearances as f64;
            }
            candidate.path_size = factor.clamp(1e-3, 1.0);
        }
    }

    /// Number of candidates.
    pub fn len(&self) -> usize {
        self.candidates.len()
    }

    /// True when the set is empty.
    pub fn is_empty(&self) -> bool {
        self.candidates.is_empty()
    }

    /// Choice probabilities, in candidate order.
    pub fn probabilities(&self) -> Vec<f64> {
        if self.candidates.is_empty() {
            return Vec::new();
        }
        let max_cost = self
            .candidates
            .iter()
            .map(|candidate| candidate.cost_equiv_m)
            .fold(f64::INFINITY, f64::min);
        let weights: Vec<f64> = self
            .candidates
            .iter()
            .map(|candidate| {
                let utility = -(candidate.cost_equiv_m - max_cost) / self.beta;
                candidate.path_size * utility.exp()
            })
            .collect();
        let sum: f64 = weights.iter().sum();
        if sum <= f64::EPSILON {
            let uniform = 1.0 / weights.len() as f64;
            return vec![uniform; weights.len()];
        }
        weights.into_iter().map(|weight| weight / sum).collect()
    }

    /// Draws a candidate index from the choice distribution.
    pub fn sample(&self, rng: &mut Rng) -> usize {
        let probabilities = self.probabilities();
        if probabilities.is_empty() {
            return 0;
        }
        let draw = rng.uniform();
        let mut cumulative = 0.0;
        for (index, probability) in probabilities.iter().enumerate() {
            cumulative += *probability;
            if draw <= cumulative {
                return index;
            }
        }
        probabilities.len() - 1
    }

    /// Converts a stored library set into candidates.
    ///
    /// `beta` is the individual's rationality temperature rather than a
    /// constant: the design requires the stored and the on-line candidate sets
    /// to be scored with one parameter set, or the path-frequency metric drifts
    /// depending on whether a query happened to hit the library.
    pub fn from_library(library: &KPathLibrary, paths: &[KPath], beta: f64) -> Self {
        let candidates = paths
            .iter()
            .map(|path| Candidate {
                points: path
                    .points(&library.nodes)
                    .iter()
                    .map(|point| glam::DVec2::new(point[0] as f64, point[1] as f64))
                    .collect(),
                cost_equiv_m: path.total_cost_equiv_m as f64,
                length_m: path.length_m as f64,
                path_size: path.path_size as f64,
            })
            .collect();
        Self {
            candidates,
            beta: beta.max(1e-6),
        }
    }

    /// Validates the candidate set before it is used.
    pub fn validate(&self) -> Result<()> {
        for candidate in &self.candidates {
            if candidate.points.len() < 2 {
                return Err(CoreError::config(
                    "a candidate path needs at least two sample points",
                ));
            }
            if !candidate.cost_equiv_m.is_finite() {
                return Err(CoreError::config("candidate cost is not finite"));
            }
        }
        Ok(())
    }
}

/// Rounded keys of the segments of a polyline.
fn edge_keys(points: &[glam::DVec2]) -> Vec<(i64, i64, i64, i64)> {
    const SCALE: f64 = 2.0; // half-metre resolution
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
            // Order-independent so the same segment scores as shared in both
            // travel directions.
            if key.0 > key.2 || (key.0 == key.2 && key.1 > key.3) {
                key = (key.2, key.3, key.0, key.1);
            }
            key
        })
        .collect()
}
