//! Final clearance validation of a planned path.
//!
//! The search clears every edge against the hard constraints, but the smoothing,
//! resampling and shortcut passes that follow each work from a local or sampled
//! model of the environment, and any of them can leave a segment that clips a
//! forbidden cell. A hard constraint is not negotiable, so the planner validates
//! the finished path against the exact cell traversal and falls back through the
//! stages until one is clear.

use glam::DVec2;

use crate::error::{CoreError, Result};
use crate::field::HardMask;
use crate::path::Path;

/// Index of the first candidate whose every segment stays on passable ground.
fn first_clear(candidates: &[&[DVec2]], hard: &HardMask) -> Option<usize> {
    candidates.iter().position(|points| {
        points.len() >= 2
            && points
                .windows(2)
                .all(|window| hard.segment_is_clear(window[0], window[1]))
    })
}

/// The highest-fidelity candidate that is still traversable.
///
/// `candidates` are ordered from most processed to least, and the returned path is
/// the first one whose segments all stay on passable ground. A later, less
/// processed candidate is preferable to shipping a path across a wall: an
/// unsmoothed path costs a little realism, an infeasible one is simply wrong.
pub fn first_clear_path(candidates: &[&[DVec2]], hard: &HardMask, what: &str) -> Result<Path> {
    let chosen = first_clear(candidates, hard).ok_or_else(|| {
        let detail: Vec<String> = candidates
            .iter()
            .enumerate()
            .map(|(index, points)| {
                let blocking: Vec<String> = points
                    .windows(2)
                    .filter(|window| !hard.segment_is_clear(window[0], window[1]))
                    .map(|window| {
                        format!(
                            "({:.2},{:.2})->({:.2},{:.2})",
                            window[0].x, window[0].y, window[1].x, window[1].y
                        )
                    })
                    .collect();
                let forbidden_vertices = points.iter().filter(|p| hard.is_forbidden(**p)).count();
                format!(
                    "candidate {index}: {} point(s), {} blocked segment(s) {:?},                      {forbidden_vertices} forbidden vertex/vertices",
                    points.len(),
                    blocking.len(),
                    blocking
                )
            })
            .collect();
        CoreError::config(format!(
            "no traversable {what} was produced: every candidate leaves passable ground ({})",
            detail.join("; ")
        ))
    })?;
    if chosen > 0 {
        tracing::warn!(
            "{what}: the processed path was not traversable, falling back to candidate {chosen} \
             of {}",
            candidates.len()
        );
    }
    Path::new(candidates[chosen].to_vec())
}
