//! The cell grid the importer rasterises into.
//!
//! The grid is anchored at the south-west corner of the request: cell `(0, 0)`
//! is the first cell inward from that corner, and the origin of the local metre
//! plane is exactly the corner itself. The extent is rounded up to a whole
//! number of cells, so the map covers slightly more than the box that was asked
//! for; every geographic position inside the request therefore lands inside the
//! grid rather than on its edge.

use ourealis_map_format::Aabb;

use crate::error::{GeoError, Result};
use crate::geo::GeoBounds;

/// Largest grid the importer will rasterise, in cells.
///
/// The importer holds one `f32` per cell per layer and builds seven of them, so
/// an unbounded request is not a slow run but an aborting allocation. Sixteen
/// million cells is a 4 km square at 1 m and about 64 MB per layer, well above
/// the campus scale this crate is meant for.
pub const MAX_CELLS: u64 = 16_000_000;

/// Regular grid of square cells over a geographic box.
#[derive(Debug, Clone, PartialEq)]
pub struct Grid {
    width: u32,
    height: u32,
    res_m: f64,
    geo: GeoBounds,
}

impl Grid {
    /// Builds a grid whose cells are `res_m` on a side.
    pub fn new(geo: GeoBounds, res_m: f64) -> Result<Self> {
        if !res_m.is_finite() || res_m <= 0.0 {
            return Err(GeoError::invalid(format!(
                "cell size {res_m} m is not a positive length"
            )));
        }
        let width = (geo.width_m() / res_m).ceil() as u32;
        let height = (geo.height_m() / res_m).ceil() as u32;
        if width == 0 || height == 0 {
            return Err(GeoError::invalid(format!(
                "a {} x {} m box is smaller than one {res_m} m cell",
                geo.width_m(),
                geo.height_m()
            )));
        }
        let cells = u64::from(width) * u64::from(height);
        if cells > MAX_CELLS {
            return Err(GeoError::invalid(format!(
                "a {width} x {height} grid needs {cells} cells, above the {MAX_CELLS} limit; \
                 raise the cell size or shrink the box"
            )));
        }
        Ok(Self {
            width,
            height,
            res_m,
            geo,
        })
    }

    /// Cell count per axis, west to east then south to north.
    #[inline]
    pub fn dims(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// Cells per row.
    #[inline]
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Rows.
    #[inline]
    pub fn height(&self) -> u32 {
        self.height
    }

    /// Total cell count.
    #[inline]
    pub fn len(&self) -> usize {
        self.width as usize * self.height as usize
    }

    /// Cell side length in metres.
    #[inline]
    pub fn res_m(&self) -> f64 {
        self.res_m
    }

    /// East-west extent of the grid in metres, rounded up to whole cells.
    #[inline]
    pub fn width_m(&self) -> f64 {
        self.width as f64 * self.res_m
    }

    /// North-south extent of the grid in metres, rounded up to whole cells.
    #[inline]
    pub fn height_m(&self) -> f64 {
        self.height as f64 * self.res_m
    }

    /// Geographic box the grid was built from.
    #[inline]
    pub fn geo(&self) -> GeoBounds {
        self.geo
    }

    /// Extent of the grid in the local metre plane, as the map header wants it.
    pub fn extent(&self) -> Aabb {
        Aabb::new(0.0, 0.0, self.width_m(), self.height_m())
    }

    /// Linear index of a cell. The caller must have bounds-checked it.
    #[inline]
    pub fn index(&self, x: u32, y: u32) -> usize {
        debug_assert!(x < self.width && y < self.height);
        y as usize * self.width as usize + x as usize
    }

    /// Centre of a cell in the local metre plane.
    #[inline]
    pub fn cell_center(&self, x: u32, y: u32) -> (f64, f64) {
        ((x as f64 + 0.5) * self.res_m, (y as f64 + 0.5) * self.res_m)
    }

    /// Projects a geographic position into the local metre plane.
    #[inline]
    pub fn local_of_geo(&self, lon: f64, lat: f64) -> (f64, f64) {
        self.geo.project(lon, lat)
    }

    /// Projects a local metre position back to `(longitude, latitude)`.
    ///
    /// The inverse of [`Grid::local_of_geo`] under the same spherical
    /// approximation, used to ask the DEM for the elevation of a cell centre.
    pub fn geo_of_local(&self, x: f64, y: f64) -> (f64, f64) {
        let cos_lat = self.geo.south.to_radians().cos();
        (
            self.geo.west + (x / (crate::geo::EARTH_RADIUS_M * cos_lat)).to_degrees(),
            self.geo.south + (y / crate::geo::EARTH_RADIUS_M).to_degrees(),
        )
    }

    /// Inclusive index range of the cells whose centres fall inside the metre
    /// span `[lo, hi]` on one axis, clamped to the grid.
    ///
    /// `None` when the span misses the grid entirely or is not finite, which is
    /// how callers prune the work of painting a long way: only the cells its
    /// bounding box can reach are ever visited.
    pub fn axis_cells(&self, lo: f64, hi: f64, cells: u32) -> Option<(u32, u32)> {
        if !lo.is_finite() || !hi.is_finite() || cells == 0 {
            return None;
        }
        let last_index = cells as f64 - 1.0;
        let first = (lo / self.res_m - 0.5).ceil();
        let last = (hi / self.res_m - 0.5).floor();
        if last < 0.0 || first > last_index {
            return None;
        }
        let first = first.max(0.0);
        let last = last.min(last_index);
        if first > last {
            return None;
        }
        Some((first as u32, last as u32))
    }
}
