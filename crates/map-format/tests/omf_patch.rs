//! Patch application tests.

use ourealis_map_format::builder::MapBuilder;
use ourealis_map_format::codec::id as codec_id;
use ourealis_map_format::layer::{DType, LayerDesc, LayerId, LayerKind};
use ourealis_map_format::patch::{ChunkReplacement, Patch, file_hash64};
use ourealis_map_format::tlv::value::{FeatureSchema, MapInfo};
use ourealis_map_format::writer::MapHeaderSpec;
use ourealis_map_format::{Aabb, Map, MapError};

fn spec() -> MapHeaderSpec {
    MapHeaderSpec {
        bounds: Aabb::new(0.0, 0.0, 64.0, 64.0),
        base_res_cm: 100,
        chunk_size: 32,
        lod_count: 1,
        ..Default::default()
    }
}

fn elevation_desc() -> LayerDesc {
    LayerDesc::new(
        LayerId::ELEVATION,
        LayerKind::Raster,
        1,
        DType::I16,
        codec_id::RAW,
    )
    .with_quantisation(0.1, 0.0)
}

fn base_map() -> Vec<u8> {
    let mut builder = MapBuilder::new(spec(), FeatureSchema::default())
        .with_lod_levels(0)
        .with_map_info(MapInfo {
            name: "base".into(),
            ..Default::default()
        });
    builder
        .add_layer(
            elevation_desc(),
            64,
            64,
            (0..64 * 64).map(|i| 10.0 + (i % 5) as f32).collect(),
        )
        .expect("layer");
    builder.build_to_bytes().expect("build")
}

/// Encodes a replacement elevation payload with the RAW codec.
fn replacement_payload(value: f32) -> (Vec<u8>, u32) {
    let chunk = ourealis_map_format::RasterChunk::zeros(32, 32, 1);
    let mut chunk = chunk;
    for y in 0..32 {
        for x in 0..32 {
            chunk.set(x, y, 0, value);
        }
    }
    let payload = ourealis_map_format::raster::pack(&elevation_desc(), &chunk).expect("pack");
    let len = payload.len() as u32;
    (payload, len)
}

#[test]
fn patch_replaces_a_chunk_and_keeps_the_rest_intact() {
    let base = base_map();
    let map = Map::from_bytes(base.clone()).expect("open");

    // The untouched half of the map must keep its values.
    let untouched_id = map.grid().chunk_id_at(4.0, 4.0, 0).expect("chunk");
    let untouched_before = map
        .chunk(LayerId::ELEVATION, 0, untouched_id)
        .unwrap()
        .unwrap();

    let (payload, raw_len) = replacement_payload(42.5);
    let target = map.grid().chunk_id_at(60.0, 60.0, 0).expect("chunk");
    let patch = Patch::for_map(
        &map,
        vec![ChunkReplacement::new(
            LayerId::ELEVATION,
            0,
            target,
            codec_id::RAW,
            payload,
            raw_len,
        )],
    )
    .expect("patch");

    let patched_bytes = patch.apply(&base).expect("apply");
    let patched = Map::from_bytes(patched_bytes).expect("open patched");

    let changed = patched
        .chunk(LayerId::ELEVATION, 0, target)
        .unwrap()
        .unwrap();
    assert!((changed.get(0, 0, 0) - 42.5).abs() < 0.05);

    let untouched_after = patched
        .chunk(LayerId::ELEVATION, 0, untouched_id)
        .unwrap()
        .unwrap();
    assert_eq!(untouched_before.data, untouched_after.data);
    patched
        .verify_file_hash()
        .expect("hash must verify after patching");
    assert!(patched.stats().chunk_count >= map.stats().chunk_count);
}

#[test]
fn patch_round_trips_through_bytes() {
    let map = Map::from_bytes(base_map()).expect("open");
    let (payload, raw_len) = replacement_payload(11.0);
    let patch = Patch::for_map(
        &map,
        vec![ChunkReplacement::new(
            LayerId::ELEVATION,
            0,
            0,
            codec_id::RAW,
            payload,
            raw_len,
        )],
    )
    .expect("patch")
    .with_meta(
        ourealis_map_format::tlv::tag::MAP_INFO,
        &MapInfo {
            name: "patched".into(),
            ..Default::default()
        },
    );

    let encoded = patch.encode();
    let decoded = Patch::decode(&encoded).expect("decode");
    assert_eq!(decoded.base_hash, patch.base_hash);
    assert_eq!(decoded.replacements.len(), 1);
    assert_eq!(decoded.replacements[0].raw_len, raw_len);
    assert_eq!(decoded.meta_patch.len(), 1);
}

#[test]
fn patch_rejects_a_foreign_base_file() {
    let map = Map::from_bytes(base_map()).expect("open");
    let (payload, raw_len) = replacement_payload(1.0);
    let mut patch = Patch::for_map(
        &map,
        vec![ChunkReplacement::new(
            LayerId::ELEVATION,
            0,
            0,
            codec_id::RAW,
            payload,
            raw_len,
        )],
    )
    .expect("patch");

    // A different base file must be refused rather than silently mis-patched.
    let other = {
        let mut builder = MapBuilder::new(spec(), FeatureSchema::default()).with_lod_levels(0);
        builder
            .add_layer(
                elevation_desc(),
                64,
                64,
                (0..64 * 64).map(|i| 1.0 + i as f32 * 0.01).collect(),
            )
            .expect("layer");
        builder.build_to_bytes().expect("build")
    };
    patch.base_hash = file_hash64(&other);
    match patch.apply(&base_map()) {
        Err(MapError::PatchBaseMismatch { .. }) => {}
        other => panic!("expected a base mismatch, got {other:?}"),
    }
}

#[test]
fn patch_refuses_to_touch_derived_layers() {
    let map = Map::from_bytes(base_map()).expect("open");
    let (payload, raw_len) = replacement_payload(1.0);
    let replacement = ChunkReplacement::new(LayerId::SLOPE, 0, 0, codec_id::RAW, payload, raw_len);
    match Patch::for_map(&map, vec![replacement]) {
        Err(MapError::PatchDerivedLayer { layer_id }) => {
            assert_eq!(layer_id, LayerId::SLOPE.raw());
        }
        other => panic!("derived layers must not be patchable: {other:?}"),
    }
}

#[test]
fn patch_decode_rejects_an_impossible_replacement_count() {
    // A header claiming four billion replacements in a handful of bytes must be
    // rejected before the decoder reserves memory for them.
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&ourealis_map_format::patch::MAGIC);
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&0u16.to_le_bytes());
    bytes.extend_from_slice(&0u64.to_le_bytes());
    bytes.extend_from_slice(&u32::MAX.to_le_bytes());
    bytes.extend_from_slice(&0u32.to_le_bytes());
    match Patch::decode(&bytes) {
        Err(MapError::Truncated { .. }) => {}
        other => panic!("expected a truncation error, got {other:?}"),
    }
}

#[test]
fn patched_metadata_is_visible_after_reload() {
    let base = base_map();
    let map = Map::from_bytes(base.clone()).expect("open");
    let info = MapInfo {
        name: "patched".into(),
        author: "edit".into(),
        ..Default::default()
    };
    let patch = Patch::for_map(&map, Vec::new())
        .expect("patch")
        .with_meta(ourealis_map_format::tlv::tag::MAP_INFO, &info);
    let patched = Map::from_bytes(patch.apply(&base).expect("apply")).expect("open");
    let loaded = patched.map_info().unwrap().expect("info");
    assert_eq!(loaded.name, "patched");
    assert_eq!(loaded.author, "edit");
}
