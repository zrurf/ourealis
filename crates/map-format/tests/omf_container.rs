//! Container-level integration tests: header, footer, directory, skeleton and
//! the reader/writer round trip, including malformed input handling.

use ourealis_map_format::builder::MapBuilder;
use ourealis_map_format::codec::id as codec_id;
use ourealis_map_format::synthetic::{self, SyntheticMapSpec};
use ourealis_map_format::tlv::value::{FeatureSchema, MapInfo, ZstdDict};
use ourealis_map_format::{
    Aabb, ChunkDirectory, ChunkRecord, DType, DerivedStatus, LayerDesc, LayerId, LayerKind, Map,
    MapError, MapHeaderSpec, MapWriter, QNode, QuadtreeSkeleton, footer::field as footer_field,
    header::field as header_field,
};

mod fixtures;
use fixtures::small_map;

#[test]
fn header_layout_is_exactly_128_bytes_and_crc_protected() {
    let spec = MapHeaderSpec {
        ref_lon: 2.03,
        ref_lat: 0.69,
        bounds: Aabb::new(-10.0, -20.0, 30.0, 40.0),
        base_res_cm: 50,
        chunk_size: 128,
        lod_count: 3,
        ..Default::default()
    };
    let bytes = small_map(&spec);
    let header_bytes = &bytes[..header_field::SIZE];
    assert_eq!(header_bytes.len(), 128);

    let header = ourealis_map_format::Header::from_bytes(header_bytes).expect("parse");
    assert_eq!(header.bounds, spec.bounds);
    assert_eq!(header.base_res_cm, 50);
    assert_eq!(header.chunk_size, 128);
    assert_eq!(
        header.footer_offset as usize,
        bytes.len() - footer_field::SIZE
    );

    // Flipping any covered byte must be caught by the CRC.
    for offset in [0usize, 5, 12, 48, 99] {
        let mut tampered = header_bytes.to_vec();
        tampered[offset] ^= 0xFF;
        assert!(
            ourealis_map_format::Header::from_bytes(&tampered).is_err(),
            "tampering at offset {offset} must be detected"
        );
    }
}

#[test]
fn footer_records_counts_and_whole_file_hash() {
    let spec = MapHeaderSpec::default();
    let bytes = small_map(&spec);
    let footer_bytes = &bytes[bytes.len() - footer_field::SIZE..];
    let footer = ourealis_map_format::Footer::from_bytes(footer_bytes).expect("parse");
    assert_eq!(footer.file_len as usize, bytes.len());
    assert_eq!(footer.chunk_count, footer.dir_record_count);
    assert_eq!(
        footer.dir_offset as usize + footer.dir_len as usize,
        footer.file_len as usize - 64
    );

    let map = Map::from_bytes(bytes.clone()).expect("open");
    map.verify_file_hash().expect("hash must verify");

    // A single flipped byte in the data region breaks the hash.
    let mut tampered = bytes.clone();
    let index = bytes.len() / 2;
    tampered[index] ^= 0x01;
    let reopened = Map::from_bytes(tampered);
    if let Ok(map) = reopened {
        assert!(map.verify_file_hash().is_err());
    }
}

#[test]
fn truncation_is_rejected() {
    let bytes = small_map(&MapHeaderSpec::default());
    let truncated = bytes[..bytes.len() - 32].to_vec();
    assert!(Map::from_bytes(truncated).is_err());
}

#[test]
fn wrong_magic_is_reported() {
    let mut bytes = small_map(&MapHeaderSpec::default());
    bytes[0] = b'X';
    match Map::from_bytes(bytes) {
        Err(MapError::BadMagic { .. }) | Err(MapError::HeaderCrc { .. }) => {}
        other => panic!("expected a magic or CRC failure, got {other:?}"),
    }
}

