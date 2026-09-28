//! What the importer is asked to build.

use std::path::{Path, PathBuf};

use crate::error::{GeoError, Result};
use crate::facilities::Overlay;
use crate::geo::GeoBounds;
use crate::osm::DEFAULT_OVERPASS_ENDPOINT;
use crate::raster::Grid;

/// Cell size used when the caller does not pick one, metres.
///
/// Copernicus GLO-30 samples at about 30 m and OSM geometry is surveyed to a few
/// metres; 2 m keeps a footpath and a building wall apart without storing a grid
/// three orders of magnitude larger than the data behind it.
pub const DEFAULT_RESOLUTION_M: f64 = 2.0;

/// Chunk side length in cells.
pub const DEFAULT_CHUNK_SIZE: u16 = 128;

/// Margin grown around every building footprint, metres.
pub const DEFAULT_BUILDING_MARGIN_M: f64 = 1.0;

/// Finest cell size the OMF header can record, metres.
pub const MIN_RESOLUTION_M: f64 = 0.01;

/// Coarsest cell size the OMF header can record, metres.
///
/// The header stores the resolution as a whole number of centimetres.
pub const MAX_RESOLUTION_M: f64 = 655.35;

/// The box, resolution and sources of one import.
#[derive(Debug, Clone, PartialEq)]
pub struct ImportSpec {
    /// Map name recorded in the file's metadata block.
    pub name: String,
    /// Geographic box to cover.
    pub bounds: GeoBounds,
    /// Requested cell size in metres.
    pub resolution_m: f64,
    /// Chunk side length in cells.
    pub chunk_size: u16,
    /// Directory the DEM tiles are cached in.
    pub dem_cache: PathBuf,
    /// Overpass endpoint to query.
    pub overpass_endpoint: String,
    /// Margin grown around every building footprint, metres.
    pub building_margin_m: f64,
    /// Hand-surveyed facilities to overlay on the OSM extract.
    ///
    /// `None` imports the extract as it stands; a campus whose tracks OSM draws
    /// wrongly or not at all carries the survey that fixes them.
    pub overlay: Option<Overlay>,
}

impl ImportSpec {
    /// A spec with the importer's defaults.
    pub fn new(name: impl Into<String>, bounds: GeoBounds) -> Self {
        Self {
            name: name.into(),
            bounds,
            resolution_m: DEFAULT_RESOLUTION_M,
            chunk_size: DEFAULT_CHUNK_SIZE,
            dem_cache: default_dem_cache(),
            overpass_endpoint: DEFAULT_OVERPASS_ENDPOINT.to_owned(),
            building_margin_m: DEFAULT_BUILDING_MARGIN_M,
            overlay: None,
        }
    }

    /// Replaces the cell size.
    pub fn with_resolution_m(mut self, resolution_m: f64) -> Self {
        self.resolution_m = resolution_m;
        self
    }

    /// Replaces the chunk side length.
    pub fn with_chunk_size(mut self, chunk_size: u16) -> Self {
        self.chunk_size = chunk_size;
        self
    }

    /// Replaces the DEM cache directory.
    pub fn with_dem_cache(mut self, dem_cache: impl Into<PathBuf>) -> Self {
        self.dem_cache = dem_cache.into();
        self
    }

    /// Replaces the Overpass endpoint.
    pub fn with_overpass_endpoint(mut self, endpoint: impl Into<String>) -> Self {
        self.overpass_endpoint = endpoint.into();
        self
    }

    /// Replaces the building margin.
    pub fn with_building_margin_m(mut self, margin_m: f64) -> Self {
        self.building_margin_m = margin_m;
        self
    }

    /// Overlays hand-surveyed facilities on the OSM extract.
    pub fn with_overlay(mut self, overlay: Overlay) -> Self {
        self.overlay = Some(overlay);
        self
    }

    /// Cell size the map is actually rasterised at, metres.
    ///
    /// The header records the resolution in whole centimetres and every reader
    /// derives cell geometry from that field, so the raster must use the rounded
    /// value: rasterising at the raw one would place every cell a fraction of a
    /// centimetre away from where the file says it is.
    pub fn effective_resolution_m(&self) -> f64 {
        ((self.resolution_m * 100.0).round() / 100.0).clamp(MIN_RESOLUTION_M, MAX_RESOLUTION_M)
    }

    /// Grid the spec describes.
    pub fn grid(&self) -> Result<Grid> {
        Grid::new(self.bounds, self.effective_resolution_m())
    }

    /// Checks the spec before anything is downloaded for it.
    pub fn validate(&self) -> Result<()> {
        if self.name.trim().is_empty() {
            return Err(GeoError::invalid("the map needs a name"));
        }
        if !self.resolution_m.is_finite() {
            return Err(GeoError::invalid("the cell size must be finite"));
        }
        if self.resolution_m < MIN_RESOLUTION_M || self.resolution_m > MAX_RESOLUTION_M {
            return Err(GeoError::invalid(format!(
                "cell size {} m is outside the {} m to {} m the header can record",
                self.resolution_m, MIN_RESOLUTION_M, MAX_RESOLUTION_M
            )));
        }
        if self.chunk_size == 0 {
            return Err(GeoError::invalid("the chunk size must not be zero"));
        }
        if !self.building_margin_m.is_finite() || self.building_margin_m < 0.0 {
            return Err(GeoError::invalid(
                "the building margin must be a non-negative length",
            ));
        }
        if self.dem_cache.as_os_str().is_empty() {
            return Err(GeoError::invalid(
                "the dem cache directory must not be empty",
            ));
        }
        // Building the grid is what rejects an extent or a resolution that would
        // need an unbounded allocation.
        self.grid().map(|_| ())
    }

    /// Resolution the map will be rasterised at, for diagnostics.
    pub fn cell_dims(&self) -> Result<(u32, u32)> {
        self.grid().map(|grid| grid.dims())
    }
}

/// Directory the workspace keeps downloaded datasets in.
pub fn workspace_data_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data")
}

/// Default DEM cache directory.
pub fn default_dem_cache() -> PathBuf {
    workspace_data_dir().join("dem")
}
