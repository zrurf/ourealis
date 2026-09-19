//! Synthetic campus map generator.
//!
//! Produces a deterministic OMF file from a seed: a road grid, a 400 m oval
//! track with a counter-clockwise direction constraint, buildings and a lake as
//! hard obstacles, elevation with a hill, surface/crowding/lighting feature
//! channels, region annotations and one stair connector.
//!
//! It exists so tests and examples run without an OSM import pipeline, and so
//! the whole format — chunks, codecs, partitioning, regions, connectors — is
//! exercised on every test run. Source layers only: slope, distance transform
//! and graphs are absent on purpose, which also exercises the simulator's
//! "derive it if the map does not carry it" paths.

use std::f64::consts::TAU;

use rand::{RngExt, SeedableRng};
use rand_chacha::ChaCha8Rng;

use crate::builder::{MapBuilder, PartitionOptions};
use crate::codec::id as codec_id;
use crate::error::Result;
use crate::geometry::Aabb;
use crate::layer::{DType, LayerDesc, LayerId, LayerKind};
use crate::motion::MotionMode;
use crate::region::{RegionFeature, RegionSet, RegionTag, TriggerMode};
use crate::tlv::value::*;
use crate::writer::MapHeaderSpec;

/// Surface categories used by the synthetic map.
pub mod surface {
    /// Asphalt road.
    pub const ROAD: u8 = 0;
    /// Rubber running track.
    pub const TRACK: u8 = 1;
    /// Lawn.
    pub const GRASS: u8 = 2;
    /// Paved sidewalk.
    pub const SIDEWALK: u8 = 3;
    /// Compacted dirt path.
    pub const DIRT: u8 = 4;
    /// Normalised resistance values, indexed by category id.
    pub const NORMALISED: [f32; 5] = [0.45, 0.10, 0.70, 0.30, 0.85];
}

/// Feature channel indices of the synthetic map.
pub mod feature {
    /// Surface category channel.
    pub const SURFACE: u8 = 0;
    /// Motor traffic exposure.
    pub const TRAFFIC: u8 = 1;
    /// Pedestrian crowding.
    pub const CROWDING: u8 = 2;
    /// Street lighting.
    pub const LIGHTING: u8 = 3;
}

/// Description of the map to generate.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SyntheticMapSpec {
    /// Whether to attach a K-shortest-path candidate library along the ring road.
    ///
    /// Off by default, and deliberately so: the fixture stores one candidate per
    /// OD pair — the ring edge itself — which is not how a real library is built
    /// (its candidates come from the same generator as the on-line ones). With it
    /// on, a query whose OD cell matches a ring corner takes that edge instead of
    /// the route the planner would have chosen, so the map is a fixture for the
    /// last-mile contract rather than a realistic cache. `tests/library.rs` turns
    /// it on; everything else runs without it.
    pub with_kpath_library: bool,
    /// Width in metres.
    pub width_m: f64,
    /// Height in metres.
    pub height_m: f64,
    /// Cell size in metres.
    pub resolution_m: f64,
    /// Chunk side length in cells.
    pub chunk_size: u16,
    /// Seed controlling the randomised details (building sizes, dirt paths).
    pub seed: u64,
    /// Whether to add a hill with a stair connector.
    pub with_hill: bool,
    /// Whether to add a lake.
    pub with_lake: bool,
}

impl Default for SyntheticMapSpec {
    fn default() -> Self {
        Self {
            with_kpath_library: false,
            width_m: 600.0,
            height_m: 400.0,
            resolution_m: 1.0,
            chunk_size: 128,
            seed: 20_260_918,
            with_hill: true,
            with_lake: true,
        }
    }
}

impl SyntheticMapSpec {
    /// A compact spec suited to unit tests.
    pub fn compact() -> Self {
        Self {
            with_kpath_library: false,
            width_m: 300.0,
            height_m: 200.0,
            resolution_m: 2.0,
            chunk_size: 64,
            ..Default::default()
        }
    }

    /// Cell dimensions of the generated grids.
    pub fn cell_dims(&self) -> (u32, u32) {
        (
            (self.width_m / self.resolution_m).ceil() as u32,
            (self.height_m / self.resolution_m).ceil() as u32,
        )
    }