#[test]
fn directory_sorts_and_finds_records() {
    let records = vec![
        ChunkRecord::new(LayerId::ELEVATION, 1, 5, codec_id::ZSTD, 100, 10, 20, 1),
        ChunkRecord::new(LayerId::ELEVATION, 0, 3, codec_id::ZSTD, 200, 11, 21, 2),
        ChunkRecord::new(LayerId::SLOPE, 0, 1, codec_id::RAW, 300, 12, 22, 3),
    ];
    let directory = ChunkDirectory::new(records);
    assert_eq!(directory.len(), 3);
    let keys: Vec<_> = directory.records().iter().map(|r| r.sort_key()).collect();
    let mut sorted = keys.clone();
    sorted.sort_unstable();
    assert_eq!(keys, sorted);

    let found = directory.find(LayerId::ELEVATION, 0, 3).expect("present");
    assert_eq!(found.offset, 200);
    assert!(directory.find(LayerId::ELEVATION, 0, 99).is_none());
    assert_eq!(directory.for_layer_level(LayerId::ELEVATION, 0).len(), 1);
    assert_eq!(directory.layer_levels().len(), 3);
    assert_eq!(directory.total_compressed_bytes(), 33);

    // Round-trip through bytes preserves the records.
    let parsed = ChunkDirectory::from_bytes(&directory.to_bytes()).expect("parse");
    assert_eq!(parsed.len(), 3);
    assert!(parsed.find(LayerId::SLOPE, 0, 1).is_some());

    // A non-multiple length is a format error.
    assert!(ChunkDirectory::from_bytes(&[0u8; 7]).is_err());
}

#[test]
fn missing_chunk_is_not_an_error() {
    let map = Map::from_bytes(small_map(&MapHeaderSpec::default())).expect("open");
    let absent = map
        .chunk(LayerId::ELEVATION, 7, 12345)
        .expect("missing chunk must be a valid state");
    assert!(absent.is_none());
}

#[test]
fn unknown_layer_reports_a_typed_error() {
    let map = Map::from_bytes(small_map(&MapHeaderSpec::default())).expect("open");
    match map.chunk(LayerId(0x0FFF), 0, 0) {
        Err(MapError::LayerNotFound { layer_id }) => assert_eq!(layer_id, 0x0FFF),
        other => panic!("unexpected result: {other:?}"),
    }
}

#[test]
fn skeleton_keys_carry_depth_and_survive_round_trip() {
    let nodes = vec![
        QNode::new(
            ourealis_map_format::geometry::node_key(0, 0, 3),
            QNode::LEAF,
            0,
            10,
            20,
        ),
        QNode::new(
            ourealis_map_format::geometry::node_key(1, 0, 3),
            QNode::LEAF,
            0,
            11,
            21,
        ),
        QNode::new(
            ourealis_map_format::geometry::node_key(0, 0, 5),
            QNode::LEAF,
            0,
            12,
            22,
        ),
    ];
    let skeleton = QuadtreeSkeleton::build(nodes.clone()).expect("build");
    assert_eq!(skeleton.len(), 3);
    assert_eq!(skeleton.nodes()[0].key_depth(), 3);
    let deepest = skeleton.get(ourealis_map_format::geometry::node_key(0, 0, 5));
    assert!(
        deepest.is_some(),
        "the origin node at depth 5 must be found"
    );

    let bytes = skeleton.to_bytes();
    let parsed = QuadtreeSkeleton::from_bytes(&bytes).expect("parse");
    assert_eq!(parsed.nodes(), skeleton.nodes());

    // Duplicate keys are rejected: two nodes for one cell are ambiguous.
    let duplicate = vec![nodes[0], nodes[0]];
    assert!(QuadtreeSkeleton::build(duplicate).is_err());
}

#[test]
fn locate_finds_the_finest_node_covering_a_point() {
    let bounds = Aabb::new(0.0, 0.0, 1024.0, 1024.0);
    // A coarse node over the left half and a finer one over its top-left quarter.
    let coarse = QNode::new(
        ourealis_map_format::geometry::node_key(0, 0, 1),
        QNode::LEAF,
        0,
        1,
        1,
    );
    let fine = QNode::new(
        ourealis_map_format::geometry::node_key(0, 0, 2),
        QNode::LEAF,
        0,
        2,
        2,
    );
    let skeleton = QuadtreeSkeleton::build(vec![coarse, fine]).expect("build");

    // (100, 100) falls inside both nodes; the finer one wins.
    let hit = skeleton
        .locate(100.0, 100.0, &bounds, 1.0, 4)
        .expect("node");
    assert_eq!(hit.key_depth(), 2);

    // (400, 100) lies in the coarse node but outside the finer quadrant, which
    // is the upper-left quarter of the map.
    let other = skeleton
        .locate(400.0, 100.0, &bounds, 1.0, 4)
        .expect("node");
    assert_eq!(other.key_depth(), 1);

    // The right half has no node at all.
    assert!(skeleton.locate(900.0, 100.0, &bounds, 1.0, 4).is_none());
    assert!(skeleton.locate(-5.0, 100.0, &bounds, 1.0, 4).is_none());
}

