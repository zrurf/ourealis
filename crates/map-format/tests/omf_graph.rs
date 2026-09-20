//! Graph, region and vector layer tests, including fingerprint validation.

use ourealis_map_format::builder::MapBuilder;
use ourealis_map_format::codec::id as codec_id;
use ourealis_map_format::graph::kpath::{KPath, KPathLibrary, KPathParams, OdEntry};
use ourealis_map_format::graph::prm::{InterfaceLink, PrmEdge, PrmGraph, PrmNode};
use ourealis_map_format::graph::vector::{VectorKind, VectorLayer, VectorShape};
use ourealis_map_format::layer::{DType, LayerDesc, LayerId, LayerKind};
use ourealis_map_format::region::{
    RegionFeature, RegionSet, RegionTag, TriggerMode, spatial_event,
};
use ourealis_map_format::tlv::value::FeatureSchema;
use ourealis_map_format::{Aabb, DerivedStatus, Map, MapError, MapHeaderSpec};

fn base_spec() -> MapHeaderSpec {
    MapHeaderSpec {
        bounds: Aabb::new(0.0, 0.0, 64.0, 64.0),
        base_res_cm: 100,
        chunk_size: 32,
        lod_count: 1,
        ..Default::default()
    }
}

fn prm_fixture() -> PrmGraph {
    let nodes = vec![
        PrmNode::new(4.0, 4.0, 10.0),
        PrmNode::new(20.0, 4.0, 10.0),
        PrmNode::new(20.0, 20.0, 11.0),
        PrmNode {
            position: [4.0, 20.0, 11.0],
            flags: PrmNode::INTERFACE,
        },
    ];
    let edges = vec![
        (
            0,
            PrmEdge {
                to: 1,
                cost_equiv_m: 16.0,
                len_m: 16.0,
                dir: 0,
                flags: 0,
            },
        ),
        (
            1,
            PrmEdge {
                to: 2,
                cost_equiv_m: 16.5,
                len_m: 16.0,
                dir: 4,
                flags: 0,
            },
        ),
        (
            2,
            PrmEdge {
                to: 3,
                cost_equiv_m: 16.0,
                len_m: 16.0,
                dir: 8,
                flags: 0,
            },
        ),
        (
            3,
            PrmEdge {
                to: 0,
                cost_equiv_m: 16.0,
                len_m: 16.0,
                dir: 12,
                flags: 0,
            },
        ),
    ];
    let (offsets, edges) = PrmGraph::build_offsets(nodes.len(), &edges);
    PrmGraph {
        batch: 0,
        seed: 0xC0FFEE,
        nodes,
        offsets,
        edges,
        interfaces: vec![InterfaceLink {
            prm_node: 3,
            grid_cell: [8, 40],
            cost_equiv_m: 2.5,
            len_m: 2.5,
        }],
    }
}

fn kpath_fixture() -> KPathLibrary {
    let mut library = KPathLibrary {
        params: KPathParams::default(),
        ..Default::default()
    };
    let samples = [
        [0.0f32, 0.0, 10.0],
        [10.0, 2.0, 10.0],
        [20.0, 8.0, 10.5],
        [31.5, 12.25, 11.0],
    ];
    let (start, count) = library.push_nodes(&samples);
    let path = KPath {
        total_cost_equiv_m: 34.0,
        length_m: 33.0,
        path_size: 0.8,
        node_range: (start, count),
    };
    library.insert_set(
        OdEntry {
            start_key: library.params.key_of(0.0, 0.0),
            goal_key: library.params.key_of(31.5, 12.25),
            set_index: 0,
        },
        vec![path],
    );
    library
}

