//! Shared test fixtures: small deterministic environments and paths.

#![allow(dead_code)]

use glam::DVec2;

use ourealis_map_format::Aabb;
use ourealis_map_format::tlv::value::{FeatureDim, FeatureKind, FeatureSchema};

use ourealis_core::environment::Environment;
use ourealis_core::field::{CostField, CostModelParams, CostWeights, FeatureField, HardMask};
use ourealis_core::graph::ConnectorSet;
use ourealis_core::sim::MapSource;
use ourealis_core::terrain::{DistanceField, Grid2D, Terrain};

/// Grid geometry of the standard test environment.
pub const GRID_CELLS: usize = 120;
/// Cell size of the standard test environment, metres.
pub const GRID_RESOLUTION: f64 = 1.0;
/// Cell size of the connector fixture, metres: the design's field resolution.
pub const CONNECTOR_RESOLUTION: f64 = 2.0;
/// Cells per axis of the connector fixture.
pub const CONNECTOR_GRID_CELLS: usize = 60;

/// Builds a flat environment with a wall of given cells marked forbidden.
///
/// `forbidden` lists `(x, y)` cells; everything else is passable and has unit
/// cost, which makes the geometric expectations easy to state.
pub fn flat_environment(forbidden: &[(usize, usize)]) -> Environment {
    flat_environment_at(GRID_RESOLUTION, forbidden)
}

/// Builds a flat environment on a grid of the given cell size.
///
/// The cell size is not cosmetic for anything involving a connector: its
/// footprint is a fixed band around the link, so on a grid finer than the band
/// every cell near the link is inside it and the cells around an endpoint are
/// isolated from each other. The design's field grid is 2 m, which is what the
/// connector fixture uses.
pub fn flat_environment_at(resolution: f64, forbidden: &[(usize, usize)]) -> Environment {
    let bounds = Aabb::new(
        0.0,
        0.0,
        GRID_CELLS as f64 * GRID_RESOLUTION,
        GRID_CELLS as f64 * GRID_RESOLUTION,
    );
    let grid = Grid2D::new(&bounds, resolution);
    let mut mask = vec![false; grid.len()];
    for (x, y) in forbidden {
        if *x < grid.width && *y < grid.height {
            mask[grid.index(*x, *y)] = true;
        }
    }
    let hard = HardMask::new(grid, mask);
    let terrain = Terrain::flat(&bounds, resolution, 10.0);
    let distance = DistanceField::from_mask(grid, hard.mask());

    // Two feature dimensions: surface category and crowding, both uniform.
    let channels = vec![
        ourealis_core::field::FeatureChannel {
            description: FeatureDim {
                name: "surface".into(),
                unit: String::new(),
                kind: FeatureKind::Category,
                layer_id: ourealis_map_format::LayerId::feature(0),
                channel: 0,
                scale: 1.0,
                bias: 0.0,
                norm_min: 0.0,
                norm_max: 1.0,
                palette: vec![0.5],
            },
            values: vec![0.0; grid.len()],
            min: 0.0,
            max: 0.0,
        },
        ourealis_core::field::FeatureChannel {
            description: FeatureDim {
                name: "crowding".into(),
                unit: String::new(),
                kind: FeatureKind::Scalar,
                layer_id: ourealis_map_format::LayerId::feature(1),
                channel: 0,
                scale: 1.0,
                bias: 0.0,
                norm_min: 0.0,
                norm_max: 1.0,
                palette: Vec::new(),
            },
            values: vec![0.0; grid.len()],
            min: 0.0,
            max: 0.0,
        },
    ];
    let features = FeatureField::new(grid, channels);
    let weights = CostWeights::uniform(features.dim());
    let cost = CostField::synthesize(&features, &hard, &weights, &CostModelParams::default())
        .expect("cost field");
    Environment::from_parts(
        bounds,
        terrain,
        hard,
        distance,
        features,
        cost,
        ConnectorSet::default(),
    )
}

