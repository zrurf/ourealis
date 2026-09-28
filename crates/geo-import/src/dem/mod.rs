//! Digital elevation model access.
//!
//! [`DemSource`] downloads and caches tiles; [`DemMosaic`] hides the fact that a
//! box can need more than one of them and answers elevation queries in
//! geographic coordinates.

pub mod geotiff;
pub mod source;

pub use geotiff::DemRaster;
pub use source::{COPERNICUS_GLO30_BASE, DemSource};

use crate::error::{GeoError, Result};
use crate::geo::GeoBounds;

/// The DEM tiles covering a box.
#[derive(Debug, Clone)]
pub struct DemMosaic {
    tiles: Vec<DemRaster>,
}

impl DemMosaic {
    /// Loads every tile a box needs, downloading whatever is not cached.
    pub fn load(source: &DemSource, bounds: &GeoBounds) -> Result<Self> {
        let names = source.tiles_for(bounds);
        let mut tiles = Vec::with_capacity(names.len());
        for name in names {
            tiles.push(DemRaster::open(source.ensure_tile(&name)?)?);
        }
        Self::new(tiles)
    }

    /// Builds a mosaic from already decoded tiles.
    pub fn new(tiles: Vec<DemRaster>) -> Result<Self> {
        if tiles.is_empty() {
            return Err(GeoError::data(
                "a dem mosaic needs at least one tile, but none covers the requested box",
            ));
        }
        Ok(Self { tiles })
    }

    /// The decoded tiles, in the order they were requested.
    pub fn tiles(&self) -> &[DemRaster] {
        &self.tiles
    }

    /// Bilinearly samples the tile that covers a position.
    ///
    /// Tiles do not overlap, so the first covering tile is also the only one.
    /// `None` means the position is outside every tile or falls on a void.
    pub fn sample(&self, lon: f64, lat: f64) -> Option<f64> {
        self.tiles
            .iter()
            .find(|tile| tile.covers(lon, lat))
            .and_then(|tile| tile.sample(lon, lat))
    }
}
