//! GPU compute backends.
//!
//! The search stays on the CPU — its node expansion is branch-heavy and gains
//! nothing from a GPU — but three stages are pure data parallelism and belong
//! there:
//!
//! * **cost field synthesis**: `C[cell][mode] = F[cell][d] . W[d][mode] + c0`, a
//!   matrix product over a `[H*W][D]` feature tensor and a `[D][M]` weight
//!   matrix. This is the most parallel computation in the system;
//! * **batched noise**: one thread per individual, looping over samples, which
//!   keeps the Ornstein-Uhlenbeck recursion sequential in time where it must be;
//! * **batched projection checks**: reading the constraint bitmap and the
//!   distance field for many points at once.
//!
//! ## Determinism across backends
//!
//! The batch paths use a **counter-based** random source: sample `k` of channel
//! `c` for individual `i` is a hash of `(seed, i, c, k)`, turned into a normal
//! deviate by the same transform on both sides. Sequential generators cannot give
//! that guarantee on a GPU, and the design requires reproducible batches, so the
//! two paths are deliberately separate and the batch path is verifiable by a
//! parity test rather than by construction.

pub mod cpu;

#[cfg(feature = "gpu")]
pub mod wgpu_backend;

use crate::error::{CoreError, Result};

/// Dense resistance feature tensor, `[cells][dims]`.
#[derive(Debug, Clone, PartialEq)]
pub struct FeatureTensor {
    /// Number of cells.
    pub cells: usize,
    /// Feature dimension `D`.
    pub dims: usize,
    /// Values in row-major `[cells][dims]` order.
    pub values: Vec<f32>,
}

impl FeatureTensor {
    /// Creates a tensor, checking the length.
    pub fn new(cells: usize, dims: usize, values: Vec<f32>) -> Result<Self> {
        if values.len() != cells * dims {
            return Err(CoreError::config(format!(
                "feature tensor has {} value(s), expected {}",
                values.len(),
                cells * dims
            )));
        }
        Ok(Self {
            cells,
            dims,
            values,
        })
    }
}

/// Weight matrix `[dims][modes]`.
#[derive(Debug, Clone, PartialEq)]
pub struct WeightMatrix {
    /// Number of modes.
    pub modes: usize,
    /// Feature dimension `D`.
    pub dims: usize,
    /// Values in row-major `[dims][modes]` order.
    pub values: Vec<f32>,
}

impl WeightMatrix {
    /// Builds the matrix from one weight vector per mode.
    pub fn from_vectors(vectors: &[Vec<f32>]) -> Result<Self> {
        let modes = vectors.len();
        let dims = vectors.first().map(|v| v.len()).unwrap_or(0);
        if vectors.iter().any(|v| v.len() != dims) {
            return Err(CoreError::config(
                "weight vectors must all have the same dimension",
            ));
        }
        let mut values = vec![0.0f32; dims * modes];
        for (mode, vector) in vectors.iter().enumerate() {
            for (dim, weight) in vector.iter().enumerate() {
                values[dim * modes + mode] = *weight;
            }
        }
        Ok(Self {
            modes,
            dims,
            values,
        })
    }
}

/// Cost per cell and mode, `[cells][modes]`.
#[derive(Debug, Clone, PartialEq)]
pub struct CostBatch {
    /// Number of cells covered.
    pub cells: usize,
    /// Number of modes.
    pub modes: usize,
    /// Values in row-major `[cells][modes]` order.
    pub values: Vec<f32>,
}

impl CostBatch {
    /// Cost of one cell under one mode.
    pub fn get(&self, cell: usize, mode: usize) -> f32 {
        self.values[cell * self.modes + mode]
    }
}

/// Ornstein-Uhlenbeck parameters of a noise batch.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OuSpec {
    /// Steady-state standard deviation.
    pub sigma: f32,
    /// Time constant, seconds.
    pub tau: f32,
    /// Long-run mean.
    pub mean: f32,
}

/// Specification of a batched noise generation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NoiseBatchSpec {
    /// Number of independent series.
    pub individuals: usize,
    /// Samples per series.
    pub samples: usize,
    /// Time step, seconds.
    pub dt: f32,
    /// Drift process parameters.
    pub drift: OuSpec,
    /// White noise standard deviation added on top of the drift.
    pub white_sigma: f32,
    /// Seed of the batch.
    pub seed: u64,
}

