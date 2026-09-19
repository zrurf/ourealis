//! CPU reference backend.
//!
//! This implementation is the semantic reference: it is always available, it is
//! what the test suite exercises, and the GPU path is validated against it. It
//! parallelises over cells with rayon, and the reduction order per cell is fixed
//! (a single accumulation over `D`), so results do not depend on the thread
//! count.

use rayon::prelude::*;

use crate::error::{CoreError, Result};

use super::{
    ComputeBackend, CostBatch, FeatureTensor, NoiseBatch, NoiseBatchSpec, ProjectionBatch,
    ProjectionBatchOut, WeightMatrix, noise_batch_cpu, projection_batch_cpu,
};

/// Reference backend running on the CPU.
#[derive(Debug, Clone, Copy, Default)]
pub struct CpuBackend;

impl CpuBackend {
    /// Creates the backend.
    pub fn new() -> Self {
        Self
    }
}

impl ComputeBackend for CpuBackend {
    fn name(&self) -> String {
        "cpu-rayon".to_string()
    }

    fn cost_field_batch(
        &self,
        features: &FeatureTensor,
        weights: &WeightMatrix,
        c0: f32,
    ) -> Result<CostBatch> {
        if features.dims != weights.dims {
            return Err(CoreError::DimensionMismatch {
                weights: weights.dims,
                features: features.dims,
            });
        }
        let modes = weights.modes;
        let dims = weights.dims;
        let mut values = vec![0.0f32; features.cells * modes];

        values
            .par_chunks_mut(modes)
            .enumerate()
            .for_each(|(cell, row)| {
                let base = cell * dims;
                for (mode, slot) in row.iter_mut().enumerate() {
                    let mut sum = c0;
                    for dim in 0..dims {
                        sum += features.values[base + dim] * weights.values[dim * modes + mode];
                    }
                    *slot = sum;
                }
            });

        Ok(CostBatch {
            cells: features.cells,
            modes,
            values,
        })
    }

    fn noise_batch(&self, spec: &NoiseBatchSpec) -> Result<NoiseBatch> {
        noise_batch_cpu(spec)
    }

    fn projection_check_batch(&self, batch: &ProjectionBatch) -> Result<ProjectionBatchOut> {
        projection_batch_cpu(batch)
    }
}