    /// Map extent in the local metre plane.
    pub fn bounds(&self) -> Aabb {
        Aabb::new(0.0, 0.0, self.width_m, self.height_m)
    }
}

/// Rasterised layers of a synthetic map, before encoding.
pub struct SyntheticLayers {
    /// Map extent.
    pub bounds: Aabb,
    /// Cell dimensions.
    pub dims: (u32, u32),
    /// Elevation in metres.
    pub elevation: Vec<f32>,
    /// Hard obstacle mask, 1.0 = forbidden.
    pub forbidden: Vec<f32>,
    /// Surface category id per cell.
    pub surface: Vec<f32>,
    /// Motor traffic exposure in `[0, 1]`.
    pub traffic: Vec<f32>,
    /// Pedestrian crowding in `[0, 1]`.
    pub crowding: Vec<f32>,
    /// Street lighting in `[0, 1]`.
    pub lighting: Vec<f32>,
    /// Packed direction constraint: `angle_index * 256 + strength`.
    pub direction: Vec<f32>,
}

impl SyntheticLayers {
    fn index(&self, x: u32, y: u32) -> usize {
        y as usize * self.dims.0 as usize + x as usize
    }

    fn cell_center(&self, x: u32, y: u32) -> (f64, f64) {
        (
            self.bounds.min_x + (x as f64 + 0.5) * (self.bounds.width() / self.dims.0 as f64),
            self.bounds.min_y + (y as f64 + 0.5) * (self.bounds.height() / self.dims.1 as f64),
        )
    }

    /// Samples a layer at a position using nearest-cell lookup.
    pub fn sample(&self, layer: &[f32], x: f64, y: f64) -> f32 {
        let cell_x = ((x - self.bounds.min_x) / self.bounds.width() * self.dims.0 as f64) as i64;
        let cell_y = ((y - self.bounds.min_y) / self.bounds.height() * self.dims.1 as f64) as i64;
        let cell_x = cell_x.clamp(0, self.dims.0 as i64 - 1) as u32;
        let cell_y = cell_y.clamp(0, self.dims.1 as i64 - 1) as u32;
        layer[self.index(cell_x, cell_y)]
    }

    /// True when the cell at `(x, y)` is passable.
    pub fn is_passable(&self, x: f64, y: f64) -> bool {
        self.sample(&self.forbidden, x, y) == 0.0
    }
}

