//! Cost field synthesis.
//!
//! A cell's cost is the linear combination of its normalised resistance
//! features, floored by the product of any soft rule multipliers, and infinite
//! where a hard constraint applies:
//!
//! ```text
//! c = +inf                                        if hard-forbidden
//!   = max(w . f + c0, c_base * prod(multipliers)) otherwise
//! ```
//!
//! Cost is expressed **per metre of travel**, so an edge cost is the line
//! integral of `c` and carries the unit "equivalent metres" that the Logit
//! choice model and the path-size factors are calibrated against.
//!
//! Direction constraints are the one feature that cannot be baked into a cell:
//! `f_dir = 1 + alpha * (1 - cos(theta))` depends on the travel heading, so the
//! per-cell part is precomputed and the heading term is added per query.

use glam::DVec2;

use crate::error::{CoreError, Result};
use crate::math::sampling::bilinear_f32;
use crate::terrain::Grid2D;

use super::feature::FeatureField;
use super::hard::HardMask;
use super::weights::CostWeights;

/// Sentinel standing in for `+inf` on hard-forbidden cells.
///
/// Chosen far below `f32::MAX` so that summing a few of them cannot overflow to
/// infinity, and far above any real accumulated cost.
pub const INFINITE_COST: f32 = 1.0e30;

/// Accumulated cost above which search nodes are pruned.
pub const PRUNE_THRESHOLD: f64 = 1.0e29;

/// Lower bound on the minimum unit cost, so the heuristic cannot collapse to
/// zero when every feature minimum is zero.
pub const MIN_COST_EPSILON: f64 = 1.0e-3;

/// Parameters of the cost model.
#[derive(Debug, Clone, PartialEq)]
pub struct CostModelParams {
    /// Baseline cost added to the weighted feature sum.
    pub c0: f64,
    /// Base cost of the multiplicative soft-rule term.
    pub c_base: f64,
    /// Multipliers of the soft rules; their product forms the multiplicative
    /// branch. An empty list disables that branch.
    pub soft_multipliers: Vec<f64>,
    /// Uniform cost scale applied to the whole field, mirroring
    /// [`CostWeights::scale`].
    pub cost_scale: f64,
}

impl Default for CostModelParams {
    fn default() -> Self {
        Self {
            c0: 1.0,
            c_base: 1.0,
            soft_multipliers: Vec::new(),
            cost_scale: 1.0,
        }
    }
}

/// Weighted feature sum per cell, the parallelisable part of the synthesis.
///
/// The direction dimension is skipped: its contribution depends on the travel
/// heading and is added per query.
#[allow(clippy::needless_range_loop)]
pub fn weight_features(
    features: &FeatureField,
    weights: &CostWeights,
    params: &CostModelParams,
) -> Vec<f32> {
    let grid = features.grid();
    let direction = features.direction_index();
    let mut partial = vec![0.0f32; grid.len()];
    for index in 0..grid.len() {
        let mut value = params.c0;
        for dim in 0..features.dim() {
            if Some(dim) == direction {
                continue;
            }
            value += weights.get(dim) * features.channels()[dim].values[index] as f64;
        }
        partial[index] = value as f32;
    }
    partial
}

/// Per-cell cost field with a directional term evaluated at query time.
#[derive(Debug, Clone)]
pub struct CostField {
    grid: Grid2D,
    partial: Vec<f32>,
    forbidden: Vec<bool>,
    min_unit_cost: f64,
    max_unit_cost: f64,
    mean_unit_cost: f64,
    params: CostModelParams,
    soft_product: f64,
    direction_weight: f64,
    direction_channel: Option<usize>,
    direction_values: Vec<f32>,
}

impl CostField {
    /// Synthesises the field from features, constraints and weights.
    pub fn synthesize(
        features: &FeatureField,
        hard: &HardMask,
        weights: &CostWeights,
        params: &CostModelParams,
    ) -> Result<Self> {
        if features.dim() != weights.dim() {
            return Err(CoreError::DimensionMismatch {
                weights: weights.dim(),
                features: features.dim(),
            });
        }
        let partial = weight_features(features, weights, params);
        Self::from_partial(features, hard, weights, params, partial)
    }