#[test]
fn writer_freezes_metadata_after_the_first_chunk() {
    let spec = MapHeaderSpec {
        bounds: Aabb::new(0.0, 0.0, 32.0, 32.0),
        base_res_cm: 100,
        chunk_size: 32,
        lod_count: 1,
        ..Default::default()
    };
    let mut writer = MapWriter::new(std::io::Cursor::new(Vec::new()), spec).expect("writer");
    let desc = LayerDesc::new(
        LayerId::ELEVATION,
        LayerKind::Raster,
        1,
        DType::U8,
        codec_id::RAW,
    );
    writer.set_layer(desc).expect("layer");
    let chunk = ourealis_map_format::RasterChunk::zeros(32, 32, 1);
    writer
        .write_raster_chunk(&desc, 0, 0, None, &chunk)
        .expect("chunk");

    let err = writer
        .set_weight_prior(&ourealis_map_format::tlv::value::WeightPrior::default())
        .expect_err("metadata must be frozen");
    assert!(format!("{err}").contains("after the first chunk"));
}

#[test]
fn feature_and_weight_metadata_round_trip() {
    let spec = SyntheticMapSpec::compact();
    let bytes = synthetic::build(&spec).expect("build");
    let map = Map::from_bytes(bytes).expect("open");

    let schema = map.feature_schema().unwrap().expect("schema");
    assert_eq!(schema.dim(), 5);
    assert_eq!(schema.dims[0].name, "surface_type");

    let prior = map.weight_prior().unwrap().expect("prior");
    assert!(prior.get(ourealis_map_format::MotionMode::Jog).is_some());

    let stats = map.global_stats().unwrap().expect("stats");
    assert!(!stats.channels.is_empty());
    let elevation_stats = stats
        .channels
        .iter()
        .find(|c| c.layer_id == LayerId::ELEVATION)
        .expect("elevation statistics");
    assert!(elevation_stats.max > elevation_stats.min);
    assert!(stats.connector_unit_cost_min.is_some());

    let info = map.map_info().unwrap().expect("map info");
    assert_eq!(info.name, "synthetic-campus");

    assert!(map.chunk_layout().unwrap().is_some());
    assert!(map.slope_model().is_ok());
    assert!(map.aggregation_rules().is_ok());
}

#[test]
fn lod_levels_are_stored_and_readable() {
    let spec = SyntheticMapSpec::compact();
    let bytes = synthetic::build(&spec).expect("build");
    let map = Map::from_bytes(bytes).expect("open");

    let view = map.layer(LayerId::ELEVATION).expect("layer view");
    assert!(view.levels().contains(&0));
    assert!(view.levels().contains(&1), "LOD level 1 expected");

    // Every level stores a full `chunk_size` payload. A level-L cell spans
    // `2^L` base pixels, so the cell count per chunk is constant while the
    // chunk's footprint grows; halving the payload instead would leave three
    // quarters of each coarse chunk's advertised area without data.
    let level1 = view.chunk_shape(1);
    assert_eq!(level1.width, spec.chunk_size as u32);
    assert_eq!(level1.height, spec.chunk_size as u32);

    let grid = map.grid();
    let read_cell = |level: u8, cell_x: u32, cell_y: u32| -> f32 {
        let (px, py) = grid.cell_center(cell_x, cell_y, level);
        let id = grid.chunk_id_at(px, py, level).expect("chunk id");
        let (ox, oy) = grid.chunk_origin_cell(id);
        let chunk = map
            .chunk(LayerId::ELEVATION, level, id)
            .expect("read")
            .expect("chunk present");
        chunk.get(cell_x - ox, cell_y - oy, 0)
    };

    // Each coarse cell must be the box average of the four finer cells under
    // it, over the whole covered area rather than only the first quadrant.
    let (cells_x, cells_y) = grid.cell_dims(0);
    let mut checked = 0usize;
    for gy in (0..cells_y.saturating_sub(1)).step_by(2) {
        for gx in (0..cells_x.saturating_sub(1)).step_by(2) {
            if gx % 16 != 0 || gy % 16 != 0 {
                continue;
            }
            let expected = (read_cell(0, gx, gy)
                + read_cell(0, gx + 1, gy)
                + read_cell(0, gx, gy + 1)
                + read_cell(0, gx + 1, gy + 1))
                * 0.25;
            let actual = read_cell(1, gx / 2, gy / 2);
            // Both levels are quantised to the elevation layer's 0.1 m step, so
            // the comparison allows one step at each end.
            assert!(
                (actual - expected).abs() <= 0.2,
                "coarse cell ({}, {}) is {actual}, box average of its four children is {expected}",
                gx / 2,
                gy / 2
            );
            checked += 1;
        }
    }
    assert!(checked >= 8, "only {checked} coarse cells were checked");
}