/// Builds an environment with a wall of thickness one, spanning a vertical line
/// with a single gap.
pub fn walled_environment(wall_x: usize, gap_y: usize) -> Environment {
    let mut forbidden = Vec::new();
    for y in 0..GRID_CELLS {
        if y != gap_y {
            forbidden.push((wall_x, y));
        }
    }
    flat_environment(&forbidden)
}

/// Builds an environment where the only way across is a Z-axis connector.
///
/// The link runs from `(x, y)` to `(x + 2, y)` and rises three metres: a stair.
/// A wall one cell thick crosses the map at `x + 1` with a single opening at `y`,
/// and the link spans that opening. The link's footprint is the planar projection
/// widened by the connector radius, so the opening cannot be crossed as flat
/// ground: a route from the west to the east has to use the link, which is the
/// situation a stairwell in a building actually presents.
pub fn connector_environment(x: usize, y: usize) -> Environment {
    let mut forbidden = Vec::new();
    for wall_y in 0..CONNECTOR_GRID_CELLS {
        if wall_y != y {
            forbidden.push((x + 1, wall_y));
        }
    }
    let mut environment = flat_environment_at(CONNECTOR_RESOLUTION, &forbidden);
    let a = environment.grid.cell_center(x, y);
    let b = environment.grid.cell_center(x + 2, y);
    let connector = ourealis_map_format::tlv::value::Connector {
        type_id: ourealis_map_format::tlv::value::ConnectorType::Stair as u16,
        a: [a.x as f32, a.y as f32, 10.0],
        b: [b.x as f32, b.y as f32, 13.0],
        dir_flag: ourealis_map_format::tlv::value::ConnectorDirection::Both,
        v_up: 0.6,
        v_down: 0.8,
        wait_time: 0.0,
        attr_ref: 0,
        unit_cost: 1.0,
    };
    environment.connectors = ConnectorSet::new(&ourealis_map_format::tlv::value::ConnectorTable {
        connectors: vec![connector],
    });
    environment
}

/// A straight horizontal path inside the environment.
///
/// The length is capped so even long test runs stay within the map: a path that
/// leaves the grid has no environment description, which would make lateral
/// offsets illegal for reasons unrelated to the test.
pub fn straight_path(length_m: f64) -> ourealis_core::path::Path {
    let length = length_m.min(GRID_CELLS as f64 * GRID_RESOLUTION - 15.0);
    let points: Vec<DVec2> = (0..=(length as usize))
        .map(|index| DVec2::new(5.0 + index as f64, 50.0))
        .collect();
    ourealis_core::path::Path::new(points).expect("path")
}