    /// Builds the field from precomputed weighted sums.
    ///
    /// The dot products are the parallelisable part of the synthesis and are
    /// exactly what a compute backend accelerates
    /// ([`crate::gpu::ComputeBackend::cost_field_batch`]); everything else — the
    /// hard-constraint mask, the soft-rule floor and the bounds the heuristic
    /// needs — is identical on both paths, so a caller can hand in either.
    #[allow(clippy::needless_range_loop)]
    pub fn from_partial(
        features: &FeatureField,
        hard: &HardMask,
        weights: &CostWeights,
        params: &CostModelParams,
        partial: Vec<f32>,
    ) -> Result<Self> {
        if features.dim() != weights.dim() {
            return Err(CoreError::DimensionMismatch {
                weights: weights.dim(),
                features: features.dim(),
            });
        }
        let grid = *features.grid();
        if grid.len() != hard.grid().len() {
            return Err(CoreError::config(
                "feature field and hard mask cover different grids",
            ));
        }
        if partial.len() != grid.len() {
            return Err(CoreError::config(format!(
                "weighted sum covers {} cell(s), the grid has {}",
                partial.len(),
                grid.len()
            )));
        }

        let direction_channel = features.direction_index();
        let direction_weight = direction_channel.map(|dim| weights.get(dim)).unwrap_or(0.0);
        let direction_values = direction_channel
            .map(|dim| features.channels()[dim].values.clone())
            .unwrap_or_default();

        let soft_product = if params.soft_multipliers.is_empty() {
            0.0
        } else {
            params.c_base * params.soft_multipliers.iter().product::<f64>()
        };

        let mut partial = partial;
        let mut min_unit = f64::INFINITY;
        let mut max_unit = f64::NEG_INFINITY;
        let mut sum = 0.0f64;
        let mut counted = 0usize;
        for index in 0..grid.len() {
            if hard.mask()[index] {
                partial[index] = INFINITE_COST;
                continue;
            }
            let mut value = partial[index] as f64 * params.cost_scale;
            if soft_product > 0.0 {
                value = value.max(soft_product);
            }
            value = value.max(MIN_COST_EPSILON);
            partial[index] = value as f32;
            min_unit = min_unit.min(value);
            max_unit = max_unit.max(value);
            sum += value;
            counted += 1;
        }
        if counted == 0 {
            return Err(CoreError::config("cost field has no passable cell"));
        }

        // Lower bound for the heuristic: the weighted sum of per-dimension
        // minima plus the baseline, floored by a small positive number. It cannot
        // exceed the true minimum because weights and features are non-negative.
        let mut lower = params.c0;
        for (dim, minimum) in features.minima().iter().enumerate() {
            if Some(dim) == direction_channel {
                continue;
            }
            lower += weights.get(dim) * minimum;
        }
        lower *= params.cost_scale;
        let min_unit_cost = lower.max(MIN_COST_EPSILON).min(min_unit);

        Ok(Self {
            grid,
            partial,
            forbidden: hard.mask().to_vec(),
            min_unit_cost,
            max_unit_cost: max_unit,
            mean_unit_cost: sum / counted as f64,
            params: params.clone(),
            soft_product,
            direction_weight,
            direction_channel,
            direction_values,
        })
    }

    /// Grid geometry.
    #[inline]
    pub fn grid(&self) -> &Grid2D {
        &self.grid
    }

    /// Model parameters.
    #[inline]
    pub fn params(&self) -> &CostModelParams {
        &self.params
    }

    /// Smallest possible unit cost, the heuristic's positive lower bound.
    #[inline]
    pub fn min_unit_cost(&self) -> f64 {
        self.min_unit_cost
    }

    /// Largest unit cost over passable cells.
    #[inline]
    pub fn max_unit_cost(&self) -> f64 {
        self.max_unit_cost
    }

    /// Mean unit cost over passable cells.
    #[inline]
    pub fn mean_unit_cost(&self) -> f64 {
        self.mean_unit_cost
    }

    /// Cost per metre at a position, optionally for a travel heading.
    ///
    /// The heading only matters where the map declares a direction constraint;
    /// elsewhere the argument is ignored.
    pub fn cost_at(&self, position: DVec2, heading: Option<DVec2>) -> f64 {
        let index = self.index_of(position);
        let base = self.partial[index] as f64;
        if base >= INFINITE_COST as f64 {
            return f64::INFINITY;
        }
        if self.direction_channel.is_none() || self.direction_weight <= 0.0 {
            return base;
        }
        let Some(heading) = heading else {
            return base;
        };
        let packed = self.direction_values[index];
        if packed <= 0.0 {
            return base;
        }
        let angle_index = (packed / 256.0).floor() as f64;
        let strength = (packed as f64 - angle_index * 256.0) / 255.0;
        if strength <= 0.0 {
            return base;
        }
        let preferred = angle_index / 256.0 * std::f64::consts::TAU;
        let cos_theta = (preferred - heading.y.atan2(heading.x)).cos();
        let feature = 1.0 + strength * (1.0 - cos_theta);
        base + self.direction_weight * feature * self.params.cost_scale
    }

    /// True when the cell at a position is hard-forbidden or outside the map.
    pub fn is_forbidden(&self, position: DVec2) -> bool {
        if !self.grid.contains(position) {
            return true;
        }
        let index = self.index_of(position);
        self.forbidden[index]
    }

    /// True when the whole cell is passable and within bounds.
    pub fn is_passable(&self, position: DVec2) -> bool {
        !self.is_forbidden(position)
    }

    /// True when a straight segment stays on passable cells.
    ///
    /// Delegates to the same grid traversal [`HardMask`] provides, so a caller
    /// holding only the cost field gets the exact check rather than a sampled
    /// approximation.
    ///
    /// [`HardMask`]: crate::field::HardMask
    pub fn segment_is_clear(&self, from: DVec2, to: DVec2) -> bool {
        crate::field::hard::segment_is_clear(&self.grid, &self.forbidden, from, to)
    }

    #[inline]
    fn index_of(&self, position: DVec2) -> usize {
        let (x, y) = self.grid.cell_of(position);
        self.grid.index(x, y)
    }

    /// Bilinearly interpolated cost, used for smooth reporting rather than for
    /// path integration.
    pub fn sampled_cost_at(&self, position: DVec2) -> f64 {
        let continuous = self.grid.continuous(position);
        bilinear_f32(
            &self.partial,
            self.grid.width,
            self.grid.height,
            continuous.x,
            continuous.y,
        )
    }

    /// Raw per-cell costs, row-major.
    #[inline]
    pub fn raw(&self) -> &[f32] {
        &self.partial
    }

    /// Multiplicative soft-rule branch, zero when disabled.
    #[inline]
    pub fn soft_product(&self) -> f64 {
        self.soft_product
    }
}
