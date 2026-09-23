//! Unit tests for the synthetic map generator (included from `src/synthetic.rs`).

use super::*;
use crate::reader::Map;

#[test]
fn rasterisation_is_deterministic_for_a_seed() {
    let spec = SyntheticMapSpec::compact();
    let first = rasterise(&spec);
    let second = rasterise(&spec);
    assert_eq!(first.elevation, second.elevation);
    assert_eq!(first.forbidden, second.forbidden);
    assert_eq!(first.surface, second.surface);
    assert_eq!(first.traffic, second.traffic);
}

#[test]
fn different_seeds_change_the_details() {
    // The terrain is a fixed analytic shape; the randomised details are the
    // building footprints and the dirt desire lines, both of which show up in
    // the surface categories.
    let mut spec = SyntheticMapSpec::compact();
    let first = rasterise(&spec);
    spec.seed += 1;
    let second = rasterise(&spec);
    assert_ne!(first.surface, second.surface);
}

#[test]
fn layer_shapes_match_the_spec() {
    let spec = SyntheticMapSpec::compact();
    let layers = rasterise(&spec);
    let expected = spec.cell_dims().0 as usize * spec.cell_dims().1 as usize;
    assert_eq!(layers.elevation.len(), expected);
    assert_eq!(layers.forbidden.len(), expected);
    assert_eq!(layers.surface.len(), expected);
}

#[test]
fn the_track_has_a_direction_constraint() {
    let spec = SyntheticMapSpec::compact();
    let layers = rasterise(&spec);
    let constrained = layers
        .direction
        .iter()
        .filter(|value| **value > 0.0)
        .count();
    assert!(constrained > 0, "track must carry a direction constraint");
    assert!(
        (constrained as f64) < layers.direction.len() as f64 * 0.25,
        "only the track should be constrained"
    );
}

#[test]
fn forbidden_cells_exist_and_are_limited() {
    let spec = SyntheticMapSpec::compact();
    let layers = rasterise(&spec);
    let blocked = layers.forbidden.iter().filter(|v| **v != 0.0).count();
    let total = layers.forbidden.len();
    assert!(blocked > 0, "buildings and lake must block cells");
    assert!(
        blocked < total / 2,
        "most of the map should stay passable, blocked {blocked}/{total}"
    );
}

#[test]
fn feature_schema_matches_the_generated_channels() {
    let schema = feature_schema();
    assert_eq!(schema.dim(), 5);
    assert_eq!(schema.dims[0].palette.len(), 5);
    assert_eq!(schema.dims[4].kind, FeatureKind::Direction);
}

#[test]
fn weight_priors_cover_every_mode() {
    let prior = weight_prior();
    for mode in MotionMode::ALL {
        let entry = prior.get(mode).expect("prior per mode");
        assert_eq!(entry.weights.len(), 5);
    }
}

#[test]
fn regions_are_inside_the_map_bounds() {
    let spec = SyntheticMapSpec::compact();
    let layers = rasterise(&spec);
    let regions = regions(&layers);
    assert_eq!(regions.features().len(), 2);
    for outline in regions.polygons() {
        for point in outline {
            assert!(layers.bounds.contains(point[0] as f64, point[1] as f64));
        }
    }
}

#[test]
fn built_map_round_trips_through_the_reader() {
    let spec = SyntheticMapSpec::compact();
    let bytes = build(&spec).expect("build");
    let map = Map::from_bytes(bytes).expect("open");

    assert_eq!(map.header().bounds.width(), spec.width_m);
    // Terrain, constraints, the four feature channels, direction and the regions.
    // The candidate library is opt-in and off in this spec.
    assert_eq!(map.layer_ids().len(), 8);
    assert!(map.kpath_library().unwrap().is_none());
    assert!(map.layer(LayerId::ELEVATION).is_some());
    assert!(map.layer_desc(LayerId::HARD_FORBIDDEN).is_some());
    assert!(map.connectors().unwrap().is_some());
    assert!(map.regions().unwrap().is_some());
    assert!(map.stats().chunk_count > 0);
    map.verify_file_hash().expect("hash must verify");
}

#[test]
fn built_elevation_matches_the_source_raster() {
    let spec = SyntheticMapSpec::compact();
    let (bytes, layers) = build_with_layers(&spec).expect("build");
    let map = Map::from_bytes(bytes).expect("open");

    let probe = (spec.width_m * 0.3, spec.height_m * 0.4);
    let expected = layers.sample(&layers.elevation, probe.0, probe.1);
    let chunk_id = map.grid().chunk_id_at(probe.0, probe.1, 0).expect("chunk");
    let chunk = map
        .chunk(LayerId::ELEVATION, 0, chunk_id)
        .expect("read")
        .expect("present");
    let origin = map.grid().chunk_origin_cell(chunk_id);
    let cell = map.grid().cell_size_m(0);
    let local_x = ((probe.0 - layers.bounds.min_x) / cell) as u32 - origin.0;
    let local_y = ((probe.1 - layers.bounds.min_y) / cell) as u32 - origin.1;
    let got = chunk.get(local_x, local_y, 0);
    assert!(
        (got - expected).abs() <= 0.1,
        "elevation {got} should match {expected} within quantisation"
    );
}

#[test]
fn skeleton_has_coarse_nodes_for_open_areas() {
    let spec = SyntheticMapSpec::compact();
    let bytes = build(&spec).expect("build");
    let map = Map::from_bytes(bytes).expect("open");
    assert!(
        !map.skeleton().is_empty(),
        "open areas should produce coarse skeleton nodes"
    );
    assert!(map.skeleton().iter().all(|n| n.is_leaf() && n.depth > 0));
}

