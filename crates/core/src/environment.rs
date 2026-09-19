//! Loaded map environment.
//!
//! Everything the simulator reads from an OMF file is loaded once into dense,
//! query-friendly structures here: terrain, constraints, the distance transform,
//! the feature field, the synthesised cost field, connectors, the roadmap and
//! the region annotations. Planning, motion and sensing all operate on this
//! type, so a map is opened exactly once per run.

use glam::DVec2;

use ourealis_map_format::tlv::value::{MapInfo, SlopeModel, WeightPrior};
use ourealis_map_format::{Aabb, Map};

use crate::error::{CoreError, Result};
use crate::field::{CostField, CostModelParams, CostWeights, FeatureField, HardMask};
use crate::graph::{CoarseGrid, CoarseOptions, ConnectorSet, PrmParams, PrmRoadmap};
use crate::math::LocalFrame;
use crate::terrain::{DistanceField, Grid2D, Terrain};

/// Map contents prepared for simulation.
#[derive(Debug, Clone)]
pub struct Environment {
    /// Identity block of the map, when present.
    pub map_info: Option<MapInfo>,
    /// Reference point of the local plane, when the map carries one.
    pub frame: Option<LocalFrame>,
    /// Extent of the map.
    pub bounds: Aabb,
    /// Grid geometry of the loaded fields.
    pub grid: Grid2D,
    /// Elevation and slope access.
    pub terrain: Terrain,
    /// Hard constraint mask.
    pub hard: HardMask,
    /// Distance transform, loaded or derived.
    pub distance: DistanceField,
    /// Resistance features.
    pub features: FeatureField,
    /// Synthesised cost field.
    pub cost: CostField,
    /// Z-axis connectors.
    pub connectors: ConnectorSet,
    /// Roadmap of open areas, when available or generated.
    pub prm: Option<PrmRoadmap>,
    /// Coarse blocks from the map's quadtree skeleton.
    pub coarse: CoarseGrid,
    /// Pre-generated candidate paths, when the map carries them.
    pub kpath: Option<ourealis_map_format::graph::kpath::KPathLibrary>,
    /// Region annotations for sensor events.
    pub regions: Option<ourealis_map_format::region::RegionSet>,
    /// Local geomagnetic parameters.
    pub magnetic_field: Option<ourealis_map_format::tlv::value::MagneticField>,
    /// Slope model parameters from the map.
    pub slope_model: SlopeModel,
    /// Weight priors from the map.
    pub weight_prior: Option<WeightPrior>,
}

impl Environment {
    /// Loads an environment from a map.
    ///
    /// Derived layers are used when the map carries them (and their fingerprints
    /// verify) and computed on the spot otherwise, which is what makes maps built
    /// from source layers only usable without a preprocessing pass.
    pub fn load(
        map: &Map,
        weights: &CostWeights,
        cost_params: &CostModelParams,
        prm: PrmOptions,
    ) -> Result<Self> {
        Self::load_with_backend(map, weights, cost_params, prm, None)
    }

    /// Loads an environment with an explicit coarse-layer configuration.
    pub fn load_with_coarse(
        map: &Map,
        weights: &CostWeights,
        cost_params: &CostModelParams,
        prm: PrmOptions,
        coarse: CoarseOptions,
        backend: Option<&dyn crate::gpu::ComputeBackend>,
    ) -> Result<Self> {
        let mut environment = Self::load_with_backend(map, weights, cost_params, prm, backend)?;
        environment.coarse = CoarseGrid::from_map(map, &coarse)?;
        Ok(environment)
    }

    /// Loads an environment, optionally synthesising the cost field on a
    /// compute backend.
    ///
    /// The weighted feature sum is the one part of the synthesis that is pure
    /// data parallelism, so it is the part a GPU can take over; the constraint
    /// mask, the soft-rule floor and the heuristic bounds are applied identically
    /// afterwards, which is what keeps the two paths comparable.
    pub fn load_with_backend(
        map: &Map,
        weights: &CostWeights,
        cost_params: &CostModelParams,
        prm: PrmOptions,
        backend: Option<&dyn crate::gpu::ComputeBackend>,
    ) -> Result<Self> {
        let features = FeatureField::from_map(map)?;
        let hard = HardMask::from_map(map)?;
        let terrain = Terrain::from_map(map)?;
        let grid = *terrain.grid();
        if grid.len() != hard.grid().len() {
            return Err(CoreError::config(
                "terrain and constraint grids disagree on size",
            ));
        }
        let distance = match DistanceField::from_map(map) {
            Some(field) if field.grid().len() == grid.len() => field,
            _ => {
                tracing::debug!(
                    "map carries no usable distance transform; deriving it from the hard mask"
                );
                DistanceField::from_mask(grid, hard.mask())
            }
        };
        let cost = match backend {
            Some(backend) => {
                let partial = weighted_sum_on_backend(&features, weights, cost_params, backend)?;
                CostField::from_partial(&features, &hard, weights, cost_params, partial)?
            }
            None => CostField::synthesize(&features, &hard, weights, cost_params)?,
        };

        let connectors = ConnectorSet::new(&map.connectors()?.unwrap_or_default());
        let kpath = map.kpath_library()?;
        let regions = map.regions()?;
        let weight_prior = map.weight_prior()?;
        let slope_model = map.slope_model()?;
        let magnetic_field = map.magnetic_field()?;
        let map_info = map.map_info()?;
        let frame = map
            .has_geo_reference()
            .then(|| LocalFrame::from_header(map.header()));

        let coarse = CoarseGrid::from_map(map, &CoarseOptions::default())?;
        let prm = match prm {
            PrmOptions::None => None,
            PrmOptions::Stored(batch) => PrmRoadmap::from_map(map, batch)?,
            PrmOptions::Generate { seed, params } => Some(PrmRoadmap::generate(
                &grid,
                &cost,
                &hard,
                &connectors,
                Some(&terrain),
                seed,
                &params,
            )),
        };

        Ok(Self {
            map_info,
            frame,
            bounds: map.header().bounds,
            grid,
            terrain,
            hard,
            distance,
            features,
            cost,
            connectors,
            kpath,
            prm,
            coarse,
            regions,
            magnetic_field,
            slope_model,
            weight_prior,
        })
    }

