//! Interpolation, filtering and statistics helpers.

/// Linear interpolation between `a` and `b`.
#[inline]
pub fn lerp(a: f64, b: f64, t: f64) -> f64 {
    a + (b - a) * t
}

/// Smoothstep with zero first derivative at both ends.
#[inline]
pub fn smoothstep(t: f64) -> f64 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// First-order low-pass coefficient for a time step `dt` and time constant `tau`.
///
/// Returns the weight of the *new* sample, so `y += alpha * (x - y)`.
#[inline]
pub fn first_order_alpha(dt: f64, tau: f64) -> f64 {
    if tau <= f64::EPSILON {
        return 1.0;
    }
    (dt / tau).clamp(0.0, 1.0)
}

/// First-order low-pass step.
#[inline]
pub fn low_pass(previous: f64, target: f64, dt: f64, tau: f64) -> f64 {
    let alpha = first_order_alpha(dt, tau);
    previous + alpha * (target - previous)
}

/// Shifts `current` into the branch closest to `reference`.
///
/// Angles are stored unwrapped wherever a filter follows, otherwise a crossing
/// of the `-pi/pi` cut shows up as a full turn of spurious motion.
#[inline]
pub fn unwrap_angle(reference: f64, current: f64) -> f64 {
    let mut value = current;
    while value - reference > std::f64::consts::PI {
        value -= std::f64::consts::TAU;
    }
    while value - reference < -std::f64::consts::PI {
        value += std::f64::consts::TAU;
    }
    value
}

/// First-order low-pass on an angle, unwrapping the target first.
#[inline]
pub fn low_pass_angle(previous: f64, target: f64, dt: f64, tau: f64) -> f64 {
    let unwrapped = unwrap_angle(previous, target);
    low_pass(previous, unwrapped, dt, tau)
}

/// Bilinear sample of a row-major grid in index space.
///
/// Coordinates are clamped to the grid, so samples outside the array return the
/// nearest edge value instead of failing.
pub fn bilinear(values: &[f64], width: usize, height: usize, x: f64, y: f64) -> f64 {
    if width == 0 || height == 0 || values.len() < width * height {
        return 0.0;
    }
    let x = x.clamp(0.0, (width - 1) as f64);
    let y = y.clamp(0.0, (height - 1) as f64);
    let x0 = x.floor() as usize;
    let y0 = y.floor() as usize;
    let x1 = (x0 + 1).min(width - 1);
    let y1 = (y0 + 1).min(height - 1);
    let fx = x - x0 as f64;
    let fy = y - y0 as f64;

    let v00 = values[y0 * width + x0];
    let v10 = values[y0 * width + x1];
    let v01 = values[y1 * width + x0];
    let v11 = values[y1 * width + x1];
    lerp(lerp(v00, v10, fx), lerp(v01, v11, fx), fy)
}

/// Bilinear sample of a row-major `f32` grid in index space.
///
/// Fields loaded from the map are stored as `f32`; widening inside the
/// interpolation avoids a second full-resolution copy.
pub fn bilinear_f32(values: &[f32], width: usize, height: usize, x: f64, y: f64) -> f64 {
    if width == 0 || height == 0 || values.len() < width * height {
        return 0.0;
    }
    let x = x.clamp(0.0, (width - 1) as f64);
    let y = y.clamp(0.0, (height - 1) as f64);
    let x0 = x.floor() as usize;
    let y0 = y.floor() as usize;
    let x1 = (x0 + 1).min(width - 1);
    let y1 = (y0 + 1).min(height - 1);
    let fx = x - x0 as f64;
    let fy = y - y0 as f64;

    let v00 = values[y0 * width + x0] as f64;
    let v10 = values[y0 * width + x1] as f64;
    let v01 = values[y1 * width + x0] as f64;
    let v11 = values[y1 * width + x1] as f64;
    lerp(lerp(v00, v10, fx), lerp(v01, v11, fx), fy)
}

/// Centred moving average with edge clamping.
///
/// The window holds exactly `window` samples wherever the input is long enough
/// to supply them; near the edges it is clamped, so the result there is the mean
/// of however many samples are available.
pub fn moving_average(values: &[f64], window: usize) -> Vec<f64> {
    if values.is_empty() || window <= 1 {
        return values.to_vec();
    }
    let half = window / 2;
    let mut out = Vec::with_capacity(values.len());
    for index in 0..values.len() {
        let start = index.saturating_sub(half);
        let end = (start + window).min(values.len());
        let start = end.saturating_sub(window);
        let slice = &values[start..end];
        out.push(slice.iter().sum::<f64>() / slice.len() as f64);
    }
    out
}

/// Central-difference derivative with one-sided ends.
pub fn differentiate(values: &[f64], dt: f64) -> Vec<f64> {
    if values.len() < 2 || dt <= 0.0 {
        return vec![0.0; values.len()];
    }
    let mut out = Vec::with_capacity(values.len());
    out.push((values[1] - values[0]) / dt);
    for index in 1..values.len() - 1 {
        out.push((values[index + 1] - values[index - 1]) / (2.0 * dt));
    }
    out.push((values[values.len() - 1] - values[values.len() - 2]) / dt);
    out
}

/// Percentile of a sorted slice using linear interpolation between ranks.
pub fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let rank = p.clamp(0.0, 1.0) * (sorted.len() - 1) as f64;
    let low = rank.floor() as usize;
    let high = rank.ceil() as usize;
    if low == high {
        sorted[low]
    } else {
        lerp(sorted[low], sorted[high], rank - low as f64)
    }
}

/// Histogram of `values` over `[low, high)` with `bins` buckets.
///
/// Values outside the range are dropped, which keeps the tail of a
/// heavy-tailed distribution from collapsing every bucket into one.
pub fn histogram(values: &[f64], bins: usize, low: f64, high: f64) -> Vec<usize> {
    let mut out = vec![0usize; bins.max(1)];
    if bins == 0 || high <= low {
        return out;
    }
    let scale = bins as f64 / (high - low);
    for value in values {
        if *value < low || *value >= high || !value.is_finite() {
            continue;
        }
        let index = ((*value - low) * scale) as usize;
        out[index.min(bins - 1)] += 1;
    }
    out
}
