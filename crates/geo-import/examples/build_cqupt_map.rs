//! Builds the Chongqing University of Posts and Telecommunications map.
//!
//! Usage: `cargo run -p ourealis-geo-import --example build_cqupt_map [cell size m]`
//!
//! The first run downloads the Copernicus DEM tile covering N29/E106 and queries
//! Overpass; the tile is cached on disk afterwards, so a rebuild is offline apart
//! from the Overpass query. The image is written to `data/cqupt/cqupt.omf`, which
//! is then posted to a running service's map import endpoint as raw bytes with
//! the map name as a query parameter.

use std::path::PathBuf;

use ourealis_geo_import::{build, cqupt};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let resolution_m = match std::env::args().nth(1) {
        Some(arg) => arg.parse()?,
        None => cqupt::DEFAULT_RESOLUTION_M,
    };
    let spec = cqupt::spec_with_resolution(build::default_dem_cache(), resolution_m);
    let output = output_path();
    build::build_to_file(&spec, &output)?;
    println!("wrote {}", output.display());
    Ok(())
}

fn output_path() -> PathBuf {
    build::workspace_data_dir().join("cqupt").join("cqupt.omf")
}
