//! Dense raster grid geometry shared by the terrain and feature fields.

use glam::DVec2;

use ourealis_map_format::Aabb;

/// Row-major dense grid over the map's local metre plane.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Grid2D {
    /// Origin of the grid (centre of cell `(0, 0)` minus half a cell).
    pub origin: DVec2,
    /// Cell size in metres.
    pub resolution: f64,
    /// Cells along x.
    pub width: usize,
    /// Cells along y.
    pub height: usize,
}

impl Grid2D {
    /// Creates a grid covering `bounds` at the given resolution.
    pub fn new(bounds: &Aabb, resolution: f64) -> Self {
        let resolution = resolution.max(1e-3);
        Self {
            origin: DVec2::new(bounds.min_x, bounds.min_y),
            resolution,
            width: (bounds.width() / resolution).ceil().max(1.0) as usize,
            height: (bounds.height() / resolution).ceil().max(1.0) as usize,
        }
    }

    /// Number of cells.
    #[inline]
    pub fn len(&self) -> usize {
        self.width * self.height
    }

    /// True when the grid holds no cell.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Extent covered by the grid.
    pub fn bounds(&self) -> Aabb {
        Aabb::new(
            self.origin.x,
            self.origin.y,
            self.origin.x + self.width as f64 * self.resolution,
            self.origin.y + self.height as f64 * self.resolution,
        )
    }

    /// Centre of a cell.
    #[inline]
    pub fn cell_center(&self, x: usize, y: usize) -> DVec2 {
        self.origin
            + DVec2::new(
                (x as f64 + 0.5) * self.resolution,
                (y as f64 + 0.5) * self.resolution,
            )
    }

    /// Continuous grid coordinates of a world position.
    #[inline]
    pub fn continuous(&self, position: DVec2) -> DVec2 {
        (position - self.origin) / self.resolution - DVec2::splat(0.5)
    }

    /// Nearest cell of a world position, clamped into the grid.
    #[inline]
    pub fn cell_of(&self, position: DVec2) -> (usize, usize) {
        let continuous = self.continuous(position);
        (
            (continuous.x.round().max(0.0) as usize).min(self.width - 1),
            (continuous.y.round().max(0.0) as usize).min(self.height - 1),
        )
    }

    /// Linear index of a cell.
    #[inline]
    pub fn index(&self, x: usize, y: usize) -> usize {
        y * self.width + x
    }

    /// Cell coordinates of a linear index.
    #[inline]
    pub fn coordinates(&self, index: usize) -> (usize, usize) {
        (index % self.width, index / self.width)
    }

    /// True when a position lies inside the grid.
    #[inline]
    pub fn contains(&self, position: DVec2) -> bool {
        let continuous = self.continuous(position);
        continuous.x >= -0.5
            && continuous.y >= -0.5
            && continuous.x <= self.width as f64 - 0.5
            && continuous.y <= self.height as f64 - 0.5
    }
}
