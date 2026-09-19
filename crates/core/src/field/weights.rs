//! Cost weights: static lookup and optional attention gating.
//!
//! The zero-training default is the static lookup — a softmax over per-mode
//! prior logits, scaled by a uniform cost factor so different modes share one
//! cost scale. The contextual attention gate is the optional enhancement: it
//! modulates each feature dimension by a channel-attention weight derived from
//! a context vector, with the safeguards the design requires (normalisation to
//! mean one, clipping and renormalisation after clipping).

use ourealis_map_format::MotionMode;
use ourealis_map_format::tlv::value::{WeightPrior, WeightPriorEntry};

use crate::error::{CoreError, Result};

/// Default temperature applied to prior logits.
pub const DEFAULT_TAU: f64 = 1.0;

/// Default lower clip bound of the normalised modulation factor.
pub const DEFAULT_CLIP_MIN: f64 = 0.2;

/// Default upper clip bound of the normalised modulation factor.
pub const DEFAULT_CLIP_MAX: f64 = 5.0;

/// Contextual gating matrices.
///
/// Both matrices are static: in the zero-training stage they are hand-built
/// from domain knowledge, and the design allows them to be calibrated later
/// from the same initial values.
#[derive(Debug, Clone, PartialEq)]
pub struct AttentionGating {
    /// Embedding dimension `d`, independent of the feature dimension `D`.
    pub embedding_dim: usize,
    /// Query projection, `d x context_dim` row-major.
    pub w_q: Vec<f64>,
    /// Key projection, `d x D` row-major.
    pub w_k: Vec<f64>,
    /// Lower clip bound `a_min` for the normalised modulation factor.
    pub clip_min: f64,
    /// Upper clip bound `a_max`.
    pub clip_max: f64,
}

impl AttentionGating {
    /// Creates a gate with the design's default clip range.
    pub fn new(embedding_dim: usize, w_q: Vec<f64>, w_k: Vec<f64>) -> Result<Self> {
        if embedding_dim == 0 {
            return Err(CoreError::config(
                "attention embedding dimension must be positive",
            ));
        }
        if !w_q.len().is_multiple_of(embedding_dim) || !w_k.len().is_multiple_of(embedding_dim) {
            return Err(CoreError::config(
                "attention projection matrices must have a whole number of rows",
            ));
        }
        Ok(Self {
            embedding_dim,
            w_q,
            w_k,
            clip_min: DEFAULT_CLIP_MIN,
            clip_max: DEFAULT_CLIP_MAX,
        })
    }

    /// Replaces the clip bounds.
    pub fn with_clip(mut self, min: f64, max: f64) -> Result<Self> {
        if min <= 0.0 || max < min {
            return Err(CoreError::config(
                "attention clip bounds must satisfy 0 < min <= max",
            ));
        }
        self.clip_min = min;
        self.clip_max = max;
        Ok(self)
    }

    /// Context width the query projection expects.
    pub fn context_dim(&self) -> usize {
        self.w_q.len() / self.embedding_dim
    }

    /// Feature dimension the key projection expects.
    pub fn feature_dim(&self) -> usize {
        self.w_k.len() / self.embedding_dim
    }

    /// Modulates a prior weight vector by the attention gate.
    ///
    /// Implements `w_i = w_prior,i * clip(D * alpha_i)` renormalised to mean one,
    /// then scaled: the mean-one property keeps the average cost level intact
    /// while still letting individual dimensions dominate in context.
    // The matrix loops are written with explicit indices: they walk a
    // projection matrix and a feature vector together, which iterator adapters
    // would obscure.
    #[allow(clippy::needless_range_loop)]
    pub fn modulate(&self, prior: &[f64], context: &[f64], scale: f64) -> Result<Vec<f64>> {
        let d = self.embedding_dim;
        if context.len() != self.context_dim() {
            return Err(CoreError::DimensionMismatch {
                weights: self.context_dim(),
                features: context.len(),
            });
        }
        if prior.len() != self.feature_dim() {
            return Err(CoreError::DimensionMismatch {
                weights: self.feature_dim(),
                features: prior.len(),
            });
        }

        // q = W_q z_ctx
        let mut q = vec![0.0f64; d];
        for row in 0..d {
            let mut sum = 0.0;
            for (column, value) in context.iter().enumerate() {
                sum += self.w_q[row * context.len() + column] * value;
            }
            q[row] = sum;
        }

        // Scores q . k_i / sqrt(d) for each feature dimension.
        let inv_sqrt_d = 1.0 / (d as f64).sqrt();
        let dim = prior.len();
        let mut scores = vec![0.0f64; dim];
        for i in 0..dim {
            let mut dot = 0.0;
            for row in 0..d {
                dot += q[row] * self.w_k[row * dim + i];
            }
            scores[i] = dot * inv_sqrt_d;
        }
        let alphas = softmax(&scores);

        // Normalised modulation factor, clipped and renormalised to mean one.
        let mut factors: Vec<f64> = alphas
            .iter()
            .map(|alpha| (dim as f64 * alpha).clamp(self.clip_min, self.clip_max))
            .collect();
        let sum: f64 = factors.iter().sum();
        if sum <= f64::EPSILON {
            return Err(CoreError::config("attention factors collapsed to zero"));
        }
        let normaliser = dim as f64 / sum;
        for factor in factors.iter_mut() {
            *factor *= normaliser;
        }

        Ok(prior
            .iter()
            .zip(factors.iter())
            .map(|(weight, factor)| weight * factor * scale)
            .collect())
    }
}