    /// Builds an environment from explicit parts, used by tests.
    #[allow(clippy::too_many_arguments)]
    pub fn from_parts(
        bounds: Aabb,
        terrain: Terrain,
        hard: HardMask,
        distance: DistanceField,
        features: FeatureField,
        cost: CostField,
        connectors: ConnectorSet,
    ) -> Self {
        let grid = *terrain.grid();
        Self {
            map_info: None,
            frame: None,
            bounds,
            grid,
            terrain,
            hard,
            distance,
            features,
            cost,
            connectors,
            kpath: None,
            prm: None,
            coarse: CoarseGrid::default(),
            regions: None,
            magnetic_field: None,
            slope_model: SlopeModel::default(),
            weight_prior: None,
        }
    }

    /// Clamps a position into the map and onto passable ground.
    pub fn snap_to_passable(&self, position: DVec2, max_radius_m: f64) -> Result<DVec2> {
        if self.hard.is_passable(position) {
            return Ok(position);
        }
        self.hard
            .nearest_passable(
                position,
                (max_radius_m / self.grid.resolution).ceil() as usize,
            )
            .ok_or_else(|| {
                CoreError::unusable(position.x, position.y, "no passable ground within reach")
            })
    }

    /// Packs a set of points into a projection batch.
    ///
    /// The batch is the interface the compute backends expose for "is this point
    /// on passable ground, how far is the nearest obstacle": the smoother, the
    /// lateral offset and the route attachment all ask it of many points at once,
    /// and none of the answers depends on another. Building it here keeps the field
    /// layout in one place instead of at every call site.
    pub fn projection_batch(&self, points: &[DVec2]) -> Result<crate::gpu::ProjectionBatch> {
        crate::gpu::projection_batch_from(
            &self.grid,
            self.hard.mask(),
            |point| self.distance.distance_at(point),
            points,
        )
    }

    /// Fraction of the map that is impassable.
    pub fn forbidden_ratio(&self) -> f64 {
        self.hard.forbidden_ratio()
    }
}

/// Runs the weighted feature sum through a compute backend.
fn weighted_sum_on_backend(
    features: &FeatureField,
    weights: &CostWeights,
    params: &CostModelParams,
    backend: &dyn crate::gpu::ComputeBackend,
) -> Result<Vec<f32>> {
    let grid = features.grid();
    let dims = features.dim();
    let direction = features.direction_index();

    // The tensor is channel-continuous so it can be uploaded as one buffer; the
    // direction dimension is zeroed because its cost depends on the heading.
    let mut tensor = vec![0.0f32; grid.len() * dims];
    for (dim, channel) in features.channels().iter().enumerate() {
        if Some(dim) == direction {
            continue;
        }
        for (cell, value) in channel.values.iter().enumerate() {
            tensor[cell * dims + dim] = *value;
        }
    }
    let tensor = crate::gpu::FeatureTensor::new(grid.len(), dims, tensor)?;
    let weight_vector: Vec<f32> = weights.values.iter().map(|value| *value as f32).collect();
    let weight_matrix = crate::gpu::WeightMatrix::from_vectors(&[weight_vector])?;
    let batch = backend.cost_field_batch(&tensor, &weight_matrix, params.c0 as f32)?;
    Ok((0..grid.len()).map(|cell| batch.get(cell, 0)).collect())
}

/// How the roadmap should be obtained.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PrmOptions {
    /// Do not use a roadmap; search the fine grid only.
    None,
    /// Load the batch stored in the map.
    Stored(u16),
    /// Sample a fresh roadmap with the given seed.
    Generate {
        /// Sampling seed.
        seed: u64,
        /// Sampling parameters.
        params: PrmParams,
    },
}

impl Default for PrmOptions {
    fn default() -> Self {
        PrmOptions::Generate {
            seed: 0x9E37,
            params: PrmParams::default(),
        }
    }
}
