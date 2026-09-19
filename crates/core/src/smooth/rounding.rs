//! Corner rounding by circular fillets.
//!
//! Search, smoothing and simplification all treat a bend as a vertex: the angle
//! between two straight segments. The motion stage does not read vertices, it
//! reads a three-point curvature estimate over consecutive samples, so a corner
//! that turns forty degrees at one sample is a turn of radius zero there — the
//! speed ceiling collapses, the lateral-offset clamp reacts to the spike, and
//! the third difference of position shows up as tens of thousands of m/s^3 of
//! jerk. A fillet spreads the same total turn over a metre or two of arc, which
//! is what the estimate needs.
//!
//! Three details decide where a fillet can go:
//!
//! * the radius is capped by the *straight runs* on either side, not by the
//!   immediately adjacent samples. Resampling leaves a short segment behind at
//!   every corner it does not land on, and capping by that leftover would shrink
//!   almost every fillet below the minimum radius — the cap has to measure the
//!   run the fillet's tangent point actually lies on;
//! * a fillet is a shortcut, so it is the one pass that can move a legal path
//!   onto forbidden ground. Every fillet is checked before it is inserted, and a
//!   corner whose fillet is not traversable keeps its original vertex;
//! * a corner inside a reversal window the motion stage turns at is left alone:
//!   the stop-and-pivot manoeuvre reads its pivot position and exit heading from
//!   the path, and reshaping the path inside that window moves both.

use glam::DVec2;

use crate::field::HardMask;
use crate::field::sampler::CostSampler;
use crate::math::{angle_of, dir_of, left_normal};
use crate::path::resample::{MIN_SEGMENT_M, turn_angle};

/// Smallest turn angle that is treated as a corner, radians (10 degrees).
///
/// A resampled straight run leaves a fraction of a degree between samples;
/// rounding those would add arc samples to every straight stretch for no
/// reduction in curvature.
const MIN_TURN_ANGLE_RAD: f64 = 10.0 * std::f64::consts::PI / 180.0;

/// Largest turn between consecutive fillet samples, radians (5 degrees).
///
/// The curvature estimate uses three consecutive samples, so the fillet's own
/// turn per sample has to be small compared with the total deflection; at five
/// degrees the estimated radius stays within a few percent of the true one.
const MAX_ARC_TURN_RAD: f64 = 5.0 * std::f64::consts::PI / 180.0;

/// Smallest fillet radius that is worth inserting, metres.
///
/// Below this the arc removes centimetres from the corner while adding samples
/// the downstream stages have to carry.
const MIN_RADIUS_M: f64 = 0.3;

/// Largest share of an adjacent straight run a fillet's tangent point may
/// consume.
///
/// Below one half, so the fillets of two corners sharing a run stay inside their
/// own halves and cannot meet or cross.
const TANGENT_SHARE: f64 = 0.45;

/// Window over which the motion stage detects a reversal, metres.
///
/// Mirrors `motion::maneuvers`'s turn detection: a heading change of
/// [`TURN_ANGLE_RAD`] across the window schedules a stop-and-pivot manoeuvre
/// whose pivot and exit heading are read from the path. A fillet inside the
/// window moves both, so corners that fall in one are left to the manoeuvre.
const TURN_WINDOW_M: f64 = 6.0;

/// Heading change across [`TURN_WINDOW_M`] that counts as a reversal, radians.
///
/// The threshold is the motion stage's own, so the two cannot drift apart.
const TURN_ANGLE_RAD: f64 =
    crate::motion::maneuvers::TURN_DETECTION_ANGLE_DEG * std::f64::consts::PI / 180.0;