#[test]
fn hard_mask_bitmap_preserves_blocked_cells() {
    let spec = SyntheticMapSpec::compact();
    let (bytes, layers) = build_with_layers(&spec).expect("build");
    let map = Map::from_bytes(bytes).expect("open");

    // A lake cell must read back as blocked, whatever chunk it falls in.
    let lake = (spec.width_m * 0.78, spec.height_m * 0.78);
    let chunk_id = map.grid().chunk_id_at(lake.0, lake.1, 0).expect("chunk");
    let chunk = map
        .chunk(LayerId::HARD_FORBIDDEN, 0, chunk_id)
        .expect("read")
        .expect("present");
    let origin = map.grid().chunk_origin_cell(chunk_id);
    let cell = map.grid().cell_size_m(0);
    let local_x = ((lake.0 - layers.bounds.min_x) / cell) as u32 - origin.0;
    let local_y = ((lake.1 - layers.bounds.min_y) / cell) as u32 - origin.1;
    assert_eq!(chunk.get(local_x, local_y, 0), 1.0);
}

#[test]
fn an_unbounded_grid_is_rejected_instead_of_allocated() {
    // The generator holds one `f32` per cell for seven layers, so a request like this
    // asks for tens of terabytes and used to abort the process in the allocator.
    let spec = SyntheticMapSpec {
        width_m: 3_000_000.0,
        height_m: 3_000_000.0,
        resolution_m: 1.0,
        ..SyntheticMapSpec::default()
    };
    let error = spec
        .validate()
        .expect_err("a 9e12-cell grid must be refused");
    assert!(
        error.to_string().contains("above the"),
        "the message must name the bound: {error}"
    );
    assert!(build(&spec).is_err(), "build must refuse it too");
}

#[test]
fn a_resolution_the_header_cannot_record_is_rejected() {
    for resolution_m in [0.0, -1.0, 0.005, 1000.0] {
        let spec = SyntheticMapSpec {
            resolution_m,
            ..SyntheticMapSpec::compact()
        };
        assert!(
            spec.validate().is_err(),
            "{resolution_m} m resolution must be refused"
        );
    }
}

#[test]
fn the_header_states_the_resolution_the_grid_was_rasterised_at() {
    // Readers derive every cell position from the header's centimetre field, so a
    // spec that is not a whole number of centimetres has to rasterise at the rounded
    // value rather than at the one it was asked for.
    let spec = SyntheticMapSpec {
        width_m: 705.0,
        height_m: 705.0,
        resolution_m: 0.705,
        ..SyntheticMapSpec::compact()
    };
    let bytes = build(&spec).expect("build");
    let map = Map::from_bytes(bytes).expect("open");
    let declared = map.header().base_res_cm as f64 / 100.0;
    assert!((declared - 0.71).abs() < 1e-9, "declared {declared} m");
    let cell = map.grid().cell_size_m(0);
    assert!(
        (cell - declared).abs() < 1e-9,
        "the grid must use the declared resolution: {cell} m against {declared} m"
    );
    // The map extent must hold the number of cells the header implies.
    let expected = (
        (spec.width_m / declared).ceil() as u32,
        (spec.height_m / declared).ceil() as u32,
    );
    assert_eq!(spec.cell_dims(), expected);
}

#[test]
fn global_stats_describe_the_map_and_not_its_padding() {
    let spec = SyntheticMapSpec::compact();
    let bytes = build(&spec).expect("build");
    let map = Map::from_bytes(bytes).expect("open");
    let stats = map.global_stats().expect("read").expect("present");

    // The synthetic campus has a hill but no cell at zero elevation, and its extent is
    // not a whole number of chunks. Counting the chunks' zero padding pulled the
    // minimum down to zero, and folding in the decimated levels shifted the mean.
    let elevation = stats
        .channels
        .iter()
        .find(|channel| channel.layer_id == LayerId::ELEVATION && channel.channel == 0)
        .expect("elevation statistics");
    assert!(
        elevation.min > 0.0,
        "the minimum must come from the map, not from padding: {}",
        elevation.min
    );
    assert!(
        (elevation.coverage - 1.0).abs() < 1e-6,
        "a full-coverage layer reports 1, not {}",
        elevation.coverage
    );

    // The same data, averaged over the in-map cells of the level-0 chunks: the reported
    // mean can only agree if neither the padding nor the upper levels are counted.
    let (map_cells_x, map_cells_y) = map.grid().cell_dims(0);
    let mut sum = 0.0f64;
    let mut count = 0u64;
    for record in map.directory().for_layer_level(LayerId::ELEVATION, 0) {
        let chunk_id = record.chunk_id;
        let Some(chunk) = map.chunk(LayerId::ELEVATION, 0, chunk_id).expect("read") else {
            continue;
        };
        let (origin_x, origin_y) = map.grid().chunk_origin_cell(chunk_id);
        let in_map_w = map_cells_x.saturating_sub(origin_x).min(chunk.width);
        let in_map_h = map_cells_y.saturating_sub(origin_y).min(chunk.height);
        for y in 0..in_map_h {
            for x in 0..in_map_w {
                sum += chunk.get(x, y, 0) as f64;
                count += 1;
            }
        }
    }
    assert!(count > 0, "the elevation layer must have level-0 cells");
    let measured = (sum / count as f64) as f32;
    assert!(
        (measured - elevation.mean).abs() < 1e-2,
        "reported mean {} against the in-map mean {measured}",
        elevation.mean
    );
}
