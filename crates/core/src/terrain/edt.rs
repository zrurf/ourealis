//! Euclidean distance transform and its quantised gradient.
//!
//! Path smoothing pushes points away from obstacles along the distance field's
//! gradient. Differentiating the distance field at runtime is ill-conditioned
//! where the field is nearly flat — the centre of a wide plaza — so the OMF
//! format stores the gradient direction quantised to 16 compass directions, and
//! this type reproduces that convention when the layer is absent.

use glam::DVec2;

use ourealis_map_format::{LayerId, Map};

use super::grid::Grid2D;

/// Number of compass directions the gradient is quantised to.
pub const GRADIENT_DIRECTIONS: usize = 16;

/// Distance to the nearest obstacle, in metres, plus gradient direction.
#[derive(Debug, Clone)]
pub struct DistanceField {
    grid: Grid2D,
    distance: Vec<f64>,
    /// Quantised gradient direction index per cell, or `u8::MAX` when undefined.
    gradient: Vec<u8>,
}

impl DistanceField {
    /// Builds the transform from a forbidden mask.
    ///
    /// The distance is measured to the nearest forbidden cell centre using the
    /// exact Felzenszwalb–Huttenlocher algorithm: two passes of a 1D lower
    /// envelope of parabolas, which is linear in the number of cells.
    ///
    /// A `forbidden` slice shorter than the grid is read as far as it goes and
    /// the remaining cells count as passable, so a mismatched caller cannot make
    /// the transform index out of range.
    pub fn from_mask(grid: Grid2D, forbidden: &[bool]) -> Self {
        let width = grid.width;
        let height = grid.height;
        // Sentinel for empty cells. It must dwarf any real squared distance:
        // the parabola envelope picks the smallest `f + (q - x)^2`, so a sentinel
        // comparable to the map size would let empty columns beat real
        // obstacles. Infinity would poison the intersection arithmetic with
        // `inf - inf`, hence a large finite value.
        let big = 1.0e12;
        let mut distance: Vec<f64> = (0..grid.len())
            .map(|index| {
                if forbidden.get(index).copied().unwrap_or(false) {
                    0.0
                } else {
                    big
                }
            })
            .collect();

        // Along columns.
        let mut column = vec![0.0f64; height];
        for x in 0..width {
            for y in 0..height {
                column[y] = distance[grid.index(x, y)];
            }
            let transformed = transform_1d(&column);
            for y in 0..height {
                distance[grid.index(x, y)] = transformed[y];
            }
        }
        // Along rows.
        let mut row = vec![0.0f64; width];
        for y in 0..height {
            for x in 0..width {
                row[x] = distance[grid.index(x, y)];
            }
            let transformed = transform_1d(&row);
            for x in 0..width {
                distance[grid.index(x, y)] = transformed[x];
            }
        }

        for value in distance.iter_mut() {
            *value = value.max(0.0).sqrt() * grid.resolution;
        }
        let gradient = quantise_gradient(&grid, &distance, forbidden);
        Self {
            grid,
            distance,
            gradient,
        }
    }

    /// Loads the layer from a map when present.
    ///
    /// Returns `None` when the map carries no EDT layer; callers then build one
    /// from the hard mask. The stored layer takes precedence because it was
    /// produced by the preprocessing tools that own the format's conventions.
    pub fn from_map(map: &Map) -> Option<Self> {
        let view = map.layer(LayerId::EDT)?;
        if !view.has_level(0) {
            return None;
        }
        let grid = Grid2D::new(&map.header().bounds, map.grid().base_res_m);
        let mut distance = vec![0.0f64; grid.len()];
        let mut gradient = vec![u8::MAX; grid.len()];
        let (chunk_dim_x, chunk_dim_y) = map.grid().chunk_dims(0);
        let mut loaded = 0usize;
        for cy in 0..chunk_dim_y {
            for cx in 0..chunk_dim_x {
                let chunk_id =
                    ourealis_map_format::geometry::morton_encode_chunk(cx as u16, cy as u16);
                let Ok(Some(chunk)) = map.chunk(LayerId::EDT, 0, chunk_id) else {
                    continue;
                };
                let origin_x = cx as usize * map.grid().chunk_size as usize;
                let origin_y = cy as usize * map.grid().chunk_size as usize;
                for y in 0..chunk.height as usize {
                    for x in 0..chunk.width as usize {
                        let gx = origin_x + x;
                        let gy = origin_y + y;
                        if gx >= grid.width || gy >= grid.height {
                            continue;
                        }
                        let index = grid.index(gx, gy);
                        distance[index] = chunk.get(x as u32, y as u32, 0) as f64;
                        if chunk.channels > 1 {
                            gradient[index] = chunk.get(x as u32, y as u32, 1) as u8;
                        }
                        loaded += 1;
                    }
                }
            }
        }
        if loaded == 0 {
            return None;
        }
        Some(Self {
            grid,
            distance,
            gradient,
        })
    }