/// Rasterises the synthetic map without encoding it.
pub fn rasterise(spec: &SyntheticMapSpec) -> SyntheticLayers {
    let dims = spec.cell_dims();
    let cells = dims.0 as usize * dims.1 as usize;
    let mut layers = SyntheticLayers {
        bounds: spec.bounds(),
        dims,
        elevation: vec![0.0; cells],
        forbidden: vec![0.0; cells],
        surface: vec![surface::GRASS as f32; cells],
        traffic: vec![0.0; cells],
        crowding: vec![0.0; cells],
        lighting: vec![0.5; cells],
        direction: vec![0.0; cells],
    };

    let mut rng = ChaCha8Rng::seed_from_u64(spec.seed);
    let res_x = spec.width_m / dims.0 as f64;
    let res_y = spec.height_m / dims.1 as f64;

    // Roads: a ring road plus two crossing streets.
    let road_width = 8.0;
    let ring = Aabb::new(
        spec.width_m * 0.12,
        spec.height_m * 0.12,
        spec.width_m * 0.88,
        spec.height_m * 0.88,
    );
    for y in 0..dims.1 {
        for x in 0..dims.0 {
            let (cx, cy) = layers.cell_center(x, y);
            let index = layers.index(x, y);
            let on_ring = ring_distance(cx, cy, &ring) <= road_width * 0.5;
            let on_cross = (cx - spec.width_m * 0.5).abs() <= road_width * 0.5
                || (cy - spec.height_m * 0.5).abs() <= road_width * 0.5;
            if on_ring || on_cross {
                layers.surface[index] = surface::ROAD as f32;
                layers.traffic[index] = 0.9;
                layers.lighting[index] = 0.8;
            } else if ring_distance(cx, cy, &ring) <= road_width * 0.5 + 4.0 {
                layers.surface[index] = surface::SIDEWALK as f32;
                layers.traffic[index] = 0.25;
                layers.lighting[index] = 0.75;
            }
        }
    }

    // Oval running track in the west: 400 m circumference, 6 lanes wide.
    let track_center = (spec.width_m * 0.28, spec.height_m * 0.5);
    let track_radius = (spec.width_m * 0.14, spec.height_m * 0.24);
    let track_half_width = 6.0;
    for y in 0..dims.1 {
        for x in 0..dims.0 {
            let (cx, cy) = layers.cell_center(x, y);
            let nx = (cx - track_center.0) / track_radius.0;
            let ny = (cy - track_center.1) / track_radius.1;
            let radius = (nx * nx + ny * ny).sqrt();
            let radial = (radius - 1.0) * track_radius.0.min(track_radius.1);
            if radial.abs() <= track_half_width && !(nx.abs() < 0.2 && ny.abs() < 0.35) {
                let index = layers.index(x, y);
                layers.surface[index] = surface::TRACK as f32;
                layers.traffic[index] = 0.0;
                layers.lighting[index] = 0.6;
                // Counter-clockwise direction preference: the tangent of the
                // oval at this point, quantised to 256 steps, strength in the
                // low byte.
                let angle = (ny.atan2(nx) + std::f64::consts::FRAC_PI_2).rem_euclid(TAU);
                let angle_index = (angle / TAU * 256.0) as u32 % 256;
                layers.direction[index] = (angle_index as f32) * 256.0 + 220.0;
            }
        }
    }

    // Buildings: deterministic rectangles inside the ring, never on roads.
    let mut buildings: Vec<Aabb> = Vec::new();
    for _ in 0..14 {
        let w = 18.0 + rng.random_range(0.0..26.0);
        let h = 14.0 + rng.random_range(0.0..22.0);
        let x0 = ring.min_x + 15.0 + rng.random_range(0.0..(ring.width() - 60.0).max(1.0));
        let y0 = ring.min_y + 15.0 + rng.random_range(0.0..(ring.height() - 60.0).max(1.0));
        let rect = Aabb::new(x0, y0, x0 + w, y0 + h);
        if rect.center().0 < spec.width_m * 0.45 && rect.center().1 > spec.height_m * 0.2 {
            // Keep the track area clear.
            continue;
        }
        buildings.push(rect);
    }
    for rect in &buildings {
        for y in 0..dims.1 {
            for x in 0..dims.0 {
                let (cx, cy) = layers.cell_center(x, y);
                if rect.contains(cx, cy) {
                    let index = layers.index(x, y);
                    layers.forbidden[index] = 1.0;
                    layers.surface[index] = surface::SIDEWALK as f32;
                    layers.traffic[index] = 0.1;
                    layers.lighting[index] = 0.9;
                }
            }
        }
    }

    // Lake in the north-east quadrant.
    if spec.with_lake {
        let lake = (
            spec.width_m * 0.78,
            spec.height_m * 0.78,
            spec.width_m * 0.08,
            spec.height_m * 0.12,
        );
        for y in 0..dims.1 {
            for x in 0..dims.0 {
                let (cx, cy) = layers.cell_center(x, y);
                let dx = (cx - lake.0) / lake.2;
                let dy = (cy - lake.1) / lake.3;
                if dx * dx + dy * dy <= 1.0 {
                    let index = layers.index(x, y);
                    layers.forbidden[index] = 1.0;
                }
            }
        }
    }

    // Dirt desire lines cutting across the lawns.
    for _ in 0..3 {
        let ax = rng.random_range(ring.min_x..ring.max_x);
        let ay = rng.random_range(ring.min_y..ring.max_y);
        let bx = rng.random_range(ring.min_x..ring.max_x);
        let by = rng.random_range(ring.min_y..ring.max_y);
        let touched = line_cells(&layers, (ax, ay), (bx, by), 1.5);
        for index in touched {
            if layers.surface[index] == surface::GRASS as f32 {
                layers.surface[index] = surface::DIRT as f32;
            }
        }
    }

    // Elevation: gentle base slope plus a hill in the north-east corner.
    let hill = (spec.width_m * 0.85, spec.height_m * 0.15, 55.0, 8.0);
    for y in 0..dims.1 {
        for x in 0..dims.0 {
            let (cx, cy) = layers.cell_center(x, y);
            let index = layers.index(x, y);
            let base = 12.0 + (cx / spec.width_m) * 3.0 + (cy / spec.height_m) * 1.5;
            let mut height = base;
            if spec.with_hill {
                let dx = (cx - hill.0) / hill.2;
                let dy = (cy - hill.1) / hill.2;
                let d2 = dx * dx + dy * dy;
                if d2 < 1.0 {
                    height += hill.3 * (1.0 - d2).powi(2);
                }
            }
            layers.elevation[index] = height as f32;
        }
    }

    // Crowding: dense along sidewalks near the ring, sparse on the track.
    for y in 0..dims.1 {
        for x in 0..dims.0 {
            let (cx, cy) = layers.cell_center(x, y);
            let index = layers.index(x, y);
            let ring_d = ring_distance(cx, cy, &ring);
            let crowding = if layers.surface[index] == surface::TRACK as f32 {
                0.05
            } else if ring_d < 20.0 {
                0.35 + 0.25 * (1.0 - (ring_d / 20.0)).max(0.0)
            } else if buildings.iter().any(|b| b.expanded(6.0).contains(cx, cy)) {
                0.5
            } else {
                0.15
            };
            layers.crowding[index] = crowding.clamp(0.0, 1.0) as f32;
        }
    }

    let _ = res_x;
    let _ = res_y;
    layers
}

