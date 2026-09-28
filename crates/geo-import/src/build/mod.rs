//! Fetching the source datasets and turning them into an OMF image.
//!
//! The three stages are separate so that everything except the download can be
//! tested offline: [`fetch`] retrieves the DEM tiles and the Overpass extract,
//! [`rasterise`] projects both onto the grid, and [`omf::assemble`] encodes the
//! result. Fetching is the only stage that touches the network.

pub mod omf;
pub mod spec;

pub use omf::{assemble, feature, feature_schema, weight_prior};
pub use spec::{ImportSpec, default_dem_cache, workspace_data_dir};

use std::path::Path;

use crate::dem::{DemMosaic, DemSource};
use crate::error::Result;
use crate::osm::{self, OsmData};
use crate::raster::{self, Layers};

/// Everything one import downloads, before anything is rasterised.
#[derive(Debug, Clone)]
pub struct ImportData {
    /// Ways and areas covering the request.
    pub osm: OsmData,
    /// Terrain tiles covering the request.
    pub dem: DemMosaic,
}

/// Downloads the datasets a spec needs.
pub fn fetch(spec: &ImportSpec) -> Result<ImportData> {
    spec.validate()?;
    let grid = spec.grid()?;
    tracing::info!(
        width_m = grid.width_m(),
        height_m = grid.height_m(),
        cells_x = grid.width(),
        cells_y = grid.height(),
        res_m = grid.res_m(),
        "import grid"
    );

    let source = DemSource::copernicus_glo30(spec.dem_cache.clone());
    let dem = DemMosaic::load(&source, &spec.bounds)?;
    tracing::info!(tiles = dem.tiles().len(), "loaded dem");
    let mut osm = osm::fetch(&spec.bounds, &spec.overpass_endpoint)?;
    if let Some(overlay) = &spec.overlay {
        overlay.apply(&grid, &mut osm);
    }
    tracing::info!(
        roads = osm.roads.len(),
        areas = osm.areas.len(),
        "loaded osm"
    );
    Ok(ImportData { osm, dem })
}

/// Paints the downloaded data onto the import grid.
pub fn rasterise(spec: &ImportSpec, data: &ImportData) -> Result<Layers> {
    let grid = spec.grid()?;
    let mut layers = raster::rasterise(&grid, &data.osm, spec.building_margin_m);
    layers.elevation = raster::elevation::sample(&grid, &data.dem);
    // Buildings are raised after the terrain is sampled, so a footprint stands on
    // the ground it was surveyed on rather than replacing it: the elevation a
    // building cell carries is the ground plus its own height, and the step
    // between two neighbouring cells is the wall a viewer sees.
    for (elevation, height) in layers.elevation.iter_mut().zip(&layers.building_height) {
        *elevation += height;
    }
    Ok(layers)
}

/// Fetches, rasterises and encodes in one call.
pub fn build(spec: &ImportSpec) -> Result<Vec<u8>> {
    let data = fetch(spec)?;
    let layers = rasterise(spec, &data)?;
    assemble(spec, &layers)
}

/// Fetches, rasterises and writes the image to `path`, creating its parent
/// directory if needed.
pub fn build_to_file(spec: &ImportSpec, path: &Path) -> Result<()> {
    let bytes = build(spec)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, &bytes)?;
    tracing::info!(path = %path.display(), bytes = bytes.len(), "wrote map");
    Ok(())
}
