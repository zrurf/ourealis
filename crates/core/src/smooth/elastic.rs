//! Elastic-band path smoothing.
//!
//! A grid search output still carries cell-sized corners. The elastic band
//! relaxes them with two forces and one constraint:
//!
//! * a **contraction** term pulling each point towards the midpoint of its
//!   neighbours, which straightens the path;
//! * an **avoidance** term pushing points away from obstacles along the
//!   distance field's gradient, gated by a safety radius;
//! * a **hard projection** after every iteration, without which neither of the
//!   first two guarantees feasibility.
//!
//! An optional cost-gradient bias lets the smoothing prefer cheaper ground
//! while it straightens, which is what makes a smoothed path slide onto a
//! sidewalk instead of hugging the kerb line.

use glam::DVec2;

use crate::error::Result;
use crate::field::sampler::CostSampler;
use crate::field::{CostField, HardMask};
use crate::path::Path;
use crate::terrain::DistanceField;

use super::project::{infeasible_segments, insert_midpoints, project_polyline};

/// Tunables of the smoothing pass.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ElasticBandConfig {
    /// Contraction weight.
    pub beta: f64,
    /// Avoidance weight.
    pub eta: f64,
    /// Safety radius kept from obstacles, in metres.
    pub safe_radius_m: f64,
    /// Maximum iterations.
    pub iterations: usize,
    /// Convergence threshold on the largest point displacement, in metres.
    pub epsilon_stop: f64,
    /// Weight of the cost-gradient bias; zero disables it.
    pub cost_bias: f64,
    /// Detour tolerance of the shortcut pass, metres.
    pub simplify_tolerance_m: f64,
    /// Longest stretch a single shortcut may replace, metres.
    pub simplify_max_span_m: f64,
    /// Upper bound on the band's point count while midpoints are inserted.
    pub max_points: usize,
}

impl Default for ElasticBandConfig {
    fn default() -> Self {
        Self {
            beta: 0.35,
            eta: 0.6,
            safe_radius_m: 0.75,
            iterations: 30,
            epsilon_stop: 0.02,
            cost_bias: 0.0,
            simplify_tolerance_m: 0.5,
            simplify_max_span_m: 4.0,
            max_points: 8192,
        }
    }
}

/// Result of a smoothing pass.
#[derive(Debug, Clone, PartialEq)]
pub struct SmoothResult {
    /// Smoothed path.
    pub path: Path,
    /// Iterations performed.
    pub iterations: usize,
    /// Largest displacement in the final iteration.
    pub final_shift_m: f64,
    /// True when the convergence threshold was met.
    pub converged: bool,
    /// Number of points the hard projection had to move on the last iteration.
    pub corrections: usize,
    /// True when every segment of the result stays on passable ground.
    pub feasible: bool,
}

/// Smooths a polyline with the elastic-band method.
///
/// Endpoints stay fixed: a smoothed path must still start and end where the
/// caller asked.
pub fn smooth_path(
    points: &[DVec2],
    cost: &CostField,
    distance_field: &DistanceField,
    hard: &HardMask,
    config: &ElasticBandConfig,
) -> Result<SmoothResult> {
    smooth_path_anchored(points, cost, distance_field, hard, config, &[])
}