#[test]
fn a_builder_declares_every_lod_level_it_wrote() {
    // The header's `lod_count` is what makes a level legal, so a builder that
    // generates a pyramid must not leave the caller's smaller count in place: the
    // reader rejects a directory record whose level the header never promised.
    let spec = MapHeaderSpec {
        lod_count: 1,
        ..raster_spec(32, 64.0)
    };
    let cells = 64u32;
    let elevation: Vec<f32> = (0..cells * cells)
        .map(|index| {
            let x = (index % cells) as f32;
            let y = (index / cells) as f32;
            10.0 + 0.05 * x + 0.02 * y
        })
        .collect();
    let mut builder = MapBuilder::new(spec, FeatureSchema::default()).with_lod_levels(2);
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
            elevation,
        )
        .expect("elevation layer");

    let map = Map::from_bytes(builder.build_to_bytes().expect("build")).expect("open");
    assert_eq!(
        map.header().lod_count,
        3,
        "levels 0..=2 were written, so the header must declare three"
    );
    let view = map.layer(LayerId::ELEVATION).expect("layer view");
    assert!(view.levels().contains(&2), "LOD level 2 expected");
    assert!(
        map.chunk(LayerId::ELEVATION, 2, 0)
            .expect("read level 2")
            .is_some(),
        "the declared level must be readable"
    );
}

#[test]
fn synthetic_feature_channels_survive_the_round_trip() {
    // These channels are continuous (traffic, crowding, lighting) or need the whole
    // 16-bit range (the packed direction). A layer descriptor whose quantisation
    // cannot carry them destroys them silently at write time, which no test that
    // only inspects the source raster would notice.
    let spec = SyntheticMapSpec::compact();
    let (bytes, source) = synthetic::build_with_layers(&spec).expect("build");
    let map = Map::from_bytes(bytes).expect("open");
    let grid = map.grid();
    let read = |layer: LayerId, cell_x: u32, cell_y: u32| -> f32 {
        let (px, py) = grid.cell_center(cell_x, cell_y, 0);
        let id = grid.chunk_id_at(px, py, 0).expect("chunk id");
        let (ox, oy) = grid.chunk_origin_cell(id);
        map.chunk(layer, 0, id)
            .expect("read")
            .expect("chunk present")
            .get(cell_x - ox, cell_y - oy, 0)
    };

    let scalars = [
        (
            "traffic",
            LayerId::feature(synthetic::feature::TRAFFIC),
            source.traffic.as_slice(),
        ),
        (
            "crowding",
            LayerId::feature(synthetic::feature::CROWDING),
            source.crowding.as_slice(),
        ),
        (
            "lighting",
            LayerId::feature(synthetic::feature::LIGHTING),
            source.lighting.as_slice(),
        ),
    ];
    for (name, layer, values) in scalars {
        let mut fractional = 0usize;
        for y in (0..source.dims.1).step_by(3) {
            for x in (0..source.dims.0).step_by(3) {
                let expected = values[(y * source.dims.0 + x) as usize];
                let actual = read(layer, x, y);
                // One 1/255 quantisation step at each end.
                assert!(
                    (actual - expected).abs() <= 2.0 / 255.0 + 1e-6,
                    "{name} at ({x}, {y}) is {actual}, expected {expected}"
                );
                if (expected - expected.round()).abs() > 0.05 {
                    fractional += 1;
                }
            }
        }
        assert!(
            fractional > 0,
            "{name} kept no fractional value: the channel collapsed to whole numbers"
        );
    }

    // `angle_index * 256 + strength` overflows a byte, so a layer that cannot hold
    // more than 255 decodes every constrained cell as angle 0.
    let mut beyond_a_byte = 0usize;
    for y in (0..source.dims.1).step_by(3) {
        for x in (0..source.dims.0).step_by(3) {
            let expected = source.direction[(y * source.dims.0 + x) as usize];
            let actual = read(LayerId::DIRECTION, x, y);
            assert!(
                (actual - expected).abs() < 1e-3,
                "direction at ({x}, {y}) is {actual}, expected {expected}"
            );
            if expected > 255.0 {
                beyond_a_byte += 1;
            }
        }
    }
    assert!(
        beyond_a_byte > 0,
        "the packing must exercise values above one byte"
    );
}