/// Rounds the corners of a path with circular fillets.
///
/// Every interior vertex whose turn exceeds the threshold is replaced by an arc
/// of `radius_m`, clamped so its tangent points stay within the adjacent
/// straight runs and, for turns sharper than a right angle, shrunk so the
/// tangent does not run past the neighbouring corner. A corner whose fillet
/// would leave passable ground keeps its original vertex; the endpoints are
/// never moved.
pub fn round_corners(
    points: &[DVec2],
    sampler: &CostSampler<'_>,
    hard: &HardMask,
    radius_m: f64,
) -> Vec<DVec2> {
    if points.len() < 3 || !radius_m.is_finite() || radius_m < MIN_RADIUS_M {
        return points.to_vec();
    }
    let cumulative = crate::path::cumulative_lengths(points);
    let corner = is_corner(points);
    let run_end = runs_end(&cumulative, &corner);
    let reversals = reversal_centres(points, &cumulative);
    // Index of the last corner that was kept. A corner that has just been
    // rounded is no longer a direction change, so the next fillet measures its
    // incoming run from here rather than from the corner that disappeared.
    let mut run_anchor = 0usize;
    // Arc length of the last committed point. The incoming tangent of a fillet
    // may not run back past it: the pending mechanism skips the points a fillet
    // swallows, so a tangent point behind this arc would emit a segment that
    // doubles back on the arc before it.
    let mut committed_arc = 0.0f64;

    let mut out: Vec<DVec2> = Vec::with_capacity(points.len());
    out.push(points[0]);
    // Input points after the last committed one, held back because the next
    // fillet may swallow them; they are flushed at the next rejected corner.
    let mut pending: Vec<usize> = Vec::new();
    // Input points below this arc length lie behind a fillet's end tangent point
    // and are dropped rather than emitted behind the arc.
    let mut skip_before = f64::NEG_INFINITY;

    for index in 1..points.len() - 1 {
        if !corner[index] {
            if cumulative[index] >= skip_before {
                pending.push(index);
            }
            continue;
        }
        if reversals
            .iter()
            .any(|centre| (cumulative[index] - centre).abs() <= TURN_WINDOW_M)
        {
            out.extend(pending.drain(..).map(|point| points[point]));
            out.push(points[index]);
            run_anchor = index;
            committed_arc = cumulative[index];
            continue;
        }
        let incoming_delta = points[index] - points[index - 1];
        let outgoing_delta = points[index + 1] - points[index];
        let length_prev = incoming_delta.length();
        let length_next = outgoing_delta.length();
        let turn = turn_angle(incoming_delta, outgoing_delta);
        let angle = turn.abs();
        // The radius is capped by the straight runs, not by the immediately
        // adjacent samples: resampling leaves a short segment behind at every
        // corner it does not land on, and capping by that leftover would shrink
        // almost every fillet below the minimum. `tan(angle / 2)` is the tangent
        // length per unit radius and passes one at a right angle, so the share
        // alone only keeps the tangent inside the runs up to 90 degrees; a
        // sharper turn shrinks the radius instead of running past the neighbour.
        let run_prev = cumulative[index] - cumulative[run_anchor];
        let run_next = cumulative[run_end[index]] - cumulative[index];
        let cap = TANGENT_SHARE * run_prev.min(run_next);
        let radius = if length_prev < MIN_SEGMENT_M || length_next < MIN_SEGMENT_M {
            0.0
        } else {
            radius_m.min(cap).min(cap / (angle * 0.5).tan())
        };
        let tangent = radius * (angle * 0.5).tan();
        if radius < MIN_RADIUS_M || cumulative[index] - tangent < committed_arc {
            out.extend(pending.drain(..).map(|point| points[point]));
            out.push(points[index]);
            run_anchor = index;
            committed_arc = cumulative[index];
            continue;
        }
        let incoming = incoming_delta / length_prev;
        let outgoing = outgoing_delta / length_next;
        let start = points[index] - incoming * tangent;
        let end = points[index] + outgoing * tangent;
        // The centre lies on the side the path turns towards, at distance
        // `radius` from both tangent points by construction.
        let side = if turn > 0.0 {
            left_normal(incoming)
        } else {
            -left_normal(incoming)
        };
        let center = start + side * radius;
        // The arc sweeps the deflection angle, and the sign of the sweep is the
        // sign of the turn, so the sampled angle runs from one tangent point to
        // the other around the centre.
        let start_angle = angle_of(start - center);
        let segments = ((angle / MAX_ARC_TURN_RAD).ceil() as usize).max(1);
        let mut arc: Vec<DVec2> = Vec::with_capacity(segments + 1);
        arc.push(start);
        for step in 1..segments {
            let theta = start_angle + turn * step as f64 / segments as f64;
            arc.push(center + dir_of(theta) * radius);
        }
        // The end tangent point is taken from the segment, not recomputed from
        // the sweep angle, so the arc joins the outgoing run exactly.
        arc.push(end);

        // The pending points closer to the corner than the tangent are the ones
        // the fillet replaces. `pending` runs from far to near, so the points at
        // or beyond the tangent are its prefix; `from` is the last of them and
        // the arc's incoming connection is checked against it.
        let keep_from =
            pending.partition_point(|point| cumulative[index] - cumulative[*point] >= tangent);
        let from = if keep_from > 0 {
            points[pending[keep_from - 1]]
        } else {
            *out.last().unwrap_or(&points[0])
        };
        // The outgoing connection is the run up to the next corner: whatever
        // point after the arc is emitted first, it lies on that run.
        let after = points[run_end[index]];
        if fillet_is_clear(&arc, from, after, sampler, hard) {
            out.extend(pending[..keep_from].iter().map(|point| points[*point]));
            out.extend(arc);
            pending.clear();
            skip_before = cumulative[index] + tangent;
            committed_arc = cumulative[index] + tangent;
        } else {
            out.extend(pending.drain(..).map(|point| points[point]));
            out.push(points[index]);
            run_anchor = index;
            committed_arc = cumulative[index];
        }
    }
    out.extend(pending.into_iter().map(|point| points[point]));
    out.push(points[points.len() - 1]);
    out
}