/// Smooths a polyline, holding the given indices fixed.
///
/// The band's contraction term drives towards the straight chord between the
/// endpoints, and on a polyline whose vertices are tens of metres apart — which
/// is what a search returns — it will pull a requested waypoint's detour out
/// entirely within its iteration budget. A waypoint is part of what the caller
/// asked for, exactly like the two endpoints, so it has to be pinned rather than
/// merely preferred.
pub fn smooth_path_anchored(
    points: &[DVec2],
    cost: &CostField,
    distance_field: &DistanceField,
    hard: &HardMask,
    config: &ElasticBandConfig,
    anchors: &[usize],
) -> Result<SmoothResult> {
    let mut working: Vec<DVec2> = points.to_vec();
    // The anchors move with the polyline: an inserted midpoint shifts every later
    // index, and a pin that stayed on its old number would hold whatever vertex
    // inherited it.
    let mut anchors: Vec<usize> = anchors.to_vec();
    if working.len() < 3 {
        // Nothing to relax, but the pair still has to be checked: a two-point
        // span can cross a wall, and `plan::standard` ships the geometry whenever
        // `feasible` is true. The long path reaches the same check through
        // `infeasible_segments`, so the short path runs it too.
        let feasible = infeasible_segments(&working, hard).is_empty();
        return Ok(SmoothResult {
            path: Path::new(working)?,
            iterations: 0,
            final_shift_m: 0.0,
            converged: true,
            corrections: 0,
            feasible,
        });
    }

    let mut converged = false;
    let mut iterations = 0usize;
    let mut final_shift = 0.0f64;
    let mut corrections = 0usize;

    for iteration in 0..config.iterations {
        iterations = iteration + 1;
        let previous = working.clone();
        let last = working.len() - 1;

        for index in 1..last {
            if anchors.contains(&index) {
                continue;
            }
            let current = working[index];
            let midpoint = (previous[index - 1] + previous[index + 1]) * 0.5;
            let smooth = (midpoint - current) * config.beta;

            let distance = distance_field.distance_at(current);
            let avoid = if distance < config.safe_radius_m {
                let gate =
                    ((config.safe_radius_m - distance) / config.safe_radius_m).clamp(0.0, 1.0);
                // Positive gradient: away from the nearest obstacle.
                distance_field.gradient_at(current) * (config.eta * gate)
            } else {
                DVec2::ZERO
            };

            // The bias follows the *negative* gradient: the design's point is to
            // slide the band downhill in cost, towards cheaper ground, and the
            // gradient as computed points uphill.
            let bias = if config.cost_bias > 0.0 {
                cost_gradient(cost, current) * -config.cost_bias
            } else {
                DVec2::ZERO
            };

            working[index] = current + smooth + avoid + bias;
        }

        corrections = project_polyline(&mut working, hard, distance_field, config.safe_radius_m);

        // A segment may still cut a corner; adding a midpoint and iterating
        // again pulls the band around the obstacle.
        let infeasible = infeasible_segments(&working, hard);
        if !infeasible.is_empty() {
            for anchor in anchors.iter_mut() {
                *anchor += infeasible
                    .iter()
                    .filter(|segment| **segment < *anchor)
                    .count();
            }
            insert_midpoints(&mut working, &infeasible, config.max_points);
        }

        final_shift = working
            .iter()
            .zip(previous.iter())
            .map(|(a, b)| (*a - *b).length())
            .fold(0.0f64, f64::max);

        if final_shift < config.epsilon_stop && infeasible.is_empty() {
            converged = true;
            break;
        }
    }

    // A local zigzag is the one artefact the band can leave behind, and it costs
    // more downstream than it saved: the profile reads it as a near-zero-radius
    // bend. Every accepted shortcut passes the same line-of-sight test the search
    // uses, so removing them keeps the path traversable. An anchor is exempt:
    // the shortcut pass would otherwise cut the corner the anchor stands for,
    // which is the same erasure the relaxation loop is pinned against.
    let sampler = CostSampler::new(cost);
    working = simplify_spans(&working, &sampler, hard, config, &anchors);
    if working.len() < 3 {
        working = points.to_vec();
    }

    // One last check: the loop can exhaust its iteration budget while a segment
    // still cuts a corner, and a smoothed path that is not traversable would be
    // worse than an unsmoothed one. Callers use `feasible` to fall back.
    let feasible = infeasible_segments(&working, hard).is_empty();

    Ok(SmoothResult {
        path: Path::new(working)?,
        iterations,
        final_shift_m: final_shift,
        converged,
        corrections,
        feasible,
    })
}

/// Runs the shortcut pass on each span between anchors.
///
/// The pass keeps the first and last vertex of whatever it is given, so
/// simplifying span by span is what guarantees an anchored vertex survives: an
/// anchor is the end of one span and the start of the next, and no shortcut can
/// cross it.
fn simplify_spans(
    points: &[DVec2],
    sampler: &CostSampler<'_>,
    hard: &HardMask,
    config: &ElasticBandConfig,
    anchors: &[usize],
) -> Vec<DVec2> {
    let last = points.len().saturating_sub(1);
    let boundaries: Vec<usize> = anchors
        .iter()
        .copied()
        .filter(|index| *index > 0 && *index < last)
        .collect();
    if boundaries.is_empty() {
        return crate::smooth::simplify::simplify(
            points,
            sampler,
            hard,
            config.simplify_tolerance_m,
            config.simplify_max_span_m,
        );
    }
    let mut out: Vec<DVec2> = Vec::with_capacity(points.len());
    let mut from = 0usize;
    for boundary in boundaries.into_iter().chain(std::iter::once(last)) {
        let span = crate::smooth::simplify::simplify(
            &points[from..=boundary],
            sampler,
            hard,
            config.simplify_tolerance_m,
            config.simplify_max_span_m,
        );
        out.extend(span.iter().skip(usize::from(!out.is_empty())).copied());
        from = boundary;
    }
    out
}

/// Central-difference gradient of the cost field, pointing uphill in cost.
fn cost_gradient(cost: &CostField, position: DVec2) -> DVec2 {
    let step = cost.grid().resolution;
    let dx = (cost.sampled_cost_at(position + DVec2::new(step, 0.0))
        - cost.sampled_cost_at(position - DVec2::new(step, 0.0)))
        / (2.0 * step);
    let dy = (cost.sampled_cost_at(position + DVec2::new(0.0, step))
        - cost.sampled_cost_at(position - DVec2::new(0.0, step)))
        / (2.0 * step);
    let gradient = DVec2::new(dx, dy);
    if !gradient.is_finite() {
        return DVec2::ZERO;
    }
    gradient
}