impl NoiseBatchSpec {
    /// Validates the specification.
    pub fn validate(&self) -> Result<()> {
        if self.individuals == 0 || self.samples == 0 {
            return Err(CoreError::config(
                "noise batch must cover at least one sample",
            ));
        }
        if self.dt < 0.0 {
            return Err(CoreError::config(
                "noise batch time step cannot be negative",
            ));
        }
        Ok(())
    }
}

/// Generated noise series, `[individuals][samples]`.
#[derive(Debug, Clone, PartialEq)]
pub struct NoiseBatch {
    /// Number of series.
    pub individuals: usize,
    /// Samples per series.
    pub samples: usize,
    /// Drift component in row-major `[individuals][samples]` order.
    pub drift: Vec<f32>,
    /// White component in the same layout.
    pub white: Vec<f32>,
}

impl NoiseBatch {
    /// Drift value of one sample.
    pub fn drift_at(&self, individual: usize, sample: usize) -> f32 {
        self.drift[individual * self.samples + sample]
    }

    /// Total value of one sample, drift plus white.
    pub fn value_at(&self, individual: usize, sample: usize) -> f32 {
        let index = individual * self.samples + sample;
        self.drift[index] + self.white[index]
    }
}

/// A batch of projection queries: points whose feasibility has to be checked.
///
/// The elastic band, the lateral offset and the attachment of a route all ask the
/// same question of many points at once — is this point on passable ground, how far
/// is the nearest obstacle, and which way does that distance increase — and none of
/// them depends on the others. That is what makes it a batch.
#[derive(Debug, Clone, PartialEq)]
pub struct ProjectionBatch {
    /// Number of points.
    pub points: usize,
    /// Point coordinates, `[x, y]` per point.
    pub xy: Vec<f32>,
    /// Grid origin of the fields, `[x0, y0, 0, 0]`.
    pub grid_origin: [f32; 4],
    /// Grid resolution in metres.
    pub resolution: f32,
    /// Grid extent in cells.
    pub grid_dims: [u32; 4],
    /// Forbidden-cell bitmap, one bit per cell, row-major.
    pub forbidden: Vec<u32>,
    /// Distance field in metres, row-major, one value per cell.
    pub distance: Vec<f32>,
}

impl ProjectionBatch {
    /// Number of 32-bit words the bitmap needs for a grid.
    pub fn bitmap_words(cells: usize) -> usize {
        cells.div_ceil(32)
    }

    /// Validates the batch against its own declared sizes.
    pub fn validate(&self) -> Result<()> {
        if self.xy.len() != self.points * 2 {
            return Err(CoreError::config(format!(
                "projection batch has {} coordinate(s) for {} point(s)",
                self.xy.len(),
                self.points
            )));
        }
        let cells = (self.grid_dims[0] as usize) * (self.grid_dims[1] as usize);
        if self.distance.len() != cells {
            return Err(CoreError::config(format!(
                "projection batch has {} distance value(s) for {cells} cell(s)",
                self.distance.len()
            )));
        }
        if self.forbidden.len() < Self::bitmap_words(cells) {
            return Err(CoreError::config(
                "projection batch bitmap is shorter than the grid",
            ));
        }
        Ok(())
    }
}

/// Answer for one projection query.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ProjectionAnswer {
    /// True when the point's cell is forbidden.
    pub forbidden: bool,
    /// Distance to the nearest obstacle in metres, or a large value outside the
    /// loaded field.
    pub distance_m: f32,
    /// Wall-clock-free marker that the point lay outside the grid.
    pub outside: bool,
}

/// Answers of a projection batch, one per query point.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ProjectionBatchOut {
    /// One answer per point, in query order.
    pub answers: Vec<ProjectionAnswer>,
}

impl ProjectionBatchOut {
    /// Answer of one point.
    pub fn get(&self, point: usize) -> ProjectionAnswer {
        self.answers.get(point).copied().unwrap_or_default()
    }
}

