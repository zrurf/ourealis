//! Shared fixtures for the integration tests.

use ourealis_map_format::builder::MapBuilder;
use ourealis_map_format::codec::id as codec_id;
use ourealis_map_format::layer::{DType, LayerDesc, LayerId, LayerKind};
use ourealis_map_format::tlv::value::FeatureSchema;
use ourealis_map_format::writer::MapHeaderSpec;

/// Builds a small two-layer map with one chunk per layer.
pub fn small_map(spec: &MapHeaderSpec) -> Vec<u8> {
    let cells = spec.chunk_size as u32;
    let mut builder = MapBuilder::new(*spec, FeatureSchema::default()).with_lod_levels(0);
    builder
        .add_layer(
            LayerDesc::new(
                LayerId::ELEVATION,
                LayerKind::Raster,
                1,
                DType::I16,
                codec_id::DELTA_VERTICAL,
            )
            .with_quantisation(0.1, 0.0),
            cells,
            cells,
            (0..cells * cells)
                .map(|i| 20.0 + (i % 17) as f32 * 0.5)
                .collect(),
        )
        .expect("elevation layer");
    builder
        .add_bitmap_layer(
            LayerId::HARD_FORBIDDEN,
            cells,
            cells,
            (0..cells * cells)
                .map(|i| if i % 97 == 0 { 1.0 } else { 0.0 })
                .collect(),
        )
        .expect("bitmap layer");
    builder.build_to_bytes().expect("build")
}
