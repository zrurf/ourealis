//! Hard constraint mask.
//!
//! Hard constraints — motorway prohibition, walls, water — never enter a linear
//! cost sum, where they could be diluted by other low-cost dimensions. They are
//! a boolean: a cell is passable or it is not, and every stage that can produce
//! a position consults this mask directly.

use glam::DVec2;

use ourealis_map_format::{LayerId, Map};

use crate::error::{CoreError, Result};
use crate::terrain::Grid2D;
use ourealis_map_format::Aabb;

/// Boolean passability grid.
#[derive(Debug, Clone)]
pub struct HardMask {
    grid: Grid2D,
    forbidden: Vec<bool>,
    forbidden_count: usize,
}

impl HardMask {
    /// Loads the bitmap layer from a map.
    pub fn from_map(map: &Map) -> Result<Self> {
        let grid = Grid2D::new(&map.header().bounds, map.grid().base_res_m);
        let mut forbidden = vec![false; grid.len()];
        let (chunk_dim_x, chunk_dim_y) = map.grid().chunk_dims(0);
        let mut loaded = 0usize;
        for cy in 0..chunk_dim_y {
            for cx in 0..chunk_dim_x {
                let chunk_id =
                    ourealis_map_format::geometry::morton_encode_chunk(cx as u16, cy as u16);
                let Some(chunk) = map.chunk(LayerId::HARD_FORBIDDEN, 0, chunk_id)? else {
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
                        let blocked = chunk.get(x as u32, y as u32, 0) != 0.0;
                        let index = grid.index(gx, gy);
                        forbidden[index] = blocked;
                        if blocked {
                            loaded += 0;
                        }
                    }
                }
                loaded += 1;
            }
        }
        if loaded == 0 {
            return Err(CoreError::MissingLayer {
                what: "hard constraint bitmap (layer 0x2001)",
            });
        }
        let forbidden_count = forbidden.iter().filter(|blocked| **blocked).count();
        Ok(Self {
            grid,
            forbidden,
            forbidden_count,
        })
    }

    /// Builds a mask from an explicit grid.
    ///
    /// A `forbidden` vector shorter than the grid is padded with passable cells:
    /// every query indexes the mask by grid cell, so a short vector would
    /// otherwise panic at the first query beyond its end.
    pub fn new(grid: Grid2D, mut forbidden: Vec<bool>) -> Self {
        forbidden.resize(grid.len(), false);
        let forbidden_count = forbidden.iter().filter(|blocked| **blocked).count();
        Self {
            grid,
            forbidden,
            forbidden_count,
        }
    }

    /// Grid geometry.
    #[inline]
    pub fn grid(&self) -> &Grid2D {
        &self.grid
    }

    /// Raw mask samples.
    #[inline]
    pub fn mask(&self) -> &[bool] {
        &self.forbidden
    }

    /// Number of forbidden cells.
    #[inline]
    pub fn forbidden_count(&self) -> usize {
        self.forbidden_count
    }

    /// Fraction of the map that is forbidden.
    pub fn forbidden_ratio(&self) -> f64 {
        if self.forbidden.is_empty() {
            0.0
        } else {
            self.forbidden_count as f64 / self.forbidden.len() as f64
        }
    }

    /// True when the cell containing `position` is passable.
    ///
    /// Positions outside the map are reported as forbidden: the simulator has
    /// no environment description beyond the bounds.
    pub fn is_passable(&self, position: DVec2) -> bool {
        if !self.grid.contains(position) {
            return false;
        }
        let (x, y) = self.grid.cell_of(position);
        !self.forbidden[self.grid.index(x, y)]
    }

    /// True when the cell containing `position` is forbidden.
    pub fn is_forbidden(&self, position: DVec2) -> bool {
        !self.is_passable(position)
    }

    /// True when a straight segment stays on passable cells.
    ///
    /// See [`segment_is_clear`] for the traversal this delegates to.
    ///
    /// Walks the cells the segment actually enters — an Amanatides–Woo grid
    /// traversal — rather than sampling along it. Sampling has to choose a step,
    /// and any step can miss a cell that the segment only clips: a shortcut that
    /// cuts a corner of a building has a chord shorter than the step, with both
    /// endpoints legal. The traversal visits exactly the cells the segment
    /// touches, so nothing can hide between two samples.
    ///
    /// Cells are half-open in `[min, min + resolution)`, the same convention
    /// [`Grid2D::cell_of`] uses, so a segment running exactly along a grid line
    /// stays in one column of cells instead of being reported as entering both.
    pub fn segment_is_clear(&self, from: DVec2, to: DVec2) -> bool {
        segment_is_clear(&self.grid, &self.forbidden, from, to)
    }

    /// Nearest passable cell centre, searching outwards in rings.
    ///
    /// The nearest one, not the first one the ring order happens to reach: the
    /// rings sweep every cell at a Chebyshev distance, so comparing centre
    /// distances within them is what makes the snapped start or goal the closest
    /// legal position rather than whichever cell the scan met first.
    ///
    /// Returns `None` only when no passable cell lies within `max_radius_cells`.
    pub fn nearest_passable(&self, position: DVec2, max_radius_cells: usize) -> Option<DVec2> {
        if self.is_passable(position) {
            return Some(position);
        }
        // `cell_of` clamps into the grid, so a position far outside it would
        // otherwise "snap" onto the border and silently accept a goal that the
        // map does not describe at all.
        let reach = max_radius_cells as f64 * self.grid.resolution;
        let bounds = self.grid.bounds();
        let outside = Aabb::new(
            bounds.min_x - reach,
            bounds.min_y - reach,
            bounds.max_x + reach,
            bounds.max_y + reach,
        );
        if !outside.contains(position.x, position.y) {
            return None;
        }
        let (cx, cy) = self.grid.cell_of(position);
        let mut best: Option<(f64, DVec2)> = None;
        for radius in 1..=max_radius_cells {
            // Everything from this ring outwards is at least `radius - 1` cells
            // away, so a hit closer than that cannot be beaten.
            if let Some((best_distance, _)) = best
                && (radius as f64 - 1.0) * self.grid.resolution > best_distance
            {
                break;
            }
            let r = radius as isize;
            for dy in -r..=r {
                for dx in -r..=r {
                    if dx.abs() != r && dy.abs() != r {
                        continue;
                    }
                    let x = cx as isize + dx;
                    let y = cy as isize + dy;
                    if x < 0
                        || y < 0
                        || x >= self.grid.width as isize
                        || y >= self.grid.height as isize
                    {
                        continue;
                    }
                    let (x, y) = (x as usize, y as usize);
                    if self.forbidden[self.grid.index(x, y)] {
                        continue;
                    }
                    let center = self.grid.cell_center(x, y);
                    let distance = (center - position).length();
                    if best
                        .as_ref()
                        .map(|(best_distance, _)| distance < *best_distance)
                        .unwrap_or(true)
                    {
                        best = Some((distance, center));
                    }
                }
            }
        }
        best.map(|(_, center)| center)
    }
}

