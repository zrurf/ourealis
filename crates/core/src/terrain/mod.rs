//! Terrain: elevation, slope and the distance transform.
//!
//! Elevation and slope feed the physiological speed model (Minetti) and the
//! pitch component of attitude; the distance field drives obstacle avoidance in
//! path smoothing and the lateral-offset feasibility check. Derived fields are
//! taken from the map when it carries them and built on the spot otherwise.

pub mod dem;
pub mod edt;
pub mod grid;

pub use dem::Terrain;
pub use edt::{DistanceField, GRADIENT_DIRECTIONS};
pub use grid::Grid2D;