/// Distance from `(x, y)` to the outline of `ring`, which is the road's
/// centre line. Non-negative on both sides, so callers can compare it against a
/// half-width without caring whether the point is inside or outside.
fn ring_distance(x: f64, y: f64, ring: &Aabb) -> f64 {
    let outside_x = (ring.min_x - x).max(x - ring.max_x);
    let outside_y = (ring.min_y - y).max(y - ring.max_y);
    if outside_x > 0.0 || outside_y > 0.0 {
        outside_x.max(0.0).hypot(outside_y.max(0.0))
    } else {
        (x - ring.min_x)
            .min(ring.max_x - x)
            .min(y - ring.min_y)
            .min(ring.max_y - y)
    }
}

/// Indices of the cells within `half_width` of the segment `a -> b`.
fn line_cells(
    layers: &SyntheticLayers,
    a: (f64, f64),
    b: (f64, f64),
    half_width: f64,
) -> Vec<usize> {
    let steps = ((b.0 - a.0).hypot(b.1 - a.1) / 1.0).ceil().max(1.0) as u32;
    let mut out = Vec::new();
    for step in 0..=steps {
        let t = step as f64 / steps as f64;
        let px = a.0 + (b.0 - a.0) * t;
        let py = a.1 + (b.1 - a.1) * t;
        for y in 0..layers.dims.1 {
            for x in 0..layers.dims.0 {
                let (cx, cy) = layers.cell_center(x, y);
                if (cx - px).hypot(cy - py) <= half_width {
                    out.push(layers.index(x, y));
                }
            }
        }
    }
    out.sort_unstable();
    out.dedup();
    out
}

