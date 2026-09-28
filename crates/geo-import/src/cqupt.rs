//! Chongqing University of Posts and Telecommunications.
//!
//! There is no published CQUPT elevation dataset. What exists for the campus is
//! the same thing that exists everywhere else: the global Copernicus DEM GLO-30
//! mosaic, which covers the N29/E106 cell at one arc-second, plus OpenStreetMap
//! geometry of the campus itself. This module fixes the box those two sources are
//! sampled over and turns it into an [`ImportSpec`].
//!
//! The campus sits on Nanshan at roughly 500 m, on ground that falls away steeply
//! to the Yangtze, so it exercises the elevation channel with real relief rather
//! than the near-flat terrain a city-centre map would have.

use std::path::PathBuf;

use crate::build::ImportSpec;
use crate::facilities::Overlay;
use crate::geo::GeoBounds;

/// Southern edge of the import box, degrees.
pub const SOUTH: f64 = 29.5212;
/// Western edge of the import box, degrees.
pub const WEST: f64 = 106.5926;
/// Northern edge of the import box, degrees.
pub const NORTH: f64 = 29.5429;
/// Eastern edge of the import box, degrees.
pub const EAST: f64 = 106.6194;

/// The box covers about 2.6 km east to west and 2.4 km north to south, centred
/// on the campus at `29.5320 N, 106.6060 E`.
pub fn bounds() -> GeoBounds {
    GeoBounds::new(SOUTH, WEST, NORTH, EAST).expect("the CQUPT box is a valid geographic box")
}

/// Import spec for the campus, at the campus resolution.
pub fn spec(dem_cache: impl Into<PathBuf>) -> ImportSpec {
    ImportSpec::new("cqupt", bounds())
        .with_dem_cache(dem_cache)
        .with_resolution_m(DEFAULT_RESOLUTION_M)
        .with_overlay(Overlay::cqupt())
}

/// Import spec for the campus at an explicit cell size.
pub fn spec_with_resolution(dem_cache: impl Into<PathBuf>, resolution_m: f64) -> ImportSpec {
    spec(dem_cache).with_resolution_m(resolution_m)
}

/// Cell size the campus is imported at by default, metres.
///
/// Finer than the importer's own default because a campus is read at walking
/// scale: a footpath is about a metre wide and a building wall about half of one,
/// so at two metres the paths and the walls are the same cell and neither has an
/// edge a reader can follow. The map stores three levels of detail, and one metre
/// at level 0 puts two metres at level 1 — the level a whole-campus view draws —
/// so the finer raster costs nothing while the map is zoomed out and shows real
/// detail as soon as it is zoomed in.
pub const DEFAULT_RESOLUTION_M: f64 = 1.0;