#[test]
fn skeleton_leaves_describe_uniform_ground() {
    // A leaf *without* the drill-down hint is the builder's statement "this block
    // is uniform and may be used at coarse granularity", and the partition proxy
    // is the hard-constraint layer. Decoding such a leaf's key back into metres
    // must therefore land on a block with no forbidden cell in it; if the reader's
    // key-to-metres mapping disagreed with the builder's partition, a leaf would
    // cover a building and a coarse search granularity would walk straight through
    // it. Leaves *with* the hint describe a block that has to be refined, so they
    // are expected to contain obstacles.
    //
    // The proxy is boolean, so the builder's own uniformity test (`max - mean`)
    // admits blocks that are almost entirely forbidden; those always carry the
    // hint, which is what makes the check below the right one rather than a
    // statement about every leaf.
    let spec = SyntheticMapSpec::compact();
    let bytes = synthetic::build(&spec).expect("build");
    let map = Map::from_bytes(bytes).expect("open");

    let base_res = map.header().base_res_m();
    let bounds = map.header().bounds;
    let grid = map.grid();
    let sample = |x: f64, y: f64| -> Option<f32> {
        if !bounds.contains(x, y) {
            return None;
        }
        let cell_x = ((x - bounds.min_x) / base_res).floor() as u32;
        let cell_y = ((y - bounds.min_y) / base_res).floor() as u32;
        let chunk_id = grid.chunk_id_at(x, y, 0)?;
        let chunk = map
            .chunk(LayerId::HARD_FORBIDDEN, 0, chunk_id)
            .expect("read")?;
        let (origin_x, origin_y) = grid.chunk_origin_cell(chunk_id);
        Some(chunk.get(cell_x - origin_x, cell_y - origin_y, 0))
    };

    let mut checked = 0usize;
    let mut drilled = 0usize;
    for node in map.skeleton() {
        if node.needs_drill_down() {
            drilled += 1;
            continue;
        }
        let area = node.bounds(&bounds, base_res);
        // Sample the centre of every base cell the block covers — the same set the
        // builder aggregated over. Sampling the box's corners instead would also
        // touch the neighbouring blocks' cells.
        let first_x = ((area.min_x - bounds.min_x) / base_res).round() as i64;
        let first_y = ((area.min_y - bounds.min_y) / base_res).round() as i64;
        let count = (area.width() / base_res).round().max(1.0) as i64;
        for iy in 0..count {
            for ix in 0..count {
                let x = bounds.min_x + (first_x + ix) as f64 * base_res + base_res * 0.5;
                let y = bounds.min_y + (first_y + iy) as f64 * base_res + base_res * 0.5;
                if let Some(value) = sample(x, y) {
                    assert!(
                        value <= 0.5,
                        "leaf at depth {} covers ({x:.2}, {y:.2}), which is forbidden",
                        node.depth
                    );
                }
            }
        }
        checked += 1;
    }
    assert!(checked > 50, "only {checked} leaves were checked");
    assert!(
        drilled > 0,
        "the map should also carry blocks needing refinement"
    );
}

fn raster_spec(chunk_size: u16, extent_m: f64) -> MapHeaderSpec {
    MapHeaderSpec {
        bounds: Aabb::new(0.0, 0.0, extent_m, extent_m),
        base_res_cm: 100,
        chunk_size,
        lod_count: 1,
        ..Default::default()
    }
}

