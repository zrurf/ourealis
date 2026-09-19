//! Resampling and curvature estimation for polylines.

use glam::DVec2;

/// Shortest segment that carries usable shape information, metres.
pub const MIN_SEGMENT_M: f64 = 1.0e-3;

/// Largest curvature the estimator will report, per metre.
///
/// A curvature of 5 per metre is a 20 cm radius, which nothing running produces;
/// anything larger comes from a degenerate sample rather than from the path.
pub const MAX_CURVATURE: f64 = 5.0;

/// Removes duplicate and near-duplicate consecutive points.
pub fn deduplicate(points: &[DVec2], tolerance: f64) -> Vec<DVec2> {
    let mut out: Vec<DVec2> = Vec::with_capacity(points.len());
    for point in points {
        if out
            .last()
            .map(|last| (*last - *point).length() > tolerance)
            .unwrap_or(true)
        {
            out.push(*point);
        }
    }
    out
}

/// Resamples a polyline at a fixed arc-length spacing.
///
/// The original endpoints are preserved exactly; the interior points are placed
/// on the arc-length parameterisation, which is what the speed profile assumes.
pub fn resample(points: &[DVec2], spacing: f64) -> Vec<DVec2> {
    let points = deduplicate(points, MIN_SEGMENT_M);
    if points.len() < 2 || spacing <= 0.0 {
        return points;
    }
    let cumulative = cumulative_lengths(&points);
    let total = *cumulative.last().unwrap_or(&0.0);
    if total <= spacing {
        return points;
    }

    let steps = (total / spacing).ceil().max(1.0) as usize;
    let step = total / steps as f64;
    let mut out = Vec::with_capacity(steps + 1);
    out.push(points[0]);
    let mut segment = 0usize;
    for index in 1..steps {
        let target = index as f64 * step;
        while segment + 2 < points.len() && cumulative[segment + 1] < target {
            segment += 1;
        }
        let start_length = cumulative[segment];
        let end_length = cumulative[segment + 1];
        let span = (end_length - start_length).max(1e-9);
        let t = ((target - start_length) / span).clamp(0.0, 1.0);
        out.push(points[segment] + (points[segment + 1] - points[segment]) * t);
    }
    out.push(*points.last().unwrap());
    out
}

/// Arc length at each point, starting at zero.
pub fn cumulative_lengths(points: &[DVec2]) -> Vec<f64> {
    let mut out = Vec::with_capacity(points.len());
    out.push(0.0);
    let mut total = 0.0;
    for window in points.windows(2) {
        total += (window[1] - window[0]).length();
        out.push(total);
    }
    out
}

/// Total length of a polyline.
pub fn length(points: &[DVec2]) -> f64 {
    points
        .windows(2)
        .map(|window| (window[1] - window[0]).length())
        .sum()
}

/// Signed curvature at every interior point of an evenly spaced polyline.
///
/// The curvature of point `i` is the reciprocal radius of the circle through
/// its two neighbours, with the sign taken from the turn direction so that
/// positive means a left turn. The two end points inherit the value of their
/// neighbour.
pub fn curvatures(points: &[DVec2]) -> Vec<f64> {
    let n = points.len();
    if n < 3 {
        return vec![0.0; n];
    }
    let mut out = vec![0.0f64; n];
    for index in 1..n - 1 {
        let a = points[index - 1];
        let b = points[index];
        let c = points[index + 1];
        let ab = b - a;
        let bc = c - b;
        let ca = a - c;
        let (lab, lbc, lca) = (ab.length(), bc.length(), ca.length());
        // A degenerate triple — two points closer than a millimetre — makes the
        // circumscribed-circle formula divide by a near-zero product and report a
        // curvature of thousands per metre. Such a triple carries no shape
        // information, and a curvature that large would stop the runner dead.
        if lab < MIN_SEGMENT_M || lbc < MIN_SEGMENT_M || lca < MIN_SEGMENT_M {
            continue;
        }
        let twice_area = crate::math::cross2(ab, bc);
        out[index] = (2.0 * twice_area / (lab * lbc * lca)).clamp(-MAX_CURVATURE, MAX_CURVATURE);
    }
    out[0] = out[1];
    out[n - 1] = out[n - 2];
    out
}

/// Moving average over a window, used to suppress resampling noise in curvature.
pub fn smooth_scalar(values: &[f64], window: usize) -> Vec<f64> {
    crate::math::sampling::moving_average(values, window)
}

/// Signed angle between two consecutive segments, in radians.
pub fn turn_angle(previous: DVec2, next: DVec2) -> f64 {
    let a = previous.normalize_or_zero();
    let b = next.normalize_or_zero();
    if a == DVec2::ZERO || b == DVec2::ZERO {
        return 0.0;
    }
    crate::math::cross2(a, b).atan2(a.dot(b))
}

/// Largest absolute turn angle along a polyline, in radians.
pub fn max_turn_angle(points: &[DVec2]) -> f64 {
    points
        .windows(3)
        .map(|window| turn_angle(window[1] - window[0], window[2] - window[1]).abs())
        .fold(0.0f64, f64::max)
}