    /// Grid geometry.
    #[inline]
    pub fn grid(&self) -> &Grid2D {
        &self.grid
    }

    /// Distance to the nearest obstacle in metres.
    pub fn distance_at(&self, position: DVec2) -> f64 {
        let (x, y) = self.grid.cell_of(position);
        self.distance[self.grid.index(x, y)]
    }

    /// Unit vector pointing away from the nearest obstacle.
    ///
    /// Uses the cached quantised direction when available; otherwise falls back
    /// to a central difference of the distance field, which is only used for
    /// maps built without the derived layer.
    pub fn gradient_at(&self, position: DVec2) -> DVec2 {
        let (x, y) = self.grid.cell_of(position);
        let index = self.grid.index(x, y);
        let quantised = self.gradient[index];
        if quantised != u8::MAX {
            let angle = std::f64::consts::TAU * quantised as f64 / GRADIENT_DIRECTIONS as f64;
            return DVec2::new(angle.cos(), angle.sin());
        }
        self.finite_difference_gradient(x, y)
    }

    fn finite_difference_gradient(&self, x: usize, y: usize) -> DVec2 {
        let step = self.grid.resolution;
        let index = |dx: isize, dy: isize| -> f64 {
            let nx = (x as isize + dx).clamp(0, self.grid.width as isize - 1) as usize;
            let ny = (y as isize + dy).clamp(0, self.grid.height as isize - 1) as usize;
            self.distance[self.grid.index(nx, ny)]
        };
        let gx = (index(1, 0) - index(-1, 0)) / (2.0 * step);
        let gy = (index(0, 1) - index(0, -1)) / (2.0 * step);
        let gradient = DVec2::new(gx, gy);
        if gradient.length() <= 1e-9 {
            DVec2::ZERO
        } else {
            gradient.normalize()
        }
    }

    /// Distance samples, row-major.
    #[inline]
    pub fn distances(&self) -> &[f64] {
        &self.distance
    }
}

/// Felzenszwalb–Huttenlocher 1D squared-distance transform.
#[allow(clippy::needless_range_loop)]
fn transform_1d(values: &[f64]) -> Vec<f64> {
    let n = values.len();
    if n == 0 {
        return Vec::new();
    }
    let mut out = vec![0.0f64; n];
    let mut v = vec![0usize; n];
    let mut z = vec![0.0f64; n + 1];
    let mut k = 0usize;
    v[0] = 0;
    z[0] = f64::NEG_INFINITY;
    z[1] = f64::INFINITY;

    for q in 1..n {
        let mut s = ((values[q] + (q * q) as f64) - (values[v[k]] + (v[k] * v[k]) as f64))
            / (2.0 * q as f64 - 2.0 * v[k] as f64);
        while s <= z[k] {
            k -= 1;
            s = ((values[q] + (q * q) as f64) - (values[v[k]] + (v[k] * v[k]) as f64))
                / (2.0 * q as f64 - 2.0 * v[k] as f64);
        }
        k += 1;
        v[k] = q;
        z[k] = s;
        z[k + 1] = f64::INFINITY;
    }

    k = 0;
    for q in 0..n {
        while z[k + 1] < q as f64 {
            k += 1;
        }
        let d = q as f64 - v[k] as f64;
        out[q] = d * d + values[v[k]];
    }
    out
}

fn quantise_gradient(grid: &Grid2D, distance: &[f64], forbidden: &[bool]) -> Vec<u8> {
    let mut out = vec![u8::MAX; distance.len()];
    for y in 0..grid.height {
        for x in 0..grid.width {
            let index = grid.index(x, y);
            if forbidden.get(index).copied().unwrap_or(false) {
                // Inside an obstacle the direction is undefined; smoothing never
                // queries these cells because they are rejected up front.
                continue;
            }
            let sample = |dx: isize, dy: isize| -> f64 {
                let nx = (x as isize + dx).clamp(0, grid.width as isize - 1) as usize;
                let ny = (y as isize + dy).clamp(0, grid.height as isize - 1) as usize;
                distance[grid.index(nx, ny)]
            };
            let gx = sample(1, 0) - sample(-1, 0);
            let gy = sample(0, 1) - sample(0, -1);
            if gx.abs() < 1e-9 && gy.abs() < 1e-9 {
                continue;
            }
            let angle = gy.atan2(gx).rem_euclid(std::f64::consts::TAU);
            let step = std::f64::consts::TAU / GRADIENT_DIRECTIONS as f64;
            out[index] = (angle / step)
                .round()
                .rem_euclid(GRADIENT_DIRECTIONS as f64) as u8;
        }
    }
    out
}