/// Resolved weight vector applied to the resistance features.
#[derive(Debug, Clone, PartialEq)]
pub struct CostWeights {
    /// One weight per feature dimension.
    pub values: Vec<f64>,
    /// Uniform cost scale factor, mirroring [`CostModelParams::cost_scale`].
    ///
    /// Reported for diagnostics and for callers that synthesize a field
    /// themselves; [`crate::field::CostField`] takes the factor from its own
    /// parameters so that the GPU and CPU paths cannot disagree about it.
    ///
    /// [`CostModelParams::cost_scale`]: crate::field::CostModelParams::cost_scale
    pub scale: f64,
}

impl CostWeights {
    /// Builds weights from a prior entry using the static lookup.
    ///
    /// `w = scale * softmax(p / tau)`, with optional overrides of the entry's
    /// temperature and scale.
    pub fn from_prior(entry: &WeightPriorEntry, tau: Option<f64>, scale: Option<f64>) -> Self {
        let tau = tau.unwrap_or(entry.tau as f64).max(1e-6);
        let scale = scale.unwrap_or(entry.scale as f64);
        let logits: Vec<f64> = entry.weights.iter().map(|w| *w as f64 / tau).collect();
        let values = softmax(&logits);
        Self { values, scale }
    }

    /// Builds a uniform weight vector, used when the map carries no priors.
    pub fn uniform(dim: usize) -> Self {
        let value = if dim == 0 { 0.0 } else { 1.0 / dim as f64 };
        Self {
            values: vec![value; dim],
            scale: 1.0,
        }
    }

    /// Builds weights for a motion mode, falling back to a uniform vector.
    pub fn for_mode(prior: Option<&WeightPrior>, mode: MotionMode, dim: usize) -> Self {
        match prior.and_then(|prior| prior.get(mode)) {
            Some(entry) if entry.weights.len() == dim => Self::from_prior(entry, None, None),
            _ => Self::uniform(dim),
        }
    }

    /// Applies the attention gate on top of a prior entry.
    ///
    /// The returned weights are the *unscaled* modulated priors: `scale` is
    /// reported in [`CostWeights::scale`] but not folded in, because the cost
    /// field applies it to the whole weighted sum. Folding it in here as well
    /// would square it relative to the static path.
    pub fn with_attention(
        entry: &WeightPriorEntry,
        gating: &AttentionGating,
        context: &[f64],
        tau: Option<f64>,
        scale: Option<f64>,
    ) -> Result<Self> {
        let base = Self::from_prior(entry, tau, Some(1.0));
        let scale = scale.unwrap_or(entry.scale as f64);
        let values = gating.modulate(&base.values, context, 1.0)?;
        Ok(Self { values, scale })
    }

    /// Number of dimensions.
    #[inline]
    pub fn dim(&self) -> usize {
        self.values.len()
    }

    /// Weight of a dimension.
    #[inline]
    pub fn get(&self, dim: usize) -> f64 {
        self.values.get(dim).copied().unwrap_or(0.0)
    }
}

/// Numerically stable softmax.
pub fn softmax(logits: &[f64]) -> Vec<f64> {
    if logits.is_empty() {
        return Vec::new();
    }
    let max = logits.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let mut out: Vec<f64> = logits.iter().map(|value| (value - max).exp()).collect();
    let sum: f64 = out.iter().sum();
    if sum <= f64::EPSILON {
        let uniform = 1.0 / logits.len() as f64;
        return vec![uniform; logits.len()];
    }
    for value in out.iter_mut() {
        *value /= sum;
    }
    out
}
