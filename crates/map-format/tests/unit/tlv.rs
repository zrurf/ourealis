//! Unit tests for the TLV block (included from `src/tlv/mod.rs`).

use super::*;
use crate::tlv::value::*;

fn record(tag: u32, value: &[u8]) -> TlvRecord {
    TlvRecord::new(tag, value.to_vec())
}

#[test]
fn block_roundtrip_preserves_order_and_values() {
    let mut block = TlvBlock::new();
    block.push(record(tag::MAP_INFO, &[1, 2, 3]));
    block.push(record(tag::GLOBAL_STATS, &[]));
    block.push(record(0x8000_1234, &[9; 5]));

    let bytes = block.to_bytes();
    let parsed = TlvBlock::from_bytes(&bytes).unwrap();
    assert_eq!(parsed.len(), 3);
    assert_eq!(parsed.records()[0].tag, tag::MAP_INFO);
    assert_eq!(parsed.records()[0].value, vec![1, 2, 3]);
    assert_eq!(parsed.records()[1].value, Vec::<u8>::new());
    assert_eq!(parsed.records()[2].tag, 0x8000_1234);
}

#[test]
fn unknown_tags_are_preserved_verbatim() {
    let mut block = TlvBlock::new();
    block.push(record(0x7FDE_ADBE, &[7, 7, 7]));
    let parsed = TlvBlock::from_bytes(&block.to_bytes()).unwrap();
    assert_eq!(parsed.records()[0].value, vec![7, 7, 7]);
    assert_eq!(namespace_of(parsed.records()[0].tag), 0x7F);
}

#[test]
fn get_set_and_remove_by_tag() {
    let mut block = TlvBlock::new();
    block.push(record(tag::MAP_INFO, &[1]));
    block.set(record(tag::MAP_INFO, &[2]));
    assert_eq!(block.len(), 1);
    assert_eq!(block.get(tag::MAP_INFO).unwrap().value, vec![2]);

    block.push(record(tag::SLOPE_MODEL, &[3]));
    assert_eq!(block.remove(tag::MAP_INFO), 1);
    assert!(block.get(tag::MAP_INFO).is_none());
    assert_eq!(block.len(), 1);
    assert_eq!(block.remove(tag::MAP_INFO), 0);
}

#[test]
fn required_tag_reports_missing() {
    let block = TlvBlock::new();
    let err = block
        .require::<MapInfo>(tag::MAP_INFO, "MAP_INFO")
        .unwrap_err();
    match err {
        MapError::MissingTlv { tag: t, name } => {
            assert_eq!(t, tag::MAP_INFO);
            assert_eq!(name, "MAP_INFO");
        }
        other => panic!("unexpected error: {other:?}"),
    }
}

#[test]
fn namespace_extraction() {
    assert_eq!(namespace_of(0x0000_0001), NS_CORE);
    assert_eq!(namespace_of(0x0100_0001), NS_EXPERIMENTAL);
    assert_eq!(namespace_of(0x1000_0001), NS_COMMUNITY_START);
    assert_eq!(namespace_of(0x8000_0001), NS_VENDOR_START);
    assert_eq!(namespace_of(0xFF00_0001), NS_DEBUG);
}

#[test]
fn truncated_tlv_header_is_rejected() {
    let bytes = [1u8, 2, 3];
    assert!(TlvBlock::from_bytes(&bytes).is_err());
}

#[test]
fn declared_length_beyond_buffer_is_rejected() {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&tag::MAP_INFO.to_le_bytes());
    bytes.extend_from_slice(&64u32.to_le_bytes());
    bytes.extend_from_slice(&[0u8; 4]);
    assert!(TlvBlock::from_bytes(&bytes).is_err());
}

#[test]
fn map_info_roundtrip() {
    let info = MapInfo {
        name: "campus".into(),
        author: "ourealis".into(),
        built_unix: 1_700_000_000,
        upstream_hash: vec![0xAB; 32],
        description: "synthetic test map".into(),
    };
    let decoded = MapInfo::decode(&info.encode()).unwrap();
    assert_eq!(decoded, info);
}

#[test]
fn layer_table_roundtrip() {
    use crate::layer::{DType, LayerDesc, LayerId, LayerKind};
    let mut table = LayerTable::default();
    table.upsert(
        LayerDesc::new(
            LayerId::ELEVATION,
            LayerKind::Raster,
            1,
            DType::I16,
            crate::codec::id::DELTA_VERTICAL,
        )
        .with_quantisation(0.1, 0.0),
    );
    table.upsert(LayerDesc::new(
        LayerId::HARD_FORBIDDEN,
        LayerKind::Bitmap,
        1,
        DType::Bit,
        crate::codec::id::RLE,
    ));
    let decoded = LayerTable::decode(&table.encode()).unwrap();
    assert_eq!(decoded.layers.len(), 2);
    assert_eq!(decoded.get(LayerId::ELEVATION).unwrap().scale, 0.1);
    assert_eq!(
        decoded.get(LayerId::HARD_FORBIDDEN).unwrap().dtype,
        DType::Bit
    );
}

