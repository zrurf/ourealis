//! Elevation and slope access.
//!
//! The elevation layer is loaded once into a dense grid. Slopes are *not*
//! cached: they are a central difference of four bilinear samples, which is
//! cheaper than the memory a second grid would cost and keeps the terrain
//! consistent with whatever resolution the map provides.

use glam::DVec2;

use ourealis_map_format::{LayerId, Map};

use crate::error::{CoreError, Result};
use crate::math::sampling::bilinear;

use super::grid::Grid2D;

/// Elevation field over the map.
#[derive(Debug, Clone)]
pub struct Terrain {
    grid: Grid2D,
    heights: Vec<f64>,
}

impl Terrain {
    /// Loads the elevation layer from a map.
    ///
    /// Missing chunks are filled with the value of the nearest loaded cell so
    /// that the field stays continuous over the whole extent.
    pub fn from_map(map: &Map) -> Result<Self> {
        let grid = Grid2D::new(&map.header().bounds, map.grid().base_res_m);
        let mut heights = vec![f64::NAN; grid.len()];
        let mut loaded = 0usize;

        let (chunk_dim_x, chunk_dim_y) = map.grid().chunk_dims(0);
        for cy in 0..chunk_dim_y {
            for cx in 0..chunk_dim_x {
                let Some(chunk_id) = Some(ourealis_map_format::geometry::morton_encode_chunk(
                    cx as u16, cy as u16,
                )) else {
                    continue;
                };
                let Some(chunk) = map.chunk(LayerId::ELEVATION, 0, chunk_id)? else {
                    continue;
                };
                let (origin_x, origin_y) = (
                    cx as usize * map.grid().chunk_size as usize,
                    cy as usize * map.grid().chunk_size as usize,
                );
                for y in 0..chunk.height as usize {
                    for x in 0..chunk.width as usize {
                        let gx = origin_x + x;
                        let gy = origin_y + y;
                        if gx >= grid.width || gy >= grid.height {
                            continue;
                        }
                        heights[grid.index(gx, gy)] = chunk.get(x as u32, y as u32, 0) as f64;
                        loaded += 1;
                    }
                }
            }
        }

        if loaded == 0 {
            return Err(CoreError::MissingLayer {
                what: "elevation raster (layer 0x0001)",
            });
        }
        fill_missing(&grid, &mut heights);
        Ok(Self { grid, heights })
    }

    /// Builds a flat terrain, used by tests that do not need relief.
    pub fn flat(bounds: &ourealis_map_format::Aabb, resolution: f64, height: f64) -> Self {
        let grid = Grid2D::new(bounds, resolution);
        Self {
            grid,
            heights: vec![height; grid.len()],
        }
    }

    /// Grid geometry.
    #[inline]
    pub fn grid(&self) -> &Grid2D {
        &self.grid
    }

    /// Raw height samples.
    #[inline]
    pub fn heights(&self) -> &[f64] {
        &self.heights
    }

    /// Bilinear elevation at a position, in metres.
    pub fn height_at(&self, position: DVec2) -> f64 {
        let c = self.grid.continuous(position);
        bilinear(&self.heights, self.grid.width, self.grid.height, c.x, c.y)
    }

    /// Elevation gradient `grad h` in metres per metre.
    ///
    /// Uses a central difference over a one-cell span; the grid resolution is
    /// far below the scale over which real terrain curvature matters.
    pub fn gradient_at(&self, position: DVec2) -> DVec2 {
        let step = self.grid.resolution;
        let dx = (self.height_at(position + DVec2::new(step, 0.0))
            - self.height_at(position - DVec2::new(step, 0.0)))
            / (2.0 * step);
        let dy = (self.height_at(position + DVec2::new(0.0, step))
            - self.height_at(position - DVec2::new(0.0, step)))
            / (2.0 * step);
        DVec2::new(dx, dy)
    }

    /// Terrain-intrinsic slope magnitude `i = |grad h|`.
    pub fn slope_magnitude_at(&self, position: DVec2) -> f64 {
        self.gradient_at(position).length()
    }

    /// Slope component along a horizontal direction `i_parallel = grad h . v`.
    ///
    /// Positive means uphill: the gradient points towards increasing elevation,
    /// so a direction aligned with it climbs.
    pub fn directional_slope_at(&self, position: DVec2, direction: DVec2) -> f64 {
        let length = direction.length();
        if length <= f64::EPSILON {
            return 0.0;
        }
        self.gradient_at(position).dot(direction / length)
    }
}

/// Replaces `NaN` cells with the nearest loaded value.
fn fill_missing(grid: &Grid2D, heights: &mut [f64]) {
    if !heights.iter().any(|h| h.is_nan()) {
        return;
    }
    let fallback = heights.iter().copied().find(|h| !h.is_nan()).unwrap_or(0.0);
    // Forward pass propagates from the top-left, backward from the bottom-right;
    // two sweeps are enough to fill any hole in practice and are cheap.
    for index in 0..heights.len() {
        if !heights[index].is_nan() {
            continue;
        }
        let (x, y) = grid.coordinates(index);
        let mut value = fallback;
        if x > 0 && !heights[index - 1].is_nan() {
            value = heights[index - 1];
        } else if y > 0 && !heights[index - grid.width].is_nan() {
            value = heights[index - grid.width];
        }
        heights[index] = value;
    }
    for index in (0..heights.len()).rev() {
        if !heights[index].is_nan() {
            continue;
        }
        let (x, y) = grid.coordinates(index);
        let mut value = fallback;
        if x + 1 < grid.width && !heights[index + 1].is_nan() {
            value = heights[index + 1];
        } else if y + 1 < grid.height && !heights[index + grid.width].is_nan() {
            value = heights[index + grid.width];
        }
        heights[index] = value;
    }
    for value in heights.iter_mut() {
        if value.is_nan() {
            *value = fallback;
        }
    }
}
