//! Path geometry: arc-length parameterisation and curvature.

pub mod polyline;
pub mod resample;

pub use polyline::Path;
pub use resample::{
    cumulative_lengths, curvatures, deduplicate, max_turn_angle, resample, turn_angle,
};