#[test]
fn feature_schema_roundtrip() {
    use crate::layer::LayerId;
    let schema = FeatureSchema {
        dims: vec![
            FeatureDim {
                name: "surface".into(),
                unit: String::new(),
                kind: FeatureKind::Category,
                layer_id: LayerId::feature(0),
                channel: 0,
                scale: 1.0,
                bias: 0.0,
                norm_min: 0.0,
                norm_max: 1.0,
                palette: vec![1.0, 0.6, 0.2],
            },
            FeatureDim {
                name: "crowding".into(),
                unit: "person/m2".into(),
                kind: FeatureKind::Scalar,
                layer_id: LayerId::feature(1),
                channel: 0,
                scale: 0.01,
                bias: 0.0,
                norm_min: 0.0,
                norm_max: 2.0,
                palette: Vec::new(),
            },
        ],
    };
    let decoded = FeatureSchema::decode(&schema.encode()).unwrap();
    assert_eq!(decoded.dim(), 2);
    assert_eq!(decoded.dims[0].palette.len(), 3);
    assert_eq!(decoded.dims[1].unit, "person/m2");
    assert_eq!(decoded.dims[1].normalise(1.0), 0.5);
}

#[test]
fn weight_prior_roundtrip() {
    use crate::motion::MotionMode;
    let prior = WeightPrior {
        entries: vec![WeightPriorEntry {
            mode: MotionMode::Race,
            weights: vec![0.4, 0.3, 0.3],
            tau: 0.7,
            scale: 1.0,
        }],
    };
    let decoded = WeightPrior::decode(&prior.encode()).unwrap();
    assert_eq!(decoded.get(MotionMode::Race).unwrap().weights.len(), 3);
    assert!(decoded.get(MotionMode::Jog).is_none());
}

#[test]
fn connector_table_roundtrip_and_geometry() {
    use crate::tlv::value::{Connector, ConnectorDirection, ConnectorType};
    let connector = Connector::new(
        ConnectorType::Stair,
        [0.0, 0.0, 0.0],
        [3.0, 4.0, 12.0],
        ConnectorDirection::Both,
        0.5,
        0.7,
        1.4,
    );
    assert!((connector.length_3d() - 13.0).abs() < 1e-4);
    assert!((connector.length_horizontal() - 5.0).abs() < 1e-4);
    assert!((connector.delta_h() - 12.0).abs() < 1e-4);
    assert!((connector.cost_equiv_m(2.0) - (13.0 * 1.4)).abs() < 1e-4);
    assert_eq!(connector.speed(true), 0.5);
    assert_eq!(connector.speed(false), 0.7);

    let table = ConnectorTable {
        connectors: vec![connector],
    };
    let decoded = ConnectorTable::decode(&table.encode()).unwrap();
    assert_eq!(decoded.connectors.len(), 1);
    assert_eq!(decoded.connectors[0], connector);
    assert_eq!(decoded.min_unit_cost(), Some(1.4));
}

#[test]
fn global_stats_roundtrip_and_lookup() {
    use crate::layer::LayerId;
    let stats = GlobalStats {
        channels: vec![ChannelStats {
            layer_id: LayerId::feature(0),
            channel: 0,
            min: 0.0,
            max: 255.0,
            mean: 12.5,
            coverage: 0.98,
        }],
        forbidden_ratio: vec![(LayerId::HARD_FORBIDDEN, 0.12)],
        connector_unit_cost_min: Some(1.1),
    };
    let decoded = GlobalStats::decode(&stats.encode()).unwrap();
    assert_eq!(decoded.channel_min(LayerId::feature(0), 0), Some(0.0));
    assert_eq!(decoded.channel_min(LayerId::feature(1), 0), None);
    assert_eq!(decoded.connector_unit_cost_min, Some(1.1));
}

#[test]
fn derived_layers_roundtrip() {
    use crate::layer::LayerId;
    let mut derived = DerivedLayers::default();
    derived.upsert(DerivedLayerEntry {
        layer_id: LayerId::PRM_GRAPH,
        source_fingerprint: 0xDEAD_BEEF,
        build_params_hash: 0x1234,
        algo_version: 2,
        seeds: vec![1, 2, 3],
    });
    let decoded = DerivedLayers::decode(&derived.encode()).unwrap();
    let entry = decoded.get(LayerId::PRM_GRAPH).unwrap();
    assert_eq!(entry.source_fingerprint, 0xDEAD_BEEF);
    assert_eq!(entry.seeds, vec![1, 2, 3]);
}

#[test]
fn f32_array_helpers() {
    let values = [0.25f32, -1.0, 3.5];
    let encoded = encode_f32_array(&values);
    assert_eq!(decode_f32_array(&encoded).unwrap(), values);
    assert_eq!(encoded[0], 3);
}

#[test]
fn zstd_dict_payload_is_raw() {
    let dict = ZstdDict(vec![1, 2, 3, 4]);
    assert_eq!(dict.encode(), vec![1, 2, 3, 4]);
    assert_eq!(ZstdDict::decode(&[9, 9]).unwrap().0, vec![9, 9]);
}
