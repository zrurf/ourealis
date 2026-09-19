//! Individual parameters and population sampling.

pub mod params;
pub mod sampling;

pub use params::{PaceStrategy, PersonParams, Preset, SensorNoiseParams};
pub use sampling::{PersonSampler, PopulationSpread};
