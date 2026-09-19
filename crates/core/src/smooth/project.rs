//! Feasibility projection for path smoothing.
//!
//! Iterative smoothing alone cannot guarantee feasibility: nothing in the
//! shrink-and-push update prevents a point from ending up inside a wall, and a
//! segment can cut a corner the points themselves avoid. The projection step is
//! what makes the smoothed path *legal*, and it runs after every iteration.

use glam::DVec2;
use smallvec::SmallVec;

use crate::field::HardMask;
use crate::terrain::DistanceField;

/// Outcome of projecting a single point.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ProjectionOutcome {
    /// Corrected position.
    pub position: DVec2,
    /// True when the point had to be moved.
    pub corrected: bool,
    /// True when the point was inside a forbidden cell.
    pub was_forbidden: bool,
}

/// Projects a point out of forbidden ground and away from obstacles.
///
/// Order matters: leaving a forbidden cell first, then enforcing the safety
/// distance, avoids pushing a point deeper into an obstacle it just left.
pub fn project_point(
    point: DVec2,
    hard: &HardMask,
    distance_field: &DistanceField,
    safe_radius_m: f64,
) -> ProjectionOutcome {
    let was_forbidden = hard.is_forbidden(point);
    let mut position = point;
    if was_forbidden {
        // Pull back to the nearest passable cell centre; a smoother iteration
        // will relax the resulting kink.
        match hard.nearest_passable(point, 64) {
            Some(target) => position = target,
            None => {
                return ProjectionOutcome {
                    position,
                    corrected: false,
                    was_forbidden: true,
                };
            }
        }
    }

    let mut corrected = was_forbidden;
    let distance = distance_field.distance_at(position);
    if distance < safe_radius_m {
        let gradient = distance_field.gradient_at(position);
        if gradient != DVec2::ZERO {
            // Push along the *positive* distance gradient, away from the
            // obstacle: the negative direction would drive the path into it.
            position += gradient * (safe_radius_m - distance);
            corrected = true;
        }
        if hard.is_forbidden(position) {
            position = hard.nearest_passable(position, 64).unwrap_or(position);
        }
    }

    ProjectionOutcome {
        position,
        corrected,
        was_forbidden,
    }
}

/// Projects every point of a polyline, keeping the endpoints fixed.
// The loop indexes the slice because it writes back through the same index.
#[allow(clippy::needless_range_loop)]
pub fn project_polyline(
    points: &mut [DVec2],
    hard: &HardMask,
    distance_field: &DistanceField,
    safe_radius_m: f64,
) -> usize {
    let mut corrections = 0usize;
    let last = points.len().saturating_sub(1);
    for index in 1..last {
        let outcome = project_point(points[index], hard, distance_field, safe_radius_m);
        if outcome.corrected {
            points[index] = outcome.position;
            corrections += 1;
        }
    }
    corrections
}

/// Indices of segments where the straight line leaves passable ground.
///
/// Uses the exact grid traversal rather than sampling: sampling has to choose a
/// step, and any step can miss a cell the segment only clips, which would report
/// a path as feasible while it crosses a wall.
pub fn infeasible_segments(points: &[DVec2], hard: &HardMask) -> SmallVec<[usize; 8]> {
    let mut out = SmallVec::new();
    for (index, window) in points.windows(2).enumerate() {
        if !hard.segment_is_clear(window[0], window[1]) {
            out.push(index);
        }
    }
    out
}

/// Inserts midpoints into the given segments.
///
/// Segments are processed from the back so earlier indices stay valid.
/// The loop walks segments back to front because each insertion shifts the
/// indices after it.
///
/// `max_points` bounds the result: when both halves of a segment keep crossing a
/// thick forbidden region, one insertion per segment per iteration doubles the
/// point count each time, and the iteration budget alone would allow an
/// unbounded allocation before it gives up.
#[allow(clippy::needless_range_loop)]
pub fn insert_midpoints(points: &mut Vec<DVec2>, segments: &[usize], max_points: usize) {
    let mut sorted: SmallVec<[usize; 8]> = segments.iter().copied().collect();
    sorted.sort_unstable_by(|a, b| b.cmp(a));
    for index in sorted {
        if points.len() >= max_points {
            return;
        }
        if index + 1 >= points.len() {
            continue;
        }
        let midpoint = (points[index] + points[index + 1]) * 0.5;
        points.insert(index + 1, midpoint);
    }
}