/// Feature schema of the synthetic map.
pub fn feature_schema() -> FeatureSchema {
    FeatureSchema {
        dims: vec![
            FeatureDim {
                name: "surface_type".into(),
                unit: String::new(),
                kind: FeatureKind::Category,
                layer_id: LayerId::feature(feature::SURFACE),
                channel: 0,
                scale: 1.0,
                bias: 0.0,
                norm_min: 0.0,
                norm_max: 1.0,
                palette: surface::NORMALISED.to_vec(),
            },
            FeatureDim {
                name: "traffic".into(),
                unit: String::new(),
                kind: FeatureKind::Scalar,
                layer_id: LayerId::feature(feature::TRAFFIC),
                channel: 0,
                scale: 1.0 / 255.0,
                bias: 0.0,
                norm_min: 0.0,
                norm_max: 1.0,
                palette: Vec::new(),
            },
            FeatureDim {
                name: "crowding".into(),
                unit: String::new(),
                kind: FeatureKind::Scalar,
                layer_id: LayerId::feature(feature::CROWDING),
                channel: 0,
                scale: 1.0 / 255.0,
                bias: 0.0,
                norm_min: 0.0,
                norm_max: 1.0,
                palette: Vec::new(),
            },
            FeatureDim {
                name: "lighting".into(),
                unit: String::new(),
                kind: FeatureKind::Scalar,
                layer_id: LayerId::feature(feature::LIGHTING),
                channel: 0,
                scale: 1.0 / 255.0,
                bias: 0.0,
                norm_min: 0.0,
                norm_max: 1.0,
                palette: Vec::new(),
            },
            FeatureDim {
                name: "direction".into(),
                unit: String::new(),
                kind: FeatureKind::Direction,
                layer_id: LayerId::DIRECTION,
                channel: 0,
                scale: 1.0,
                bias: 0.0,
                norm_min: 0.0,
                norm_max: 1.0,
                palette: Vec::new(),
            },
        ],
    }
}

/// Default weight priors of the synthetic map.
pub fn weight_prior() -> WeightPrior {
    let mut prior = WeightPrior::default();
    prior.upsert(WeightPriorEntry {
        mode: MotionMode::Jog,
        weights: vec![1.4, 0.8, 0.6, 0.3, 1.0],
        tau: 1.0,
        scale: 1.0,
    });
    prior.upsert(WeightPriorEntry {
        mode: MotionMode::Moderate,
        weights: vec![1.0, 0.9, 0.5, 0.4, 1.0],
        tau: 1.0,
        scale: 1.0,
    });
    prior.upsert(WeightPriorEntry {
        mode: MotionMode::Race,
        weights: vec![0.6, 0.7, 0.4, 0.5, 1.4],
        tau: 1.0,
        scale: 1.0,
    });
    prior
}

/// Builds a synthetic region set: a multipath zone and a covered underpass.
pub fn regions(layers: &SyntheticLayers) -> RegionSet {
    let multipath = vec![
        [
            (layers.bounds.min_x + 40.0) as f32,
            (layers.bounds.min_y + 30.0) as f32,
        ],
        [
            (layers.bounds.max_x - 40.0) as f32,
            (layers.bounds.min_y + 30.0) as f32,
        ],
        [
            (layers.bounds.max_x - 40.0) as f32,
            (layers.bounds.min_y + 90.0) as f32,
        ],
        [
            (layers.bounds.min_x + 40.0) as f32,
            (layers.bounds.min_y + 90.0) as f32,
        ],
    ];
    let underpass = vec![
        [
            (layers.bounds.max_x - 120.0) as f32,
            (layers.bounds.max_y - 60.0) as f32,
        ],
        [
            (layers.bounds.max_x - 60.0) as f32,
            (layers.bounds.max_y - 60.0) as f32,
        ],
        [
            (layers.bounds.max_x - 60.0) as f32,
            (layers.bounds.max_y - 30.0) as f32,
        ],
        [
            (layers.bounds.max_x - 120.0) as f32,
            (layers.bounds.max_y - 30.0) as f32,
        ],
    ];
    RegionSet::new(
        vec![
            RegionFeature {
                tag_id: RegionTag::HighRise as u16,
                geom_ref: 0,
                p_mp: 0.6,
                mp_bias_m: 12.0,
                p_loss: 0.05,
                mp_mode: TriggerMode::SpatialDeterministic,
            },
            RegionFeature {
                tag_id: RegionTag::Tunnel as u16,
                geom_ref: 1,
                p_mp: 0.2,
                mp_bias_m: 6.0,
                p_loss: 0.8,
                mp_mode: TriggerMode::SpatialDeterministic,
            },
        ],
        vec![multipath, underpass],
    )
    .unwrap_or_default()
}

