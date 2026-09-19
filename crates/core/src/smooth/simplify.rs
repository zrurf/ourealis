//! Path simplification by feasibility-checked shortcuts.
//!
//! The elastic band can leave a local zigzag: two adjacent points pushed to
//! opposite sides of a narrow gap by the projection step. A zigzag is worse than
//! useless downstream — the speed profile reads it as near-zero-radius curvature
//! and crawls through it, and the gyroscope sees a pair of opposite yaw spikes.
//!
//! **What separates a zigzag from a curve** is the direction of travel on either
//! side of it: a zigzag leaves and re-enters the same direction, a curve does not.
//! A shortcut is therefore accepted only when the direction into it and the
//! direction out of it agree. Without that test the greedy longest-span shortcut
//! chords every bend — the chord deviates from the arc by a few centimetres, which
//! the deviation bound allows, but the curvature signal is replaced by a spike at
//! each corner: a 5 m-radius bend came out as a chain of 4 m segments turning 46°
//! at every vertex, and the runner braked at each one.
//!
//! The repair is the same primitive the search uses: from each point, extend a
//! straight line as far as the cost field allows and drop everything in between.
//! An accepted shortcut stays on passable ground, because the sampler's
//! clearance test is the search's hard-constraint test.
//!
//! Unlike the search's line-of-sight test, this pass does not consult the
//! connector footprints: a shortcut may cut across a stairwell's planar
//! footprint. The effect is bounded — the path was routed through the connector
//! by the search, so the shortcut only shaves the few metres near its ends —
//! but it is a difference from the search's notion of visibility.

use glam::DVec2;

use crate::field::HardMask;
use crate::field::sampler::CostSampler;

/// Cosine of the largest direction change a shortcut may bridge.
///
/// Fifteen degrees. Below it the shortcut is removing a wiggle; above it the path
/// is turning and the vertices are carrying that turn.
const MAX_DIRECTION_CHANGE_COS: f64 = 0.966;

/// Removes points that can be bypassed while staying on passable ground.
///
/// `tolerance_m` keeps the simplified path close to the original: a shortcut is
/// taken only when every vertex it replaces lies within this distance of the
/// chord. Comparing the chord against the replaced arc length cannot bound the
/// deviation — a straight chord is never longer than the polyline it spans — so
/// the tolerance is applied to the largest vertex-to-chord distance instead.
/// `max_span_m` bounds how much path a single shortcut may replace. Removing a
/// short detour removes a zigzag; removing a long one replaces a smooth curve
/// with a corner, which costs more downstream than it saves.
pub fn simplify(
    points: &[DVec2],
    sampler: &CostSampler<'_>,
    hard: &HardMask,
    tolerance_m: f64,
    max_span_m: f64,
) -> Vec<DVec2> {
    if points.len() < 3 {
        return points.to_vec();
    }
    let mut out: Vec<DVec2> = Vec::with_capacity(points.len());
    out.push(points[0]);

    // Arc lengths of the input, to bound how much path one shortcut may replace.
    let cumulative = crate::path::cumulative_lengths(points);
    let mut index = 0usize;
    while index + 1 < points.len() {
        let mut best = index + 1;
        // Direction of travel entering this point and leaving the candidate; a
        // shortcut that reverses either one is bending the path, not cleaning it.
        let incoming = if index > 0 {
            (points[index] - points[index - 1]).normalize_or_zero()
        } else {
            DVec2::ZERO
        };
        // Scan forward, keeping the furthest reachable point. The scan is bounded
        // so a long straight path does not turn into a quadratic pass.
        let limit = (index + 1 + 64).min(points.len() - 1);
        for candidate in (index + 1)..=limit {
            if !hard.is_passable(points[candidate]) {
                break;
            }
            let along = cumulative[candidate] - cumulative[index];
            if along <= 0.0 {
                continue;
            }
            if along > max_span_m {
                break;
            }
            let outgoing = if candidate + 1 < points.len() {
                (points[candidate + 1] - points[candidate]).normalize_or_zero()
            } else {
                DVec2::ZERO
            };
            let shape_kept = incoming == DVec2::ZERO
                || outgoing == DVec2::ZERO
                || incoming.dot(outgoing) >= MAX_DIRECTION_CHANGE_COS;
            // The shortcut may not leave the geometry it replaces: the widest gap
            // between the chord and the bypassed vertices is what a downstream
            // corner reads, and it is the only measure the tolerance bounds
            // meaningfully. Chord-versus-arc length cannot reject anything, since
            // a chord is never longer than the polyline it spans.
            let deviation = max_deviation(points, index, candidate);
            let detour_ok = deviation <= tolerance_m;
            if shape_kept && detour_ok && sampler.is_clear(points[index], points[candidate]) {
                best = candidate;
            } else if !detour_ok {
                // The deviation grows with every vertex a shortcut swallows, so no
                // later candidate can come back under the bound; stop extending.
                break;
            }
        }
        out.push(points[best]);
        index = best;
    }

    if out.len() < 2 {
        return points.to_vec();
    }
    out
}

