//! # Ourealis geographic importer
//!
//! Turns public geodata into an OMF map, so a real place can be simulated
//! without hand-authoring it:
//!
//! * **terrain** — Copernicus DEM GLO-30 tiles give the elevation raster;
//! * **structure** — OpenStreetMap ways give the road network, the building
//!   footprints that are impassable and the water bodies that are;
//! * **assembly** — [`build`] turns both into the source layers of an
//!   [`ourealis_map_format`] image, encoding the resistance feature channels the
//!   simulator's cost model consumes.
//!
//! The pipeline is split so that everything but the download is testable
//! offline: [`build::fetch`] downloads and decodes the datasets,
//! [`build::assemble`] turns already-decoded values into image bytes, and no
//! stage in between touches the network or the filesystem.
//!
//! Geometry is projected into the same flat-earth plane the simulator uses — the
//! OMF reference point is the south-west corner of the requested box and local
//! coordinates are metres from it (see [`geo`]).

#![warn(missing_docs)]

pub mod build;
pub mod cqupt;
pub mod dem;
pub mod error;
pub mod facilities;
pub mod geo;
pub mod osm;
pub mod raster;

pub use build::{ImportData, ImportSpec};
pub use error::{GeoError, Result};
pub use geo::GeoBounds;