/// Builds a stair connector linking the plain to the hill top.
pub fn connectors(layers: &SyntheticLayers, spec: &SyntheticMapSpec) -> ConnectorTable {
    if !spec.with_hill {
        return ConnectorTable::default();
    }
    let hill_x = spec.width_m * 0.85;
    let hill_y = spec.height_m * 0.15;
    let ground = SyntheticLayers {
        bounds: layers.bounds,
        dims: layers.dims,
        elevation: layers.elevation.clone(),
        forbidden: layers.forbidden.clone(),
        surface: layers.surface.clone(),
        traffic: layers.traffic.clone(),
        crowding: layers.crowding.clone(),
        lighting: layers.lighting.clone(),
        direction: layers.direction.clone(),
    };
    let base_z = ground.sample(&ground.elevation, hill_x + 40.0, hill_y + 40.0);
    let top_z = ground.sample(&ground.elevation, hill_x, hill_y);
    let _ = &ground;
    let mut table = ConnectorTable::default();
    table.connectors.push(Connector {
        type_id: ConnectorType::Stair as u16,
        a: [hill_x as f32 + 40.0, hill_y as f32 + 40.0, base_z],
        b: [hill_x as f32, hill_y as f32, top_z],
        dir_flag: ConnectorDirection::Both,
        v_up: 0.5,
        v_down: 0.7,
        wait_time: 0.0,
        attr_ref: 0,
        unit_cost: 1.4,
    });
    table
}

/// Builds a complete synthetic map image.
pub fn build(spec: &SyntheticMapSpec) -> Result<Vec<u8>> {
    let layers = rasterise(spec);
    let dims = layers.dims;

    let header = MapHeaderSpec {
        ref_lon: 116.397_f64.to_radians(),
        ref_lat: 39.909_f64.to_radians(),
        epsg: 0,
        bounds: layers.bounds,
        base_res_cm: (spec.resolution_m * 100.0).round() as u16,
        chunk_size: spec.chunk_size,
        lod_count: 3,
    };

    let mut builder = MapBuilder::new(header, feature_schema())
        .with_map_info(MapInfo {
            name: "synthetic-campus".into(),
            author: "ourealis".into(),
            built_unix: 1_700_000_000,
            upstream_hash: Vec::new(),
            description: "deterministically generated test map".into(),
        })
        .with_weight_prior(weight_prior())
        .with_magnetic_field(MagneticField::default())
        .with_connectors(connectors(&layers, spec))
        .with_regions(regions(&layers))
        .with_partition_options(PartitionOptions::default())
        .with_partition_proxy(LayerId::HARD_FORBIDDEN, 0)
        .with_lod_levels(2)
        .with_derived_params(
            LayerId::SLOPE,
            0,
            crate::fingerprint::algo::SLOPE,
            Vec::new(),
        )
        .with_derived_params(LayerId::EDT, 0, crate::fingerprint::algo::EDT, Vec::new());

    builder.add_layer(
        LayerDesc::new(
            LayerId::ELEVATION,
            LayerKind::Raster,
            1,
            DType::I16,
            codec_id::DELTA_VERTICAL,
        )
        .with_quantisation(0.1, 0.0),
        dims.0,
        dims.1,
        layers.elevation.clone(),
    )?;
    builder.add_bitmap_layer(
        LayerId::HARD_FORBIDDEN,
        dims.0,
        dims.1,
        layers.forbidden.clone(),
    )?;
    builder.add_layer(
        LayerDesc::new(
            LayerId::feature(feature::SURFACE),
            LayerKind::Raster,
            1,
            DType::U8,
            codec_id::RLE,
        ),
        dims.0,
        dims.1,
        layers.surface.clone(),
    )?;
    builder.add_layer(
        LayerDesc::new(
            LayerId::feature(feature::TRAFFIC),
            LayerKind::Raster,
            1,
            DType::U8,
            codec_id::ZSTD,
        ),
        dims.0,
        dims.1,
        layers.traffic.clone(),
    )?;
    builder.add_layer(
        LayerDesc::new(
            LayerId::feature(feature::CROWDING),
            LayerKind::Raster,
            1,
            DType::U8,
            codec_id::ZSTD,
        ),
        dims.0,
        dims.1,
        layers.crowding.clone(),
    )?;
    builder.add_layer(
        LayerDesc::new(
            LayerId::feature(feature::LIGHTING),
            LayerKind::Raster,
            1,
            DType::U8,
            codec_id::ZSTD,
        ),
        dims.0,
        dims.1,
        layers.lighting.clone(),
    )?;
    builder.add_layer(
        LayerDesc::new(
            LayerId::DIRECTION,
            LayerKind::Raster,
            1,
            DType::U8,
            codec_id::ZSTD,
        ),
        dims.0,
        dims.1,
        layers.direction.clone(),
    )?;

    if spec.with_kpath_library {
        builder = builder.with_kpath_library(ring_library(spec));
    }
    builder.build_to_bytes()
}

