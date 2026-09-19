//! Geometry, geodesy and numerical helpers.
//!
//! World geometry uses `f64` throughout: at a million metres from the origin an
//! `f32` resolves about 0.1 m, which is coarser than the quantities derived
//! from positions. Conversion to `f32` happens only where data meets the map
//! container or a GPU buffer.

pub mod fft;
pub mod geodesy;
pub mod sampling;

pub use fft::{Fft, Spectrum, SpectrumPeak};
pub use geodesy::LocalFrame;
pub use sampling::{
    bilinear, bilinear_f32, differentiate, first_order_alpha, histogram, lerp, low_pass,
    low_pass_angle, moving_average, percentile, smoothstep, unwrap_angle,
};

pub use glam::{DMat3, DQuat, DVec2, DVec3, Vec2};

/// Mean Earth radius used by the local tangent-plane projection.
pub const EARTH_RADIUS_M: f64 = 6_371_000.0;

/// Standard gravity.
pub const GRAVITY: f64 = 9.81;

/// Barometric scale height of the standard atmosphere, in metres.
pub const ATMOSPHERE_SCALE_HEIGHT_M: f64 = 8_434.0;

/// Standard sea-level pressure in pascals.
pub const SEA_LEVEL_PRESSURE_PA: f64 = 101_325.0;

/// z-component of the 2D cross product `a x b`.
#[inline]
pub fn cross2(a: DVec2, b: DVec2) -> f64 {
    a.x * b.y - a.y * b.x
}

/// Direction angle of a vector, in `[-pi, pi)`.
#[inline]
pub fn angle_of(v: DVec2) -> f64 {
    v.y.atan2(v.x)
}

/// Unit vector of a direction angle.
#[inline]
pub fn dir_of(angle: f64) -> DVec2 {
    DVec2::new(angle.cos(), angle.sin())
}

/// Signed smallest difference `a - b`, wrapped into `[-pi, pi)`.
#[inline]
pub fn angle_difference(a: f64, b: f64) -> f64 {
    let mut d = a - b;
    while d >= std::f64::consts::PI {
        d -= std::f64::consts::TAU;
    }
    while d < -std::f64::consts::PI {
        d += std::f64::consts::TAU;
    }
    d
}

/// Clamps `value` into `[low, high]`.
#[inline]
pub fn clamp(value: f64, low: f64, high: f64) -> f64 {
    value.max(low).min(high)
}

/// Rotates a 2D vector by 90 degrees counter-clockwise (the left normal).
#[inline]
pub fn left_normal(v: DVec2) -> DVec2 {
    DVec2::new(-v.y, v.x)
}

/// Distance from `point` to the segment `a -> b`.
pub fn point_segment_distance(point: DVec2, a: DVec2, b: DVec2) -> f64 {
    let ab = b - a;
    let length_sq = ab.length_squared();
    if length_sq <= f64::EPSILON {
        return (point - a).length();
    }
    let t = ((point - a).dot(ab) / length_sq).clamp(0.0, 1.0);
    (point - (a + ab * t)).length()
}