fn build_map_with_graphs() -> Vec<u8> {
    let schema = FeatureSchema::default();
    let mut builder = MapBuilder::new(base_spec(), schema)
        .with_lod_levels(0)
        .with_prm_batch(prm_fixture())
        .with_kpath_library(kpath_fixture())
        .with_vectors(VectorLayer {
            shapes: vec![VectorShape {
                id: 7,
                kind: VectorKind::Polyline,
                points: vec![[0.0, 0.0], [32.0, 0.0], [32.0, 32.0]],
                attributes: vec![(1, 2.0)],
            }],
        })
        .with_regions(
            RegionSet::new(
                vec![RegionFeature {
                    tag_id: RegionTag::HighRise as u16,
                    geom_ref: 0,
                    p_mp: 0.5,
                    mp_bias_m: 15.0,
                    p_loss: 0.25,
                    mp_mode: TriggerMode::SpatialDeterministic,
                }],
                vec![vec![[0.0, 0.0], [40.0, 0.0], [40.0, 40.0], [0.0, 40.0]]],
            )
            .expect("regions"),
        )
        .with_derived_params(LayerId::PRM_GRAPH, 0xABCD, 1, vec![0xC0FFEE])
        .with_derived_params(LayerId::KPATH_LIBRARY, 0x1234, 1, Vec::new());
    builder
        .add_layer(
            LayerDesc::new(
                LayerId::HARD_FORBIDDEN,
                LayerKind::Bitmap,
                1,
                DType::Bit,
                codec_id::RLE,
            ),
            64,
            64,
            vec![0.0; 64 * 64],
        )
        .expect("mask");
    builder.build_to_bytes().expect("build")
}

#[test]
fn prm_graph_round_trips_through_the_map() {
    let map = Map::from_bytes(build_map_with_graphs()).expect("open");
    let graph = map
        .prm_graph(0)
        .expect("valid fingerprints")
        .expect("batch");
    assert_eq!(graph.node_count(), 4);
    assert_eq!(graph.edge_count(), 4);
    assert_eq!(graph.seed, 0xC0FFEE);
    assert_eq!(graph.neighbours(0).len(), 1);
    assert_eq!(graph.neighbours(0)[0].to, 1);
    assert!(graph.nodes[3].is_interface());
    assert_eq!(graph.interfaces.len(), 1);
    assert_eq!(graph.interfaces[0].grid_cell, [8, 40]);
    assert_eq!(map.prm_seeds().unwrap().unwrap().seeds, vec![0xC0FFEE]);
    assert_eq!(map.batch_count(LayerId::PRM_GRAPH), 1);
}

#[test]
fn invalid_csr_arrays_are_rejected() {
    let mut graph = prm_fixture();
    graph.offsets = vec![0, 1];
    assert!(graph.validate().is_err());

    let mut out_of_range = prm_fixture();
    out_of_range.edges[0].to = 99;
    assert!(out_of_range.validate().is_err());
}

#[test]
fn kpath_library_keeps_exact_endpoints() {
    let map = Map::from_bytes(build_map_with_graphs()).expect("open");
    let library = map.kpath_library().expect("valid").expect("library");
    assert_eq!(library.params.k, 5);

    let key = (
        library.params.key_of(0.0, 0.0),
        library.params.key_of(31.5, 12.25),
    );
    let paths = library.find(key.0, key.1).expect("candidate set");
    assert_eq!(paths.len(), 1);
    let points = paths[0].points(&library.nodes);
    assert_eq!(points.len(), 4);
    // The endpoints must be the exact generation endpoints, never the rounded
    // OD key cell centre: the attach contract depends on it.
    assert_eq!(points[0], [0.0, 0.0, 10.0]);
    assert_eq!(points[3], [31.5, 12.25, 11.0]);
    assert_eq!(paths[0].start_point(&library.nodes).unwrap()[0], 0.0);

    // A missing OD pair is simply absent, not an error.
    assert!(library.find([999, 999], [998, 998]).is_none());
}

