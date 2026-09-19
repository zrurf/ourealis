//! Environment description and cost synthesis.
//!
//! The map stores objective environment features; this module turns them into
//! the cost field the planner searches. The split is deliberate: one map serves
//! every motion mode because nothing preference-dependent is ever stored, only
//! synthesised here from the mode's weight prior.

pub mod cost;
pub mod feature;
pub mod hard;
pub mod sampler;
pub mod weights;

pub use cost::{CostField, CostModelParams, INFINITE_COST, MIN_COST_EPSILON, PRUNE_THRESHOLD};
pub use feature::{FeatureChannel, FeatureField};
pub use hard::HardMask;
pub use sampler::{CostSampler, SegmentCost};
pub use weights::{AttentionGating, CostWeights, softmax};
