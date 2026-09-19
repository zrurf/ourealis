//! Resistance feature field.
//!
//! The map stores objective environment descriptions: surface category,
//! traffic exposure, crowding, lighting, direction constraints. Nothing in the
//! file says how much any of that costs a runner — that is decided per motion
//! mode at runtime. This module loads those channels into dense grids and
//! normalises them into the `[0, 1]` range the cost model expects.

use glam::DVec2;

use ourealis_map_format::Map;
use ourealis_map_format::tlv::value::{FeatureDim, FeatureKind, FeatureSchema};

use crate::error::{CoreError, Result};
use crate::math::sampling::bilinear_f32;
use crate::terrain::Grid2D;

/// One loaded feature dimension.
#[derive(Debug, Clone)]
pub struct FeatureChannel {
    /// Schema entry describing the dimension.
    pub description: FeatureDim,
    /// Normalised values in `[0, 1]`; for direction channels the raw packed
    /// `angle | strength` value is preserved instead, see
    /// [`FeatureChannel::direction_at`].
    pub values: Vec<f32>,
    /// Minimum finite value over the grid, used for the heuristic bound.
    pub min: f32,
    /// Maximum finite value over the grid.
    pub max: f32,
}

impl FeatureChannel {
    /// True for the direction constraint dimension.
    pub fn is_direction(&self) -> bool {
        self.description.kind == FeatureKind::Direction
    }

    /// Direction preference at a position: preferred heading and strength.
    ///
    /// The stored value packs a 256-step angle in the high bits and the
    /// strength in the low byte, matching the on-disk convention.
    pub fn direction_at(&self, position: DVec2, grid: &Grid2D) -> Option<(f64, f64)> {
        if !self.is_direction() {
            return None;
        }
        let (x, y) = grid.cell_of(position);
        let packed = *self.values.get(grid.index(x, y))?;
        if packed <= 0.0 {
            return None;
        }
        let angle_index = (packed / 256.0).floor() as f64;
        let strength = packed as f64 - angle_index * 256.0;
        if strength <= 0.0 {
            return None;
        }
        let angle = angle_index / 256.0 * std::f64::consts::TAU;
        Some((angle, strength / 255.0))
    }
}

/// All resistance dimensions of the map, normalised and ready for the cost model.
#[derive(Debug, Clone)]
pub struct FeatureField {
    grid: Grid2D,
    channels: Vec<FeatureChannel>,
    direction_index: Option<usize>,
}

impl FeatureField {
    /// Loads every dimension declared by the map's feature schema.
    pub fn from_map(map: &Map) -> Result<Self> {
        let schema: FeatureSchema = map.feature_schema()?.ok_or(CoreError::MissingLayer {
            what: "feature schema (shared FEATURE_SCHEMA metadata)",
        })?;
        if schema.dims.is_empty() {
            return Err(CoreError::MissingLayer {
                what: "resistance feature dimensions",
            });
        }
        let grid = Grid2D::new(&map.header().bounds, map.grid().base_res_m);
        let mut channels = Vec::with_capacity(schema.dims.len());
        let mut direction_index = None;

        for (index, dim) in schema.dims.iter().enumerate() {
            let values = load_channel(map, &grid, dim)?;
            if dim.kind == FeatureKind::Direction {
                direction_index = Some(index);
            }
            let (min, max) = min_max(&values);
            channels.push(FeatureChannel {
                description: dim.clone(),
                values,
                min,
                max,
            });
        }

        Ok(Self {
            grid,
            channels,
            direction_index,
        })
    }

    /// Builds a field from explicit channels, used by tests.
    pub fn new(grid: Grid2D, channels: Vec<FeatureChannel>) -> Self {
        let direction_index = channels.iter().position(|c| c.is_direction());
        Self {
            grid,
            channels,
            direction_index,
        }
    }

    /// Grid geometry.
    #[inline]
    pub fn grid(&self) -> &Grid2D {
        &self.grid
    }

    /// Number of feature dimensions `D`.
    #[inline]
    pub fn dim(&self) -> usize {
        self.channels.len()
    }