/// Walks the cells a segment enters, returning false as soon as one is forbidden.
///
/// Sampling along the segment has to choose a step, and any step can miss a cell
/// the segment only clips: a shortcut cutting a building's corner has a chord
/// shorter than the step, with both endpoints legal. The traversal visits exactly
/// the cells the segment touches, so nothing can hide between two samples, and it
/// costs one step per cell boundary crossed rather than per metre.
///
/// Cells are half-open in `[min, min + resolution)`, the convention
/// [`Grid2D::cell_of`] uses, so a segment running exactly along a grid line stays
/// in one column instead of being reported as entering both.
pub fn segment_is_clear(grid: &Grid2D, forbidden: &[bool], from: DVec2, to: DVec2) -> bool {
    let resolution = grid.resolution;
    if resolution <= 0.0 || !from.is_finite() || !to.is_finite() {
        return false;
    }
    let bounds = grid.bounds();
    let cell_passable = |x: i64, y: i64| -> bool {
        if x < 0 || y < 0 || x >= grid.width as i64 || y >= grid.height as i64 {
            return false;
        }
        !forbidden[grid.index(x as usize, y as usize)]
    };
    // A point the grid claims to cover has to map to a cell of it. The closed
    // upper edge is the case that differs: `contains` and `cell_of` accept
    // `max_x`, while the raw floor would put it one cell past the last column and
    // reject every segment touching it. Points outside the grid keep the raw
    // index, which is what makes the traversal refuse them.
    let cell_index = |point: DVec2| -> (i64, i64) {
        let mut x = ((point.x - bounds.min_x) / resolution).floor() as i64;
        let mut y = ((point.y - bounds.min_y) / resolution).floor() as i64;
        if bounds.contains(point.x, point.y) {
            x = x.clamp(0, grid.width as i64 - 1);
            y = y.clamp(0, grid.height as i64 - 1);
        }
        (x, y)
    };
    let delta = to - from;
    let (mut x, mut y) = cell_index(from);
    let (end_x, end_y) = cell_index(to);

    let step_x: i64 = if delta.x > 0.0 {
        1
    } else if delta.x < 0.0 {
        -1
    } else {
        0
    };
    let step_y: i64 = if delta.y > 0.0 {
        1
    } else if delta.y < 0.0 {
        -1
    } else {
        0
    };
    let mut t_max_x = if step_x == 0 {
        f64::INFINITY
    } else {
        let boundary =
            bounds.min_x + if step_x > 0 { (x + 1) as f64 } else { x as f64 } * resolution;
        (boundary - from.x) / delta.x
    };
    let mut t_max_y = if step_y == 0 {
        f64::INFINITY
    } else {
        let boundary =
            bounds.min_y + if step_y > 0 { (y + 1) as f64 } else { y as f64 } * resolution;
        (boundary - from.y) / delta.y
    };
    let t_step_x = if step_x == 0 {
        f64::INFINITY
    } else {
        resolution / delta.x.abs()
    };
    let t_step_y = if step_y == 0 {
        f64::INFINITY
    } else {
        resolution / delta.y.abs()
    };

    // One step per boundary crossed, plus the starting cell.
    let budget = (end_x - x).unsigned_abs() + (end_y - y).unsigned_abs() + 2;
    for _ in 0..budget {
        if !cell_passable(x, y) {
            return false;
        }
        if x == end_x && y == end_y {
            return true;
        }
        // Only a crossing strictly inside the segment moves it into the next cell.
        // An endpoint lying exactly on a boundary belongs, by `floor`, to the cell
        // on the far side — the same cell the crossing would have left — so
        // stepping there would visit a cell the segment never enters. That is not
        // hypothetical: a goal snapped onto a cell corner ends at `t = 1.0` in
        // both axes at once.
        let next_x = step_x != 0 && t_max_x < 1.0;
        let next_y = step_y != 0 && t_max_y < 1.0;
        if !next_x && !next_y {
            return cell_passable(end_x, end_y);
        }
        if next_x && (!next_y || t_max_x <= t_max_y) {
            x += step_x;
            t_max_x += t_step_x;
        } else {
            y += step_y;
            t_max_y += t_step_y;
        }
    }
    // The budget is exact, so this is unreachable; "not clear" is the
    // conservative answer if rounding ever gets there.
    false
}