#[test]
fn bitmap_chunks_survive_a_chunk_size_that_is_not_a_byte_multiple() {
    let cells = 33u32;
    let spec = raster_spec(cells as u16, cells as f64);
    let mut builder = MapBuilder::new(spec, FeatureSchema::default()).with_lod_levels(0);
    builder
        .add_bitmap_layer(
            LayerId::HARD_FORBIDDEN,
            cells,
            cells,
            (0..cells * cells)
                .map(|i| if i % 7 == 0 { 1.0 } else { 0.0 })
                .collect(),
        )
        .expect("bitmap layer");
    let map = Map::from_bytes(builder.build_to_bytes().expect("build")).expect("open");

    let chunk = map
        .chunk(LayerId::HARD_FORBIDDEN, 0, 0)
        .expect("read")
        .expect("chunk");
    assert_eq!((chunk.width, chunk.height), (cells, cells));
    for (index, value) in chunk.data.iter().enumerate() {
        let expected = if index % 7 == 0 { 1.0 } else { 0.0 };
        assert_eq!(*value, expected, "cell {index}");
    }
}

#[test]
fn maps_compressed_with_a_zstd_dictionary_are_readable() {
    let spec = raster_spec(32, 32.0);
    let samples: Vec<f32> = (0..32 * 32).map(|index| (index % 251) as f32).collect();
    let build = |dict: Option<Vec<u8>>| -> Vec<u8> {
        let mut writer = MapWriter::new(std::io::Cursor::new(Vec::new()), spec).expect("writer");
        if let Some(dict) = dict {
            writer.set_zstd_dict(&ZstdDict(dict)).expect("dictionary");
        }
        writer
            .set_map_info(&MapInfo {
                name: "dictionary".into(),
                ..Default::default()
            })
            .expect("map info");
        let desc = LayerDesc::new(
            LayerId::ELEVATION,
            LayerKind::Raster,
            1,
            DType::U8,
            codec_id::ZSTD,
        );
        writer.set_layer(desc).expect("layer");
        let mut chunk = ourealis_map_format::RasterChunk::zeros(32, 32, 1);
        chunk.data.copy_from_slice(&samples);
        writer
            .write_raster_chunk(&desc, 0, 0, None, &chunk)
            .expect("chunk");
        writer.finish().expect("finish").into_inner()
    };

    // The dictionary is the meta block of an equivalent map, so the second
    // build's meta really does reference it. The dictionary lives inside the
    // meta block, so the reader must decompress that block without it; only
    // chunk payloads may use it.
    let plain = Map::from_bytes(build(None)).expect("open plain");
    let dictionary = plain.meta_block().to_bytes();
    assert!(!dictionary.is_empty());

    let map = Map::from_bytes(build(Some(dictionary))).expect("open with dictionary");
    assert_eq!(map.map_info().unwrap().unwrap().name, "dictionary");
    let read = map
        .chunk(LayerId::ELEVATION, 0, 0)
        .expect("read")
        .expect("chunk");
    assert_eq!(read.data, samples);
}

#[test]
fn aggregation_rules_name_the_selected_partition_proxy() {
    let spec = raster_spec(32, 64.0);
    let mut builder = MapBuilder::new(spec, FeatureSchema::default())
        .with_lod_levels(0)
        .with_partition_proxy(LayerId::ELEVATION, 0);
    builder
        .add_layer(
            LayerDesc::new(
                LayerId::ELEVATION,
                LayerKind::Raster,
                1,
                DType::U8,
                codec_id::RAW,
            ),
            64,
            64,
            vec![3.0; 64 * 64],
        )
        .expect("layer");
    let map = Map::from_bytes(builder.build_to_bytes().expect("build")).expect("open");
    let rules = map.aggregation_rules().expect("rules");
    assert_eq!(rules.proxy_layer, LayerId::ELEVATION);
    assert_eq!(rules.proxy_channel, 0);
}