    /// Loaded channels.
    #[inline]
    pub fn channels(&self) -> &[FeatureChannel] {
        &self.channels
    }

    /// Index of the direction constraint dimension, when the map has one.
    #[inline]
    pub fn direction_index(&self) -> Option<usize> {
        self.direction_index
    }

    /// Normalised value of a dimension at a position.
    pub fn value_at(&self, dim: usize, position: DVec2) -> f64 {
        let Some(channel) = self.channels.get(dim) else {
            return 0.0;
        };
        if channel.is_direction() {
            return 0.0;
        }
        let continuous = self.grid.continuous(position);
        bilinear_f32(
            &channel.values,
            self.grid.width,
            self.grid.height,
            continuous.x,
            continuous.y,
        )
    }

    /// Normalised values of every dimension at a position.
    pub fn values_at(&self, position: DVec2) -> Vec<f64> {
        (0..self.channels.len())
            .map(|dim| self.value_at(dim, position))
            .collect()
    }

    /// Per-dimension minimum over the grid, for the heuristic's lower bound.
    pub fn minima(&self) -> Vec<f64> {
        self.channels
            .iter()
            .map(|channel| {
                if channel.is_direction() {
                    0.0
                } else {
                    channel.min as f64
                }
            })
            .collect()
    }
}

fn load_channel(map: &Map, grid: &Grid2D, dim: &FeatureDim) -> Result<Vec<f32>> {
    let mut values = vec![0.0f32; grid.len()];
    if map.layer_desc(dim.layer_id).is_none() {
        return Err(CoreError::MissingLayer {
            what: "resistance feature layer",
        });
    }
    let (chunk_dim_x, chunk_dim_y) = map.grid().chunk_dims(0);
    let palette = dim.palette.clone();
    for cy in 0..chunk_dim_y {
        for cx in 0..chunk_dim_x {
            let chunk_id = ourealis_map_format::geometry::morton_encode_chunk(cx as u16, cy as u16);
            let Some(chunk) = map.chunk(dim.layer_id, 0, chunk_id)? else {
                continue;
            };
            if dim.channel >= chunk.channels {
                continue;
            }
            let origin_x = cx as usize * map.grid().chunk_size as usize;
            let origin_y = cy as usize * map.grid().chunk_size as usize;
            for y in 0..chunk.height as usize {
                for x in 0..chunk.width as usize {
                    let gx = origin_x + x;
                    let gy = origin_y + y;
                    if gx >= grid.width || gy >= grid.height {
                        continue;
                    }
                    let raw = chunk.get(x as u32, y as u32, dim.channel) as f64;
                    let normalised = normalise(dim, raw, &palette);
                    values[grid.index(gx, gy)] = normalised as f32;
                }
            }
        }
    }
    Ok(values)
}

/// Maps a raw channel value into the cost model's range.
///
/// Categories resolve through the palette; direction channels keep their packed
/// `angle | strength` value because the cost depends on the travel direction and
/// is therefore evaluated per query rather than per cell.
fn normalise(dim: &FeatureDim, raw: f64, palette: &[f32]) -> f64 {
    match dim.kind {
        FeatureKind::Direction => raw,
        FeatureKind::Category => {
            let index = raw.round().max(0.0) as usize;
            palette.get(index).copied().unwrap_or(0.0) as f64
        }
        FeatureKind::Boolean => {
            if raw != 0.0 {
                1.0
            } else {
                0.0
            }
        }
        FeatureKind::Scalar => {
            let span = (dim.norm_max - dim.norm_min) as f64;
            if span.abs() < f64::EPSILON {
                0.0
            } else {
                ((raw - dim.norm_min as f64) / span).clamp(0.0, 1.0)
            }
        }
    }
}

fn min_max(values: &[f32]) -> (f32, f32) {
    let mut min = f32::INFINITY;
    let mut max = f32::NEG_INFINITY;
    for value in values {
        if value.is_finite() {
            min = min.min(*value);
            max = max.max(*value);
        }
    }
    if min > max { (0.0, 0.0) } else { (min, max) }
}