/// Common interface of the compute backends.
pub trait ComputeBackend: Send + Sync {
    /// Backend name, recorded in the output manifest.
    fn name(&self) -> String;

    /// Weighted feature sum for every cell and mode.
    fn cost_field_batch(
        &self,
        features: &FeatureTensor,
        weights: &WeightMatrix,
        c0: f32,
    ) -> Result<CostBatch>;

    /// Batched Ornstein-Uhlenbeck plus white noise.
    fn noise_batch(&self, spec: &NoiseBatchSpec) -> Result<NoiseBatch>;

    /// Batched feasibility and clearance queries.
    fn projection_check_batch(&self, batch: &ProjectionBatch) -> Result<ProjectionBatchOut>;
}

/// Selects a backend, falling back to the CPU when the GPU is unavailable.
pub fn select_backend(kind: crate::sim::config::Backend) -> Result<Box<dyn ComputeBackend>> {
    use crate::sim::config::Backend;
    match kind {
        Backend::Cpu => Ok(Box::new(cpu::CpuBackend::new())),
        Backend::Auto | Backend::Gpu => {
            #[cfg(feature = "gpu")]
            {
                match wgpu_backend::WgpuBackend::new() {
                    Ok(backend) => return Ok(Box::new(backend)),
                    Err(error) => {
                        if kind == Backend::Gpu {
                            return Err(error);
                        }
                        tracing::warn!(
                            "GPU backend unavailable ({error}); falling back to the CPU backend"
                        );
                    }
                }
            }
            #[cfg(not(feature = "gpu"))]
            if kind == Backend::Gpu {
                return Err(CoreError::NoAdapter {
                    detail: "the crate was built without the `gpu` feature".into(),
                });
            }
            Ok(Box::new(cpu::CpuBackend::new()))
        }
    }
}

/// 32-bit finaliser used by the counter-based random source.
///
/// Everything stays in 32-bit integers: WGSL has no 64-bit integer type without
/// an optional extension, and requiring one would cut the backend off from
/// hardware that is otherwise perfectly capable of this workload.
#[inline]
pub fn hash32(mut z: u32) -> u32 {
    z = (z ^ (z >> 16)).wrapping_mul(0x7feb_352d);
    z = (z ^ (z >> 15)).wrapping_mul(0x846c_a68b);
    z ^ (z >> 16)
}

/// Standard normal deviate at a counter position, computed without any
/// sequential state.
///
/// The batch path uses this rather than the sequential generator: a GPU cannot
/// advance a chained generator reproducibly, and the design requires batches to
/// be reproducible. The *hash* is exact on both backends; the Box-Muller
/// transform agrees to within the device's transcendental precision, which the
/// parity test allows for.
#[inline]
pub fn counter_gaussian(seed: u64, individual: u32, channel: u32, sample: u32) -> f32 {
    let seed_lo = seed as u32;
    let seed_hi = (seed >> 32) as u32;
    let base = seed_lo
        ^ hash32(individual ^ channel.wrapping_mul(2_654_435_761))
        ^ hash32(sample ^ seed_hi);
    let first = hash32(base);
    let second = hash32(first ^ 0x9e37_79b9);
    let u1 = ((first >> 8) as f32 / 16_777_216.0).max(1e-30);
    let u2 = (second >> 8) as f32 / 16_777_216.0;
    let radius = (-2.0 * u1.ln()).sqrt();
    radius * (std::f32::consts::TAU * u2).cos()
}

