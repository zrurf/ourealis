//! Assembling rasterised layers into an OMF image and reading it back.

use approx::assert_relative_eq;

use ourealis_geo_import::GeoBounds;
use ourealis_geo_import::build::{ImportSpec, assemble, feature};
use ourealis_geo_import::raster::Layers;
use ourealis_map_format::surface;
use ourealis_map_format::{DType, LayerId, Map, MotionMode};

fn spec() -> ImportSpec {
    let bounds = GeoBounds::new(0.0, 0.0, 0.002, 0.002).expect("a valid box");
    ImportSpec::new("fixture", bounds).with_resolution_m(10.0)
}

#[test]
fn an_assembled_image_reads_back_with_the_geometry_it_was_built_from() {
    let spec = spec();
    let grid = spec.grid().expect("a valid grid");
    let mut layers = Layers::new(grid.clone());
    layers.elevation[grid.index(1, 1)] = 12.5;
    layers.forbidden[grid.index(2, 2)] = 1.0;
    layers.surface[grid.index(3, 3)] = surface::TRACK as f32;
    layers.direction[grid.index(3, 3)] = 224.0 * 256.0 + 220.0;

    let bytes = assemble(&spec, &layers).expect("a buildable image");
    let map = Map::from_bytes(bytes).expect("a readable image");

    assert_eq!(map.header().bounds, grid.extent());
    assert_eq!(map.header().base_res_cm, 1000);
    assert_eq!(map.header().chunk_size, spec.chunk_size);
    // The builder generates the elevation pyramid plus the full-resolution level.
    assert_eq!(map.header().lod_count, 3);

    let mut ids = map.layer_ids();
    ids.sort_unstable_by_key(|id| id.raw());
    let mut expected = vec![
        LayerId::ELEVATION,
        LayerId::HARD_FORBIDDEN,
        LayerId::feature(feature::SURFACE),
        LayerId::feature(feature::TRAFFIC),
        LayerId::feature(feature::CROWDING),
        LayerId::feature(feature::LIGHTING),
        LayerId::DIRECTION,
    ];
    expected.sort_unstable_by_key(|id| id.raw());
    assert_eq!(ids, expected);
}

#[test]
fn the_quantisation_contract_matches_the_declared_features() {
    let spec = spec();
    let grid = spec.grid().expect("a valid grid");
    let bytes = assemble(&spec, &Layers::new(grid)).expect("a buildable image");
    let map = Map::from_bytes(bytes).expect("a readable image");

    let elevation = map.layer_desc(LayerId::ELEVATION).expect("elevation");
    assert_eq!(elevation.dtype, DType::I16);
    assert_relative_eq!(elevation.scale, 0.1, epsilon = 1e-6);
    assert_eq!(elevation.bias, 0.0);

    let surface_layer = map
        .layer_desc(LayerId::feature(feature::SURFACE))
        .expect("surface");
    assert_eq!(surface_layer.dtype, DType::U8);
    assert_relative_eq!(surface_layer.scale, 1.0, epsilon = 1e-6);

    let direction = map.layer_desc(LayerId::DIRECTION).expect("direction");
    assert_relative_eq!(direction.scale, 1.0, epsilon = 1e-6);
    assert_relative_eq!(direction.bias, 32768.0, epsilon = 1e-3);

    let schema = map
        .feature_schema()
        .expect("a readable schema")
        .expect("a written schema");
    assert_eq!(schema.dims.len(), 5);
    assert_eq!(schema.dims[0].name, "surface_type");
    assert_eq!(schema.dims[0].palette.len(), surface::COUNT);
    assert_eq!(schema.dims[4].name, "direction");

    let prior = map
        .weight_prior()
        .expect("a readable prior")
        .expect("a written prior");
    for mode in [MotionMode::Jog, MotionMode::Moderate, MotionMode::Race] {
        let entry = prior.get(mode).expect("a prior for every motion mode");
        assert_eq!(entry.weights.len(), schema.dims.len());
    }
}

#[test]
fn a_painted_cell_survives_the_round_trip() {
    let spec = spec();
    let grid = spec.grid().expect("a valid grid");
    let mut layers = Layers::new(grid.clone());
    layers.elevation[grid.index(1, 1)] = 12.5;
    layers.forbidden[grid.index(2, 2)] = 1.0;
    layers.surface[grid.index(3, 3)] = surface::TRACK as f32;

    let bytes = assemble(&spec, &layers).expect("a buildable image");
    let map = Map::from_bytes(bytes).expect("a readable image");

    // The 23 x 23 cell grid occupies the south-west corner of a single 128 x 128
    // chunk, so a cell index is also its offset inside the chunk.
    let chunk = |layer| {
        map.chunk(layer, 0, 0)
            .expect("a readable chunk")
            .expect("a stored chunk")
    };

    let elevation = chunk(LayerId::ELEVATION);
    assert_relative_eq!(elevation.get(1, 1, 0), 12.5, epsilon = 1e-3);
    assert_relative_eq!(elevation.get(1, 2, 0), 0.0, epsilon = 1e-3);

    // A bitmap layer returns the occupancy it was packed from: the painted cell
    // is blocked, and the unpainted ground is blocked too, because the raster
    // starts on a lawn rather than on open space.
    let forbidden = chunk(LayerId::HARD_FORBIDDEN);
    assert_eq!(forbidden.get(2, 2, 0), 1.0);
    assert_eq!(forbidden.get(0, 0, 0), 1.0);

    let surface_layer = chunk(LayerId::feature(feature::SURFACE));
    assert_eq!(surface_layer.get(3, 3, 0), surface::TRACK as f32);
    assert_eq!(surface_layer.get(0, 0, 0), surface::GRASS as f32);
}
