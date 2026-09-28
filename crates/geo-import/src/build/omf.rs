//! Assembling the OMF container from the rasterised layers.
//!
//! The layer set mirrors `ourealis_map_format::synthetic`, which is the fixture
//! the simulator's cost model was calibrated against: elevation and a hard
//! obstacle mask, then one channel per resistance dimension, then the direction
//! field. Slope, the distance transform and the graphs are deliberately left out
//! — the simulator derives them — so an imported map and the fixture exercise
//! the same code paths.

use std::time::{SystemTime, UNIX_EPOCH};

use ourealis_map_format::Aabb;
use ourealis_map_format::MotionMode;
use ourealis_map_format::builder::{MapBuilder, PartitionOptions};
use ourealis_map_format::codec::id as codec_id;
use ourealis_map_format::layer::{DType, LayerDesc, LayerId, LayerKind};
use ourealis_map_format::tlv::value::{
    FeatureDim, FeatureKind, FeatureSchema, MagneticField, MapInfo, WeightPrior, WeightPriorEntry,
};
use ourealis_map_format::writer::MapHeaderSpec;

use crate::build::ImportSpec;
use crate::error::Result;
use crate::osm::class::PALETTE;
use crate::raster::Layers;

/// Resistance feature channel indices, in weight-vector order.
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

/// Number of LOD pyramid levels generated for the elevation layer.
const LOD_LEVELS: u8 = 2;

/// Encodes the layers into a complete OMF image.
pub fn assemble(spec: &ImportSpec, layers: &Layers) -> Result<Vec<u8>> {
    let (width, height) = layers.grid.dims();
    let header = MapHeaderSpec {
        ref_lon: spec.bounds.west.to_radians(),
        ref_lat: spec.bounds.south.to_radians(),
        epsg: 0,
        bounds: Aabb::new(0.0, 0.0, layers.grid.width_m(), layers.grid.height_m()),
        base_res_cm: (layers.grid.res_m() * 100.0).round() as u16,
        chunk_size: spec.chunk_size,
        lod_count: LOD_LEVELS + 1,
    };

    let mut builder = MapBuilder::new(header, feature_schema())
        .with_map_info(MapInfo {
            name: spec.name.clone(),
            author: "ourealis-geo-import".to_owned(),
            built_unix: unix_now(),
            upstream_hash: Vec::new(),
            description: format!(
                "imported from OpenStreetMap and Copernicus DEM GLO-30 over {},{},{},{}",
                spec.bounds.south, spec.bounds.west, spec.bounds.north, spec.bounds.east
            ),
        })
        .with_weight_prior(weight_prior())
        .with_magnetic_field(MagneticField::default())
        .with_partition_options(PartitionOptions::default())
        .with_partition_proxy(LayerId::HARD_FORBIDDEN, 0)
        .with_lod_levels(LOD_LEVELS);

    builder.add_layer(
        LayerDesc::new(
            LayerId::ELEVATION,
            LayerKind::Raster,
            2,
            DType::I16,
            codec_id::DELTA_VERTICAL,
        )
        .with_quantisation(0.1, 0.0),
        width,
        height,
        elevation_channels(layers),
    )?;
    builder.add_bitmap_layer(
        LayerId::HARD_FORBIDDEN,
        width,
        height,
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
        width,
        height,
        layers.surface.clone(),
    )?;
    for (index, values) in [
        (feature::TRAFFIC, &layers.traffic),
        (feature::CROWDING, &layers.crowding),
        (feature::LIGHTING, &layers.lighting),
    ] {
        builder.add_layer(
            LayerDesc::new(
                LayerId::feature(index),
                LayerKind::Raster,
                1,
                DType::U8,
                codec_id::ZSTD,
            )
            .with_quantisation(1.0 / 255.0, 0.0),
            width,
            height,
            values.clone(),
        )?;
    }
    // The direction channel stores `angle_index * 256 + strength`, which needs
    // the whole unsigned 16-bit range; a signed element carries it exactly once
    // the bias moves zero to the bottom of that range.
    builder.add_layer(
        LayerDesc::new(
            LayerId::DIRECTION,
            LayerKind::Raster,
            1,
            DType::I16,
            codec_id::ZSTD,
        )
        .with_quantisation(1.0, 32768.0),
        width,
        height,
        layers.direction.clone(),
    )?;

    Ok(builder.build_to_bytes()?)
}

/// Elevation and the building mask, interleaved the way the format stores channels.
///
/// Channel 0 is the ground the surface is drawn from; channel 1 marks the cells a
/// building occupies, so a viewer can raise a block as the near-white model a navigation
/// display uses instead of colouring it with the elevation ramp. Both ride in one layer,
/// which keeps the mask down to a second channel rather than a second request per chunk;
/// the raster's own resolution is the finest place the mask can be read from at any zoom.
fn elevation_channels(layers: &Layers) -> Vec<f32> {
    let mut data = Vec::with_capacity(layers.elevation.len() * 2);
    for (elevation, height) in layers.elevation.iter().zip(&layers.building_height) {
        data.push(*elevation);
        data.push(if *height > 0.0 { 1.0 } else { 0.0 });
    }
    data
}

/// Feature schema of an imported map.
pub fn feature_schema() -> FeatureSchema {
    let scalar = |name: &str, index: u8| FeatureDim {
        name: name.to_owned(),
        unit: String::new(),
        kind: FeatureKind::Scalar,
        layer_id: LayerId::feature(index),
        channel: 0,
        scale: 1.0 / 255.0,
        bias: 0.0,
        norm_min: 0.0,
        norm_max: 1.0,
        palette: Vec::new(),
    };
    FeatureSchema {
        dims: vec![
            FeatureDim {
                name: "surface_type".to_owned(),
                unit: String::new(),
                kind: FeatureKind::Category,
                layer_id: LayerId::feature(feature::SURFACE),
                channel: 0,
                scale: 1.0,
                bias: 0.0,
                norm_min: 0.0,
                norm_max: 1.0,
                palette: PALETTE.to_vec(),
            },
            scalar("traffic", feature::TRAFFIC),
            scalar("crowding", feature::CROWDING),
            scalar("lighting", feature::LIGHTING),
            FeatureDim {
                name: "direction".to_owned(),
                unit: String::new(),
                kind: FeatureKind::Direction,
                layer_id: LayerId::DIRECTION,
                channel: 0,
                // Matches the layer descriptor: the stored element is the packed
                // `angle_index * 256 + strength`, biased to fit a signed element.
                scale: 1.0,
                bias: 32768.0,
                norm_min: 0.0,
                norm_max: 1.0,
                palette: Vec::new(),
            },
        ],
    }
}

/// Weight priors of an imported map.
///
/// Identical to the synthetic fixture's: the priors are a property of the motion
/// modes and the cost model, not of the place the map describes, and reusing them
/// keeps a route computed on an imported map comparable with one computed on the
/// fixture.
pub fn weight_prior() -> WeightPrior {
    let mut prior = WeightPrior::default();
    for (mode, weights) in [
        (MotionMode::Jog, vec![1.4, 0.8, 0.6, 0.3, 1.0]),
        (MotionMode::Moderate, vec![1.0, 0.9, 0.5, 0.4, 1.0]),
        (MotionMode::Race, vec![0.6, 0.7, 0.4, 0.5, 1.4]),
    ] {
        prior.upsert(WeightPriorEntry {
            mode,
            weights,
            tau: 1.0,
            scale: 1.0,
        });
    }
    prior
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}