/// Arc length of every reversal the motion stage's window test will find.
///
/// The scan mirrors `motion::maneuvers::detect_turns`; it exists so this pass can
/// leave the geometry the manoeuvre is read from alone.
fn reversal_centres(points: &[DVec2], cumulative: &[f64]) -> Vec<f64> {
    let total = *cumulative.last().unwrap_or(&0.0);
    let mut centres = Vec::new();
    if total <= TURN_WINDOW_M * 2.0 {
        return centres;
    }
    let step = (TURN_WINDOW_M / 8.0).max(0.25);
    let mut s = step;
    while s < total - TURN_WINDOW_M {
        let before = heading_at(points, cumulative, s - TURN_WINDOW_M);
        let after = heading_at(points, cumulative, s + TURN_WINDOW_M);
        if crate::math::angle_difference(after, before).abs() >= TURN_ANGLE_RAD {
            centres.push(s);
            s += TURN_WINDOW_M * 4.0;
            continue;
        }
        s += step;
    }
    centres
}

/// Tangent angle of the polyline at arc length `s`.
fn heading_at(points: &[DVec2], cumulative: &[f64], s: f64) -> f64 {
    let clamped = s.clamp(0.0, *cumulative.last().unwrap_or(&0.0));
    let index = cumulative.partition_point(|value| *value <= clamped);
    let index = index.saturating_sub(1).min(points.len().saturating_sub(2));
    crate::math::angle_of(points[index + 1] - points[index])
}

/// True for the interior vertices at which the direction changes by more than
/// the corner threshold.
fn is_corner(points: &[DVec2]) -> Vec<bool> {
    let mut corner = vec![false; points.len()];
    for index in 1..points.len() - 1 {
        let turn = turn_angle(
            points[index] - points[index - 1],
            points[index + 1] - points[index],
        );
        corner[index] = turn.abs() > MIN_TURN_ANGLE_RAD;
    }
    corner
}

/// Index of the next corner (or endpoint) of every vertex.
///
/// The stretch between a corner and this entry is the straight run its outgoing
/// fillet may use: every turn inside it is below the corner threshold.
fn runs_end(cumulative: &[f64], corner: &[bool]) -> Vec<usize> {
    let count = cumulative.len();
    let mut end = vec![count - 1; count];
    let mut anchor = count - 1;
    for index in (0..count - 1).rev() {
        end[index] = anchor;
        if corner[index] {
            anchor = index;
        }
    }
    end
}

/// True when every vertex and segment of a fillet stays on legal ground.
///
/// `from` is the last point before the arc and `to` the next corner: the
/// incoming connection and the arc are checked segment by segment, and `to`
/// covers the straight connection over the run points the fillet swallows. The
/// points beyond it keep their original, upstream-checked segments.
fn fillet_is_clear(
    arc: &[DVec2],
    from: DVec2,
    to: DVec2,
    sampler: &CostSampler<'_>,
    hard: &HardMask,
) -> bool {
    if !arc.iter().all(|point| hard.is_passable(*point)) {
        return false;
    }
    let mut previous = from;
    for point in arc {
        if !sampler.is_clear(previous, *point) {
            return false;
        }
        previous = *point;
    }
    sampler.is_clear(previous, to)
}