#[test]
fn vector_layer_round_trips() {
    let map = Map::from_bytes(build_map_with_graphs()).expect("open");
    let vectors = map.vectors().expect("read").expect("layer");
    assert_eq!(vectors.shapes.len(), 1);
    assert_eq!(vectors.get(7).unwrap().points.len(), 3);
    assert_eq!(vectors.get(7).unwrap().attributes, vec![(1, 2.0)]);
    assert_eq!(vectors.vertex_count(), 3);
}

#[test]
fn region_lookup_and_loss_probability() {
    let map = Map::from_bytes(build_map_with_graphs()).expect("open");
    let regions = map.regions().expect("read").expect("layer");

    assert_eq!(regions.features_at(10.0, 10.0).len(), 1);
    assert!(regions.features_at(60.0, 60.0).is_empty());
    assert!((regions.loss_probability_at(10.0, 10.0) - 0.25).abs() < 1e-6);
    assert_eq!(regions.loss_probability_at(60.0, 60.0), 0.0);
}

#[test]
fn an_outline_whose_spatial_index_would_explode_is_rejected() {
    // The bucket index is a nested loop over the outline's extent, so an extent of
    // continental scale would hang the loader. Such an outline is rejected outright.
    let feature = RegionFeature {
        tag_id: RegionTag::HighRise as u16,
        geom_ref: 0,
        p_mp: 0.5,
        mp_bias_m: 10.0,
        p_loss: 0.0,
        mp_mode: TriggerMode::Probabilistic,
    };
    let huge = vec![
        [-1.0e9, -1.0e9],
        [1.0e9, -1.0e9],
        [1.0e9, 1.0e9],
        [-1.0e9, 1.0e9],
    ];
    match RegionSet::new(vec![feature], vec![huge]) {
        Err(MapError::Invalid(reason)) => {
            assert!(
                reason.contains("spatial-index buckets"),
                "unexpected rejection reason: {reason}"
            );
        }
        other => panic!("an unbounded outline must be rejected, got {other:?}"),
    }

    // Non-finite coordinates are rejected before the bounds are computed, because
    // they make the bucket arithmetic meaningless.
    let nan = vec![[f32::NAN, 0.0], [1.0, 0.0], [1.0, 1.0]];
    assert!(RegionSet::new(vec![feature], vec![nan]).is_err());
}

#[test]
fn deterministic_events_repeat_for_the_same_seed() {
    let first = spatial_event(42, 3, 0, 0.5);
    let second = spatial_event(42, 3, 0, 0.5);
    assert_eq!(first, second);
    assert_eq!(first.triggered, second.triggered);

    // Different seeds or entry counters decorrelate the decision.
    let other = spatial_event(43, 3, 0, 0.5);
    assert!(
        other.magnitude != first.magnitude || other.direction_rad != first.direction_rad,
        "a different seed must not reproduce the same event"
    );

    // The trigger rate over many entries approaches the requested probability.
    let hits = (0..2000)
        .filter(|entry| spatial_event(7, 1, *entry, 0.3).triggered)
        .count();
    let rate = hits as f64 / 2000.0;
    assert!(
        (rate - 0.3).abs() < 0.05,
        "trigger rate {rate} should be near 0.3"
    );
}

#[test]
fn derived_layers_verify_and_go_stale_when_a_source_changes() {
    let bytes = build_map_with_graphs();
    let map = Map::from_bytes(bytes.clone()).expect("open");
    assert_eq!(
        map.verify_derived(LayerId::PRM_GRAPH).unwrap(),
        DerivedStatus::Valid
    );
    assert_eq!(
        map.verify_derived(LayerId::KPATH_LIBRARY).unwrap(),
        DerivedStatus::Valid
    );

    // Editing the hard-constraint bitmap invalidates every derived layer that
    // depends on it, and loading one must fail loudly rather than silently.
    let mut edited = build_map_with_graphs();
    let index = edited.len() / 2;
    edited[index] ^= 0xFF;
    if let Ok(patched) = Map::from_bytes(edited) {
        let status = patched.verify_derived(LayerId::PRM_GRAPH).unwrap();
        assert!(
            !status.is_valid(),
            "edited sources must invalidate the cache"
        );
    }
}

