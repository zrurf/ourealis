//! Builds a synthetic campus map and reports what went into it.
//!
//! Run with `cargo run -p ourealis-map-format --example build_synthetic_map`.

use ourealis_map_format::synthetic::{self, SyntheticMapSpec};
use ourealis_map_format::{LayerId, Map};

fn main() -> ourealis_map_format::Result<()> {
    let spec = SyntheticMapSpec::default();
    let image = synthetic::build(&spec)?;
    let map = Map::from_bytes(image)?;
    let stats = map.stats();

    println!(
        "map: {} layer(s), {} chunk(s)",
        stats.layer_count, stats.chunk_count
    );
    println!(
        "extent: {:.0} m x {:.0} m at {:.2} m/cell",
        map.header().bounds.width(),
        map.header().bounds.height(),
        map.grid().base_res_m
    );
    println!(
        "stored {:.1} KiB, expanded {:.1} KiB, {} skeleton node(s)",
        stats.stored_bytes as f64 / 1024.0,
        stats.raw_bytes as f64 / 1024.0,
        stats.skeleton_nodes
    );
    for layer_id in map.layer_ids() {
        let levels = map
            .layer(layer_id)
            .map(|view| view.levels().len())
            .unwrap_or(0);
        println!("  layer {layer_id}: {levels} level(s)");
    }
    if let Some(info) = map.map_info()? {
        println!("name: {}", info.name);
    }
    if let Some(connectors) = map.connectors()? {
        println!("connectors: {}", connectors.connectors.len());
    }
    if let Some(regions) = map.regions()? {
        println!("regions: {}", regions.features().len());
    }
    let elevation = map
        .layer(LayerId::ELEVATION)
        .expect("the synthetic map always carries elevation");
    println!("elevation channels: {}", elevation.channels());

    Ok(())
}