/// Largest distance between the chord `from -> to` and the vertices it replaces.
fn max_deviation(points: &[DVec2], from: usize, to: usize) -> f64 {
    let start = points[from];
    let chord = points[to] - start;
    let length = chord.length();
    points[from + 1..to]
        .iter()
        .map(|point| {
            if length <= 1e-9 {
                (*point - start).length()
            } else {
                ((*point - start).perp_dot(chord) / length).abs()
            }
        })
        .fold(0.0f64, f64::max)
}

/// Largest turn angle of a polyline, in radians, for diagnostics.
pub fn max_turn_angle(points: &[DVec2]) -> f64 {
    crate::path::max_turn_angle(points)
}

/// How close the two ends of a fold have to be, metres.
///
/// A fold is a stretch the path travels out and back along: its two ends sit
/// within this distance of each other while the arc between them is far longer.
pub const SPIKE_MAX_EXCURSION_M: f64 = 1.0;

/// Longest fold, in arc length, that is looked for at one end, metres.
const FOLD_SCAN_M: f64 = 12.0;

/// Removes the folds in a path: excursions it travels out and back along.
///
/// A fold is invisible on a map and ruinous in the motion stage. Arc length keeps
/// advancing through one while the position reverses, so the runner's own velocity
/// flips sign inside a single sample — an out-and-back of four metres reported an
/// instantaneous speed of 4.1 m/s at a pace of 2.2 — and the accelerometer reads
/// that as hundreds of metres per second squared.
///
/// [`simplify`] cannot remove one. Its shape test compares the direction entering a
/// shortcut with the direction leaving it, and on a fold those are opposed by
/// construction, so the shortcut that would clean it up is the one it rejects;
/// its span cap then limits how much it could have swallowed anyway. Here a fold is
/// cut whenever the closing segment is traversable — checked exactly, so a detour
/// around an obstacle, whose chord crosses it, is kept — and the arc it replaces is
/// at least twice the direct distance plus half a metre.
pub fn remove_spikes(
    points: &[DVec2],
    sampler: &CostSampler<'_>,
    hard: &HardMask,
    max_excursion_m: f64,
) -> Vec<DVec2> {
    let mut out = points.to_vec();
    if out.len() < 3 {
        return out;
    }
    let mut cumulative = crate::path::cumulative_lengths(&out);
    let mut changed = true;
    while changed {
        changed = false;
        let mut start = 0usize;
        'outer: while start + 2 < out.len() {
            let mut arc = 0.0f64;
            let mut end = start + 1;
            while end < out.len() {
                arc += out[end].distance(out[end - 1]);
                if arc > FOLD_SCAN_M {
                    break;
                }
                let direct = out[end].distance(out[start]);
                if direct < max_excursion_m
                    && arc > 2.0 * direct + 0.5
                    && hard.is_passable(out[start])
                    && hard.is_passable(out[end])
                    && sampler.is_clear(out[start], out[end])
                {
                    out.drain(start + 1..end);
                    cumulative = crate::path::cumulative_lengths(&out);
                    changed = true;
                    continue 'outer;
                }
                end += 1;
            }
            start += 1;
        }
        let _ = &cumulative;
    }
    out
}
