//! Path smoothing and feasibility projection.

pub mod elastic;
pub mod project;
pub mod rounding;
pub mod simplify;

pub use elastic::{ElasticBandConfig, SmoothResult, smooth_path, smooth_path_anchored};
pub use project::{
    ProjectionOutcome, infeasible_segments, insert_midpoints, project_point, project_polyline,
};
pub use rounding::round_corners;
pub use simplify::simplify;