#[test]
fn fingerprint_dedups_a_chunk_key_written_twice() {
    // Writing the same `(layer, level, chunk_id)` twice leaves only the last
    // record in the directory, so the fingerprint baked into the derived layer
    // must hash that last record set or it can never match on load.
    let spec = raster_spec(32, 64.0);
    let elevation = LayerDesc::new(
        LayerId::ELEVATION,
        LayerKind::Raster,
        1,
        DType::I16,
        codec_id::RAW,
    )
    .with_quantisation(0.1, 0.0);
    let slope = LayerDesc::new(
        LayerId::SLOPE,
        LayerKind::Raster,
        1,
        DType::I16,
        codec_id::RAW,
    )
    .with_quantisation(0.1, 0.0);
    let mut builder = MapBuilder::new(spec, FeatureSchema::default()).with_lod_levels(0);
    builder
        .add_layer(
            elevation,
            64,
            64,
            (0..64 * 64).map(|i| 10.0 + (i % 5) as f32).collect(),
        )
        .expect("first elevation write");
    builder
        .add_layer(
            elevation,
            64,
            64,
            (0..64 * 64).map(|i| 20.0 + (i % 3) as f32).collect(),
        )
        .expect("second elevation write");
    builder
        .add_layer(slope, 64, 64, vec![1.0; 64 * 64])
        .expect("slope layer");

    let map = Map::from_bytes(builder.build_to_bytes().expect("build")).expect("open");
    assert_eq!(
        map.verify_derived(LayerId::SLOPE).expect("verify"),
        DerivedStatus::Valid
    );
    assert!(map.chunk(LayerId::SLOPE, 0, 0).expect("read").is_some());
}

#[test]
fn hostile_header_bounds_are_rejected_instead_of_overrunning_the_grid() {
    let bytes = small_map(&MapHeaderSpec::default());
    let mut image = bytes.clone();
    let mut header =
        ourealis_map_format::Header::from_bytes(&image[..header_field::SIZE]).expect("header");
    header.bounds = Aabb::new(0.0, 0.0, 1.0e9, 1.0e9);
    image[..header_field::SIZE].copy_from_slice(&header.to_bytes());
    let source = std::sync::Arc::new(ourealis_map_format::MemSource::new(image));
    // The whole-file hash is skipped so the test reaches the grid validation.
    match Map::with_source(source, false) {
        Err(MapError::Invalid(_)) => {}
        other => panic!("expected an invalid-grid error, got {other:?}"),
    }
}

#[test]
fn writer_rejects_a_grid_with_more_chunks_than_the_id_can_address() {
    let spec = raster_spec(256, 1.0e9);
    match MapWriter::new(std::io::Cursor::new(Vec::new()), spec) {
        Err(MapError::Invalid(_)) => {}
        Err(other) => panic!("unexpected error: {other:?}"),
        Ok(_) => panic!("expected an invalid-grid error"),
    }
}

#[test]
fn builder_rejects_a_layer_spanning_too_many_chunks() {
    let spec = raster_spec(1, 65538.0);
    let mut builder = MapBuilder::new(spec, FeatureSchema::default()).with_lod_levels(0);
    let desc = LayerDesc::new(
        LayerId::ELEVATION,
        LayerKind::Raster,
        1,
        DType::U8,
        codec_id::RAW,
    );
    match builder.add_layer(desc, 65538, 1, vec![0.0; 65538]) {
        Err(MapError::Invalid(_)) => {}
        Ok(_) => panic!("a 65538-chunk axis must be rejected"),
        Err(other) => panic!("unexpected error: {other:?}"),
    }
}

#[test]
fn skeleton_side_saturates_on_hostile_bounds_and_resolution() {
    let bounds = Aabb::new(0.0, 0.0, 1.0e9, 1.0e9);
    let side = ourealis_map_format::quadtree::skeleton_side_m(&bounds, 0.01);
    assert!(side.is_finite() && side > 0.0);

    // Node areas and locate queries derived from the same hostile header must
    // stay finite and answer normally rather than panicking on overflow.
    let node = QNode::new(
        ourealis_map_format::geometry::node_key(0, 0, 0),
        QNode::LEAF,
        0,
        0,
        0,
    );
    let area = node.bounds(&bounds, 0.01);
    assert!(area.min_x.is_finite() && area.max_x.is_finite());
    let skeleton = QuadtreeSkeleton::build(vec![node]).expect("build");
    assert!(skeleton.locate(1.0, 1.0, &bounds, 0.01, 31).is_some());
}