/// Builds a synthetic map and returns the rasterised layers alongside it, for
/// tests that need ground truth to compare against.
/// Builds a candidate library along the ring road.
///
/// The entries are the ring's four edges, split into straight samples with the
/// corners as their exact endpoints — which is what the format requires of a
/// stored path and what the last-mile attach contract consumes. A simulator that
/// asked for a route between two corners therefore finds a stored set whose
/// endpoints coincide with the query.
fn ring_library(spec: &SyntheticMapSpec) -> crate::graph::kpath::KPathLibrary {
    use crate::graph::kpath::{KPath, KPathLibrary, KPathParams, OdEntry};

    let ring = Aabb::new(
        spec.width_m * 0.12,
        spec.height_m * 0.12,
        spec.width_m * 0.88,
        spec.height_m * 0.88,
    );
    let corners = [
        (ring.min_x, ring.min_y),
        (ring.max_x, ring.min_y),
        (ring.max_x, ring.max_y),
        (ring.min_x, ring.max_y),
    ];
    let params = KPathParams {
        param_set_id: crate_candidate_param_set_id(),
        key_cell_m: 25.0,
        ..Default::default()
    };
    let mut library = KPathLibrary {
        params,
        ..Default::default()
    };
    for side in 0..corners.len() {
        for (from, to) in [
            (corners[side], corners[(side + 1) % corners.len()]),
            (corners[(side + 1) % corners.len()], corners[side]),
        ] {
            let length = (to.0 - from.0).hypot(to.1 - from.1);
            let count = (length / 5.0).ceil().max(1.0) as usize;
            let samples: Vec<[f32; 3]> = (0..=count)
                .map(|step| {
                    let t = step as f64 / count as f64;
                    [
                        (from.0 + (to.0 - from.0) * t) as f32,
                        (from.1 + (to.1 - from.1) * t) as f32,
                        0.0,
                    ]
                })
                .collect();
            let (start, len) = library.push_nodes(&samples);
            library.insert_set(
                OdEntry {
                    start_key: params.key_of(from.0, from.1),
                    goal_key: params.key_of(to.0, to.1),
                    set_index: 0,
                },
                vec![KPath {
                    total_cost_equiv_m: length as f32,
                    length_m: length as f32,
                    path_size: 1.0,
                    node_range: (start, len),
                }],
            );
        }
    }
    library
}

/// Identifier of the candidate parameters the library is generated with.
///
/// Must match `ourealis_core::search::ksp::param_set_id` for the default
/// parameters, which is what the simulator compares against. The format crate
/// cannot depend on the simulator, so the value is duplicated here with this note;
/// the simulator treats a mismatch as "generate on line", so a drift between the
/// two is a performance regression rather than a correctness one.
fn crate_candidate_param_set_id() -> u32 {
    let mut hash = 0x811C_9DC5u32;
    for value in [5u32, 1600, 800, 3] {
        hash ^= value;
        hash = hash.wrapping_mul(0x0100_0193);
    }
    hash
}

/// Builds the map image together with the raster layers it was built from.
///
/// The layers are what the examples use to draw or cross-check the map without
/// decoding it again.
pub fn build_with_layers(spec: &SyntheticMapSpec) -> Result<(Vec<u8>, SyntheticLayers)> {
    let layers = rasterise(spec);
    let bytes = build(spec)?;
    Ok((bytes, layers))
}

#[cfg(test)]
#[path = "../tests/unit/synthetic.rs"]
mod tests;