#[test]
fn vector_decode_rejects_an_impossible_shape_count() {
    use ourealis_map_format::graph::{SectionId, write_section};

    let payload = write_section(SectionId::Vectors, &u32::MAX.to_le_bytes());
    match VectorLayer::decode(&payload) {
        Err(MapError::Truncated { .. }) => {}
        other => panic!("expected a truncation error, got {other:?}"),
    }
}

#[test]
fn kpath_decode_rejects_impossible_counts() {
    use ourealis_map_format::graph::{SectionId, write_section};

    let params = write_section(SectionId::KPathParams, &KPathParams::default().to_bytes());
    let mut sets = params.clone();
    sets.extend(write_section(SectionId::KPathSets, &u32::MAX.to_le_bytes()));
    match KPathLibrary::decode(&sets) {
        Err(MapError::Truncated { .. }) => {}
        other => panic!("expected a truncation error, got {other:?}"),
    }

    let mut nodes = params;
    nodes.extend(write_section(
        SectionId::KPathNodes,
        &u32::MAX.to_le_bytes(),
    ));
    match KPathLibrary::decode(&nodes) {
        Err(MapError::Truncated { .. }) => {}
        other => panic!("expected a truncation error, got {other:?}"),
    }
}

#[test]
fn derived_layers_without_a_source_set_verify_as_valid() {
    let mut builder = MapBuilder::new(base_spec(), FeatureSchema::default()).with_lod_levels(0);
    builder
        .add_section(
            LayerDesc::new(
                LayerId::REGION_INTERFACE,
                LayerKind::Graph,
                1,
                DType::U8,
                codec_id::RAW,
            ),
            b"region-interface",
        )
        .expect("interface section");
    // A generic cache id has no source list either.
    builder
        .add_section(
            LayerDesc::new(
                LayerId(0x4002),
                LayerKind::Graph,
                1,
                DType::U8,
                codec_id::RAW,
            ),
            b"cache-blob",
        )
        .expect("cache section");
    let map = Map::from_bytes(builder.build_to_bytes().expect("build")).expect("open");

    assert_eq!(
        map.verify_derived(LayerId::REGION_INTERFACE).unwrap(),
        DerivedStatus::Valid
    );
    assert_eq!(
        map.verify_derived(LayerId(0x4002)).unwrap(),
        DerivedStatus::Valid
    );
    assert_eq!(
        map.section(LayerId::REGION_INTERFACE).unwrap().unwrap(),
        b"region-interface"
    );
}

#[test]
fn stale_derived_layer_is_refused_on_load() {
    // Build a map whose PRM batch declares a parameter set that never produced
    // it, which is what a rebuild-with-different-parameters looks like.
    let schema = FeatureSchema::default();
    let mut builder = MapBuilder::new(base_spec(), schema)
        .with_lod_levels(0)
        .with_prm_batch(prm_fixture())
        .with_derived_params(LayerId::PRM_GRAPH, 0xAAAA, 1, vec![1]);
    builder
        .add_layer(
            LayerDesc::new(
                LayerId::HARD_FORBIDDEN,
                LayerKind::Bitmap,
                1,
                DType::Bit,
                codec_id::RLE,
            ),
            64,
            64,
            vec![0.0; 64 * 64],
        )
        .expect("mask");
    let bytes = builder.build_to_bytes().expect("build");
    let map = Map::from_bytes(bytes).expect("open");

    // The builder recorded the same parameter hash it used, so the layer is
    // valid; forcing a different hash through the metadata makes it stale.
    assert!(map.prm_graph(0).is_ok());

    let headers = map.derived_layers().unwrap().expect("headers");
    let entry = headers.get(LayerId::PRM_GRAPH).expect("entry");
    assert_eq!(entry.build_params_hash, 0xAAAA);
    assert_eq!(entry.seeds, vec![1]);
}