/// Packs a set of points into a projection batch over a grid and a constraint mask.
///
/// The one place the field layout is written, so every caller — the environment and
/// the motion stage alike — produces the same batch for the same question.
///
/// `distance_at` supplies the clearance field; it is only read by callers that ask
/// for it, but the batch carries it because a batch is validated as a whole.
pub fn projection_batch_from(
    grid: &crate::terrain::Grid2D,
    forbidden_mask: &[bool],
    distance_at: impl Fn(glam::DVec2) -> f64,
    points: &[glam::DVec2],
) -> Result<ProjectionBatch> {
    let cells = grid.len();
    if forbidden_mask.len() < cells {
        return Err(CoreError::config(
            "the constraint mask is shorter than the grid",
        ));
    }
    let mut forbidden = vec![0u32; ProjectionBatch::bitmap_words(cells)];
    for (cell, blocked) in forbidden_mask.iter().enumerate().take(cells) {
        if *blocked {
            forbidden[cell / 32] |= 1 << (cell % 32);
        }
    }
    let mut distance = vec![0.0f32; cells];
    for (cell, value) in distance.iter_mut().enumerate() {
        let (x, y) = grid.coordinates(cell);
        *value = distance_at(grid.cell_center(x, y)) as f32;
    }
    let mut xy = Vec::with_capacity(points.len() * 2);
    for point in points {
        xy.push(point.x as f32);
        xy.push(point.y as f32);
    }
    let bounds = grid.bounds();
    let batch = ProjectionBatch {
        points: points.len(),
        xy,
        grid_origin: [bounds.min_x as f32, bounds.min_y as f32, 0.0, 0.0],
        resolution: grid.resolution as f32,
        grid_dims: [grid.width as u32, grid.height as u32, 0, 0],
        forbidden,
        distance,
    };
    batch.validate()?;
    Ok(batch)
}

/// Fills a projection batch on the CPU, used directly and as the parity reference.
///
/// The lookup is deliberately the same arithmetic the shader performs: convert to
/// cell coordinates with a floor, test the bit, read the distance. Anything more
/// clever would make the parity test compare two different questions.
pub fn projection_batch_cpu(batch: &ProjectionBatch) -> Result<ProjectionBatchOut> {
    batch.validate()?;
    let (width, height) = (batch.grid_dims[0], batch.grid_dims[1]);
    let mut answers = Vec::with_capacity(batch.points);
    for index in 0..batch.points {
        let x = batch.xy[index * 2];
        let y = batch.xy[index * 2 + 1];
        let cell_x = ((x - batch.grid_origin[0]) / batch.resolution).floor();
        let cell_y = ((y - batch.grid_origin[1]) / batch.resolution).floor();
        if !(cell_x >= 0.0 && cell_y >= 0.0) || cell_x as u32 >= width || cell_y as u32 >= height {
            answers.push(ProjectionAnswer {
                forbidden: true,
                distance_m: 0.0,
                outside: true,
            });
            continue;
        }
        let (cell_x, cell_y) = (cell_x as u32, cell_y as u32);
        let cell = (cell_y * width + cell_x) as usize;
        let word = batch.forbidden[cell / 32];
        let forbidden = (word >> (cell % 32)) & 1 == 1;
        answers.push(ProjectionAnswer {
            forbidden,
            distance_m: batch.distance[cell],
            outside: false,
        });
    }
    Ok(ProjectionBatchOut { answers })
}

/// Fills a noise batch on the CPU, used directly and as the parity reference.
pub fn noise_batch_cpu(spec: &NoiseBatchSpec) -> Result<NoiseBatch> {
    spec.validate()?;
    let theta = if spec.drift.tau > 1e-6 {
        1.0 / spec.drift.tau
    } else {
        1.0e6
    };
    // Exact discrete O-U update; see `crate::noise::OuProcess::step` for why the
    // Euler form is not used. Must stay bit-comparable with the WGSL kernel.
    let decay = (-theta * spec.dt).exp();
    let step_sigma = spec.drift.sigma * (1.0 - decay * decay).max(0.0).sqrt();

    let mut drift = vec![0.0f32; spec.individuals * spec.samples];
    let mut white = vec![0.0f32; spec.individuals * spec.samples];
    for individual in 0..spec.individuals {
        let mut value = spec.drift.mean;
        for sample in 0..spec.samples {
            let index = individual * spec.samples + sample;
            let noise = counter_gaussian(spec.seed, individual as u32, 1, sample as u32);
            value = spec.drift.mean + (value - spec.drift.mean) * decay + step_sigma * noise;
            drift[index] = value;
            white[index] =
                spec.white_sigma * counter_gaussian(spec.seed, individual as u32, 2, sample as u32);
        }
    }
    Ok(NoiseBatch {
        individuals: spec.individuals,
        samples: spec.samples,
        drift,
        white,
    })
}