/// The [`connector_environment`] geometry as an in-memory OMF map.
///
/// Tests that drive the whole [`ourealis_core::sim::Simulator`] cannot inject a
/// hand-built [`Environment`], so the wall, its single opening and the stair
/// spanning it are packed into a map image here. The terrain steps from 10 m
/// west of the wall to 13 m east of it, so the link's far endpoint agrees with
/// the ground there and a route to the east has to climb the stair.
pub fn connector_map_source(x: usize, y: usize) -> MapSource {
    use ourealis_map_format::builder::MapBuilder;
    use ourealis_map_format::codec::id as codec_id;
    use ourealis_map_format::layer::{DType, LayerDesc, LayerId, LayerKind};
    use ourealis_map_format::tlv::value::{
        ConnectorDirection, ConnectorTable, ConnectorType, MapInfo,
    };
    use ourealis_map_format::writer::MapHeaderSpec;

    let cells = CONNECTOR_GRID_CELLS;
    let side = cells as f64 * CONNECTOR_RESOLUTION;
    let mut elevation = vec![10.0f32; cells * cells];
    let mut forbidden = vec![0.0f32; cells * cells];
    for row in 0..cells {
        for column in 0..cells {
            if column > x + 1 {
                elevation[row * cells + column] = 13.0;
            }
            if column == x + 1 && row != y {
                forbidden[row * cells + column] = 1.0;
            }
        }
    }
    let a = DVec2::new(
        (x as f64 + 0.5) * CONNECTOR_RESOLUTION,
        (y as f64 + 0.5) * CONNECTOR_RESOLUTION,
    );
    let b = DVec2::new(
        (x as f64 + 2.5) * CONNECTOR_RESOLUTION,
        (y as f64 + 0.5) * CONNECTOR_RESOLUTION,
    );
    let schema = FeatureSchema {
        dims: vec![FeatureDim {
            name: "surface".into(),
            unit: String::new(),
            kind: FeatureKind::Scalar,
            layer_id: ourealis_map_format::LayerId::feature(0),
            channel: 0,
            scale: 1.0 / 255.0,
            bias: 0.0,
            norm_min: 0.0,
            norm_max: 1.0,
            palette: Vec::new(),
        }],
    };
    let header = MapHeaderSpec {
        ref_lon: 116.397_f64.to_radians(),
        ref_lat: 39.909_f64.to_radians(),
        epsg: 0,
        bounds: Aabb::new(0.0, 0.0, side, side),
        base_res_cm: (CONNECTOR_RESOLUTION * 100.0).round() as u16,
        chunk_size: 64,
        lod_count: 1,
    };
    let connector = ourealis_map_format::tlv::value::Connector {
        type_id: ConnectorType::Stair as u16,
        a: [a.x as f32, a.y as f32, 10.0],
        b: [b.x as f32, b.y as f32, 13.0],
        dir_flag: ConnectorDirection::Both,
        v_up: 0.6,
        v_down: 0.8,
        wait_time: 0.0,
        attr_ref: 0,
        unit_cost: 1.0,
    };
    let mut builder = MapBuilder::new(header, schema)
        .with_lod_levels(0)
        .with_map_info(MapInfo {
            name: "connector-fixture".into(),
            author: "ourealis".into(),
            built_unix: 1_700_000_000,
            upstream_hash: Vec::new(),
            description: "wall whose only opening is a stair".into(),
        })
        .with_connectors(ConnectorTable {
            connectors: vec![connector],
        });
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
            cells as u32,
            cells as u32,
            elevation,
        )
        .expect("elevation layer");
    builder
        .add_bitmap_layer(
            LayerId::HARD_FORBIDDEN,
            cells as u32,
            cells as u32,
            forbidden,
        )
        .expect("constraint layer");
    builder
        .add_layer(
            LayerDesc::new(
                LayerId::feature(0),
                LayerKind::Raster,
                1,
                DType::U8,
                codec_id::ZSTD,
            ),
            cells as u32,
            cells as u32,
            vec![0.0f32; cells * cells],
        )
        .expect("feature layer");
    MapSource::bytes(builder.build_to_bytes().expect("map image"))
}

/// A path with a constant-radius circular arc, for curvature tests.
///
/// The arc starts well inside the map so a lateral offset has room on both
/// sides; starting on the boundary would make every offset illegal there.
pub fn arc_path(radius_m: f64, sweep_rad: f64) -> ourealis_core::path::Path {
    let steps = 200;
    let origin = DVec2::new(40.0, 30.0);
    let points: Vec<DVec2> = (0..=steps)
        .map(|index| {
            let angle = sweep_rad * index as f64 / steps as f64;
            origin + DVec2::new(radius_m * angle.sin(), radius_m * (1.0 - angle.cos()))
        })
        .collect();
    ourealis_core::path::Path::new(points).expect("arc path")
}

/// A feature schema matching [`flat_environment`].
pub fn feature_schema() -> FeatureSchema {
    FeatureSchema {
        dims: vec![FeatureDim {
            name: "surface".into(),
            unit: String::new(),
            kind: FeatureKind::Category,
            layer_id: ourealis_map_format::LayerId::feature(0),
            channel: 0,
            scale: 1.0,
            bias: 0.0,
            norm_min: 0.0,
            norm_max: 1.0,
            palette: vec![0.5],
        }],
    }
}
