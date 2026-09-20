//! OMF inspection and editing.
//!
//! An image can be inspected without entering the library, edited into a new
//! image, and patched. Editing goes through map-format's
//! [`ourealis_map_format::Patch`] mechanism, which the container is built around:
//! a patch appends the new payloads, the new meta block and a new directory, and
//! rewrites the header to point at them, so the base data stays where it was. The
//! alternative — decoding every layer and rebuilding the file — would drop the
//! coarser LOD levels and re-encode payloads, which is not what "edit one record"
//! should cost. Edits a patch cannot express are refused as `unsupported` with the
//! reason, never approximated.

use std::sync::Arc;

use axum::Json;
use axum::Router;
use axum::body::{Body, Bytes};
use axum::extract::rejection::JsonRejection;
use axum::http::header::{CONTENT_DISPOSITION, CONTENT_TYPE};
use axum::response::Response;
use axum::routing::post;
use ourealis_map_format::codec::{self, ChunkShape};
use ourealis_map_format::layer::{DType, LayerId};
use ourealis_map_format::patch::{ChunkReplacement, Patch};
use ourealis_map_format::reader::Map;
use ourealis_map_format::region::{RegionFeature, RegionSet, TriggerMode};
use ourealis_map_format::tlv::value::{
    Connector, ConnectorDirection, ConnectorTable, ConnectorType, MagneticField, MapInfo, PrmSeeds,
    Provenance, SlopeModel,
};
use ourealis_map_format::tlv::{TlvRecord, tag};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::api::error::json_rejection;
use crate::api::maps::{
    decode_base64, derived_infos, footer_json, header_json, layer_infos, section_counts,
    tlv_record_list,
};
use crate::app::AppState;
use crate::error::{Result, ServiceError};

/// A request that carries an image as base64 plus a script of edits.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EditRequest {
    /// The image to edit, base64 encoded.
    pub image_base64: String,
    /// Edits to apply, in the order map info, regions, connectors, raw records.
    #[serde(default)]
    pub edits: EditScript,
}

/// A request that carries two images as base64.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PatchRequest {
    /// The base image, base64 encoded.
    pub base_base64: String,
    /// The patch file, base64 encoded.
    pub patch_base64: String,
}

/// The edits of one request.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct EditScript {
    /// Replacement map information record.
    pub set_map_info: Option<MapInfoJson>,
    /// Region set to write.
    pub regions: Option<RegionsEdit>,
    /// Connector table to write.
    pub connectors: Option<ConnectorsEdit>,
    /// Metadata records to rewrite, each given as a tag and its JSON value.
    pub tlv: Vec<TlvEdit>,
}

/// Map information record as JSON.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MapInfoJson {
    /// Map name.
    pub name: String,
    /// Author of the build.
    #[serde(default)]
    pub author: String,
    /// Build time, Unix seconds.
    #[serde(default)]
    pub built_unix: u64,
    /// Upstream data snapshot hash, lowercase or uppercase hex.
    #[serde(default)]
    pub upstream_hash: String,
    /// Free-form description.
    #[serde(default)]
    pub description: String,
}

/// Region features and outlines to write.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegionsEdit {
    /// Features to write.
    #[serde(default)]
    pub features: Vec<RegionFeatureJson>,
    /// Outlines, addressed by a feature's `geom_ref`.
    #[serde(default)]
    pub outlines: Vec<Vec<[f32; 2]>>,
    /// Whether the given content is added to the existing set instead of
    /// replacing it. In merge mode a feature with a known `tag_id` is replaced in
    /// place, a new outline index may also address the outlines already stored,
    /// and everything else is appended.
    #[serde(default)]
    pub merge: bool,
}

/// One region feature as JSON.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegionFeatureJson {
    /// Semantic tag id.
    pub tag_id: u16,
    /// Outline index.
    #[serde(default)]
    pub geom_ref: u16,
    /// Multipath event probability per entry.
    #[serde(default)]
    pub p_mp: f32,
    /// Multipath bias magnitude, metres.
    #[serde(default)]
    pub mp_bias_m: f32,
    /// GNSS dropout probability inside the region.
    #[serde(default)]
    pub p_loss: f32,
    /// `probabilistic` or `spatial_deterministic`.
    #[serde(default = "default_trigger_mode")]
    pub trigger_mode: String,
}

/// Default trigger mode of a region feature.
fn default_trigger_mode() -> String {
    "probabilistic".to_string()
}

/// Connectors to write.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConnectorsEdit {
    /// Connectors to write.
    #[serde(default)]
    pub connectors: Vec<ConnectorJson>,
    /// Whether the given connectors are merged into the existing table instead of
    /// replacing it. In merge mode a connector with the same type and endpoints is
    /// replaced, everything else is appended.
    #[serde(default)]
    pub merge: bool,
}

/// One connector as JSON.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConnectorJson {
    /// Connector kind id.
    pub type_id: u16,
    /// Endpoint A, metres.
    pub a: [f32; 3],
    /// Endpoint B, metres.
    pub b: [f32; 3],
    /// `both`, `a_to_b` or `b_to_a`.
    #[serde(default = "default_connector_direction")]
    pub direction: String,
    /// Speed upwards, m/s.
    #[serde(default)]
    pub v_up: f32,
    /// Speed downwards, m/s.
    #[serde(default)]
    pub v_down: f32,
    /// Waiting time at an endpoint, seconds.
    #[serde(default)]
    pub wait_time: f32,
    /// Vector shape the connector references.
    #[serde(default)]
    pub attr_ref: u32,
    /// Unit cost in equivalent metres.
    #[serde(default)]
    pub unit_cost: f32,
}

/// Default traversal direction of a connector.
fn default_connector_direction() -> String {
    "both".to_string()
}

/// A metadata record to rewrite.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TlvEdit {
    /// Tag, as a name (`map_info`, `slope_model`, …) or its numeric value.
    pub tag: TagSpec,
    /// Record payload as JSON, in the shape the inspector reports.
    pub json: Value,
}

/// A tag named either by its wire name or by its number.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum TagSpec {
    /// One of the metadata tag names the inspector reports, for example
    /// `map_info` or `feature_schema`.
    Name(String),
    /// The numeric tag.
    Number(u32),
}

/// Inspects an image without importing it.
pub async fn inspect(body: Bytes) -> Result<Json<Value>> {
    if body.is_empty() {
        return Err(ServiceError::Invalid(
            "the request body is empty; send the OMF image as the body".to_string(),
        ));
    }
    let map = Map::from_bytes(body.to_vec())?;
    Ok(Json(structure_json(&map)))
}

/// Applies an edit script to an image and returns the new image.
pub async fn edit(body: Result<Json<EditRequest>, JsonRejection>) -> Result<Response> {
    let request = body.map_err(json_rejection)?.0;
    let image = decode_base64(&request.image_base64)?;
    if image.is_empty() {
        return Err(ServiceError::Invalid(
            "image_base64 decodes to an empty image".to_string(),
        ));
    }
    let (edited, name) = apply_edit(&image, &request.edits)?;
    let name = name.unwrap_or_else(|| "map".to_string());
    Ok(image_response(
        edited,
        &format!("{}-edited.omf", slug(&name)),
    ))
}

/// Applies an edit script and returns the new image with the map's own name.
///
/// Shared by the two facades, so an edit applied over HTTP and one applied over
/// gRPC are the same bytes.
pub(crate) fn apply_edit(image: &[u8], edits: &EditScript) -> Result<(Vec<u8>, Option<String>)> {
    let map = Map::from_bytes(image.to_vec())?;
    let patch = build_patch(&map, edits)?;
    let edited = patch.apply(image)?;
    Ok((edited, map.map_info()?.map(|info| info.name)))
}

/// Applies a patch file to a base image and returns the new image.
pub async fn patch(body: Result<Json<PatchRequest>, JsonRejection>) -> Result<Response> {
    let request = body.map_err(json_rejection)?.0;
    let base = decode_base64(&request.base_base64)?;
    let patch_bytes = decode_base64(&request.patch_base64)?;
    if base.is_empty() || patch_bytes.is_empty() {
        return Err(ServiceError::Invalid(
            "base_base64 and patch_base64 must both decode to a non-empty image".to_string(),
        ));
    }
    let patch = Patch::decode(&patch_bytes)?;
    let patched = patch.apply(&base)?;
    Ok(image_response(patched, "patched.omf"))
}

/// Every route of this module.
pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/omf/inspect", post(inspect))
        .route("/omf/edit", post(edit))
        .route("/omf/patch", post(patch))
}

/// Structure tree of an image, as the inspector reports it.
pub(crate) fn structure_json(map: &Map) -> Value {
    let directory: Vec<Value> = map
        .directory()
        .records()
        .iter()
        .map(|record| {
            json!({
                "layer_id": record.layer_id.raw(),
                "level": record.level,
                "chunk_id": record.chunk_id,
                "codec": record.codec,
                "codec_name": codec::name(record.codec),
                "offset": record.offset,
                "comp_len": record.comp_len,
                "raw_len": record.raw_len,
                "flags": record.flags,
                "tombstone": record.flags & ourealis_map_format::ChunkRecord::TOMBSTONE != 0,
            })
        })
        .collect();
    let directory_counts: Vec<Value> = map
        .directory()
        .layer_levels()
        .into_iter()
        .map(|(layer_id, level)| {
            let records = map.records(layer_id, level);
            json!({
                "layer_id": layer_id.raw(),
                "level": level,
                "records": records.len(),
                "chunks": records
                    .iter()
                    .filter(|record| record.flags & ourealis_map_format::ChunkRecord::TOMBSTONE == 0)
                    .count(),
            })
        })
        .collect();
    let leaves = map.skeleton().iter().filter(|node| node.is_leaf()).count();
    let max_depth = map
        .skeleton()
        .iter()
        .map(|node| node.depth)
        .max()
        .unwrap_or(0);
    let rules = map.aggregation_rules().unwrap_or_default();
    let protected: Vec<u16> = map
        .layers()
        .iter()
        .map(|desc| desc.layer_id)
        .filter(|layer_id| layer_id.is_derived())
        .map(LayerId::raw)
        .collect();
    let patchable: Vec<u16> = map
        .layers()
        .iter()
        .map(|desc| desc.layer_id)
        .filter(|layer_id| !layer_id.is_derived())
        .map(LayerId::raw)
        .collect();
    let flags = map.header().flags;
    json!({
        "header": header_json(map.header()),
        "footer": footer_json(map.footer()),
        "flags": {
            "has_prm": flags.contains(ourealis_map_format::HeaderFlags::HAS_PRM),
            "has_zstd_dict": flags.contains(ourealis_map_format::HeaderFlags::HAS_ZSTD_DICT),
            "has_kpath_library": flags.contains(ourealis_map_format::HeaderFlags::HAS_KPATH_LIB),
            "has_regions": flags.contains(ourealis_map_format::HeaderFlags::HAS_REGIONS),
            "has_vectors": flags.contains(ourealis_map_format::HeaderFlags::HAS_VECTORS),
            "debug_data": flags.contains(ourealis_map_format::HeaderFlags::DEBUG_DATA),
        },
        "stats": {
            "file_len": map.stats().file_len,
            "chunk_count": map.stats().chunk_count,
            "layer_count": map.stats().layer_count,
            "stored_bytes": map.stats().stored_bytes,
            "raw_bytes": map.stats().raw_bytes,
        },
        "metadata": tlv_record_list(map.meta_block()),
        "layers": layer_infos(map),
        "sections": section_counts(map),
        "derived": derived_infos(map).unwrap_or_default(),
        "skeleton": {
            "nodes": map.skeleton().len(),
            "leaves": leaves,
            "max_depth": max_depth,
            "aggregation_rules": {
                "proxy_layer": rules.proxy_layer.raw(),
                "proxy_channel": rules.proxy_channel,
                "aggr_scale": rules.aggr_scale,
                "aggr_bias": rules.aggr_bias,
            },
        },
        "directory": directory,
        "directory_counts": directory_counts,
        // What a patch may touch: a derived layer is refused by the format itself
        // because its fingerprint could not detect the edit.
        "patch": {
            "available": !patchable.is_empty(),
            "base_hash64": ourealis_map_format::footer::hash64(map.footer().file_hash),
            "patchable_layers": patchable,
            "protected_layers": protected,
        },
        "geo_referenced": map.has_geo_reference(),
    })
}

/// Builds the patch an edit script asks for.
fn build_patch(map: &Map, edits: &EditScript) -> Result<Patch> {
    let mut patch = Patch::for_map(map, Vec::new())?;
    let mut metadata: Vec<TlvRecord> = Vec::new();

    if let Some(info) = &edits.set_map_info {
        metadata.push(TlvRecord::encode(tag::MAP_INFO, &map_info_of(info)?));
    }
    if let Some(regions) = &edits.regions {
        let payload = regions_payload(map, regions)?;
        let desc = map.layer_desc(LayerId::REGIONS).ok_or_else(|| {
            ServiceError::Unsupported(format!(
                "the map has no region layer ({}); adding one cannot be expressed as a patch",
                LayerId::REGIONS
            ))
        })?;
        patch
            .replacements
            .push(section_replacement(*desc, LayerId::REGIONS, payload, map)?);
    }
    if let Some(connectors) = &edits.connectors {
        metadata.push(TlvRecord::encode(
            tag::CONNECTOR_TABLE,
            &connector_table(map, connectors)?,
        ));
    }
    for edit in &edits.tlv {
        metadata.push(tlv_record(edit)?);
    }
    patch.meta_patch = metadata;
    Ok(patch)
}

/// Encodes a section payload as a chunk replacement of its layer.
///
/// The reader gives a non-chunked layer the shape `(raw_len, 1, 1, u8)`, which is
/// what the codec layer has to be handed so the replaced bytes decode.
fn section_replacement(
    desc: ourealis_map_format::LayerDesc,
    layer_id: LayerId,
    payload: Vec<u8>,
    map: &Map,
) -> Result<ChunkReplacement> {
    if desc.kind.is_chunked() {
        return Err(ServiceError::Unsupported(format!(
            "layer {layer_id} is a chunked raster layer; only section layers can be replaced whole"
        )));
    }
    if payload.len() > u32::MAX as usize {
        return Err(ServiceError::TooLarge(format!(
            "the replacement payload of layer {layer_id} does not fit an OMF chunk"
        )));
    }
    let shape = ChunkShape::new(payload.len() as u32, 1, 1, DType::U8);
    let dict = map
        .meta_block()
        .get_as::<ourealis_map_format::tlv::value::ZstdDict>(tag::ZSTD_DICT)?
        .map(|dict| dict.0)
        .filter(|dict| !dict.is_empty());
    let context = match &dict {
        Some(dict) => codec::CodecContext::with_dict(dict),
        None => codec::CodecContext::none(),
    };
    let stored = codec::encode(desc.codec, &shape, &payload, &context)?;
    Ok(ChunkReplacement::new(
        layer_id,
        0,
        0,
        desc.codec,
        stored,
        payload.len() as u32,
    ))
}

/// Map information record from its JSON form.
fn map_info_of(info: &MapInfoJson) -> Result<MapInfo> {
    Ok(MapInfo {
        name: info.name.clone(),
        author: info.author.clone(),
        built_unix: info.built_unix,
        upstream_hash: decode_hex(&info.upstream_hash)?,
        description: info.description.clone(),
    })
}

/// Region layer payload for a region edit.
fn regions_payload(map: &Map, edit: &RegionsEdit) -> Result<Vec<u8>> {
    let incoming = decode_trigger_modes(&edit.features)?;
    let set = if edit.merge {
        let existing = map.regions()?.unwrap_or_default();
        merge_regions(existing, incoming, &edit.outlines)?
    } else {
        RegionSet::new(incoming, edit.outlines.clone())?
    };
    Ok(set.encode())
}

/// Applies a merge edit to an existing region set.
///
/// A feature with the same tag as a stored one replaces it, so a caller can update a
/// region without resending the whole set; a tag that is not stored is appended.
///
/// **`geom_ref` is always local to the submitted `outlines`.** The wire form names
/// an outline inside the request, which is the only thing a client can know: the
/// studio draws a polygon and sends it with index 0, not with the index it will
/// occupy once the stored outlines are in front of it. The previous behaviour
/// tested `geom_ref >= offset` and only then shifted, so a local index smaller than
/// the stored count was read as an already-combined index and the new feature ended
/// up pointing at a stored outline — a wrong map with no error. The ref is now
/// shifted unconditionally and bounds-checked.
fn merge_regions(
    existing: RegionSet,
    incoming: Vec<RegionFeature>,
    outlines: &[Vec<[f32; 2]>],
) -> Result<RegionSet> {
    let offset = u16::try_from(existing.polygons().len()).map_err(|_| {
        ServiceError::Unsupported(format!(
            "the stored region set has {} outlines, more than a region reference can address",
            existing.polygons().len()
        ))
    })?;
    let incoming_len = u16::try_from(outlines.len()).unwrap_or(u16::MAX);
    let mut features = existing.features().to_vec();
    for mut feature in incoming {
        if feature.geom_ref >= incoming_len {
            return Err(ServiceError::Invalid(format!(
                "region feature references outline {}, but the request carries {incoming_len}",
                feature.geom_ref
            )));
        }
        feature.geom_ref = feature.geom_ref.saturating_add(offset);
        match features
            .iter_mut()
            .find(|held| held.tag_id == feature.tag_id)
        {
            Some(held) => *held = feature,
            None => features.push(feature),
        }
    }
    let mut polygons = existing.polygons().to_vec();
    polygons.extend_from_slice(outlines);
    Ok(RegionSet::new(features, polygons)?)
}

/// Resolves the trigger-mode names of incoming features.
fn decode_trigger_modes(features: &[RegionFeatureJson]) -> Result<Vec<RegionFeature>> {
    features
        .iter()
        .map(|feature| {
            let mode = match feature.trigger_mode.as_str() {
                "probabilistic" | "" => TriggerMode::Probabilistic,
                "spatial_deterministic" => TriggerMode::SpatialDeterministic,
                other => {
                    return Err(ServiceError::Invalid(format!(
                        "unknown trigger_mode {other:?}; expected probabilistic or spatial_deterministic"
                    )));
                }
            };
            Ok(RegionFeature {
                tag_id: feature.tag_id,
                geom_ref: feature.geom_ref,
                p_mp: feature.p_mp,
                mp_bias_m: feature.mp_bias_m,
                p_loss: feature.p_loss,
                mp_mode: mode,
            })
        })
        .collect()
}

/// Connector table for a connector edit.
fn connector_table(map: &Map, edit: &ConnectorsEdit) -> Result<ConnectorTable> {
    let incoming = edit
        .connectors
        .iter()
        .map(connector_of)
        .collect::<Result<Vec<_>>>()?;
    let mut connectors = if edit.merge {
        map.connectors()?
            .map(|table| table.connectors)
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    for connector in incoming {
        match connectors
            .iter_mut()
            .find(|held| same_connector(held, &connector))
        {
            Some(held) => *held = connector,
            None => connectors.push(connector),
        }
    }
    Ok(ConnectorTable { connectors })
}

/// True when two connectors describe the same link, which is what makes an
/// upsert replace one instead of adding a duplicate.
fn same_connector(left: &Connector, right: &Connector) -> bool {
    left.type_id == right.type_id && left.a == right.a && left.b == right.b
}

/// One connector from its JSON form.
fn connector_of(connector: &ConnectorJson) -> Result<Connector> {
    let dir_flag = match connector.direction.as_str() {
        "both" | "" => ConnectorDirection::Both,
        "a_to_b" => ConnectorDirection::AToB,
        "b_to_a" => ConnectorDirection::BToA,
        other => {
            return Err(ServiceError::Invalid(format!(
                "unknown connector direction {other:?}; expected both, a_to_b or b_to_a"
            )));
        }
    };
    // An unknown type id would be stored and then silently ignored by every
    // reader, so it is refused here rather than written.
    ConnectorType::from_u16(connector.type_id).ok_or_else(|| {
        ServiceError::Invalid(format!(
            "unknown connector type_id {}; expected 0..=4",
            connector.type_id
        ))
    })?;
    Ok(Connector {
        type_id: connector.type_id,
        a: connector.a,
        b: connector.b,
        dir_flag,
        v_up: connector.v_up,
        v_down: connector.v_down,
        wait_time: connector.wait_time,
        attr_ref: connector.attr_ref,
        unit_cost: connector.unit_cost,
    })
}

/// Builds the metadata record a rewrite edit asks for.
fn tlv_record(edit: &TlvEdit) -> Result<TlvRecord> {
    let number = match &edit.tag {
        TagSpec::Number(number) => *number,
        TagSpec::Name(name) => tag_of_name(name).ok_or_else(|| {
            ServiceError::Invalid(format!(
                "unknown metadata tag name {name:?}; known names are {}",
                TLV_NAMES.join(", ")
            ))
        })?,
    };
    let record = match number {
        tag::MAP_INFO => {
            let info: MapInfoJson = serde_json::from_value(edit.json.clone())
                .map_err(|error| ServiceError::Invalid(format!("map_info record: {error}")))?;
            TlvRecord::encode(tag::MAP_INFO, &map_info_of(&info)?)
        }
        tag::SLOPE_MODEL => {
            let model: SlopeModelJson = serde_json::from_value(edit.json.clone())
                .map_err(|error| ServiceError::Invalid(format!("slope_model record: {error}")))?;
            TlvRecord::encode(tag::SLOPE_MODEL, &model.into_model()?)
        }
        tag::MAGNETIC_FIELD => {
            let field: MagneticFieldJson =
                serde_json::from_value(edit.json.clone()).map_err(|error| {
                    ServiceError::Invalid(format!("magnetic_field record: {error}"))
                })?;
            TlvRecord::encode(
                tag::MAGNETIC_FIELD,
                &MagneticField {
                    strength_ut: field.strength_ut,
                    declination_deg: field.declination_deg,
                    inclination_deg: field.inclination_deg,
                },
            )
        }
        tag::PRM_SEEDS => {
            let seeds: PrmSeedsJson = serde_json::from_value(edit.json.clone())
                .map_err(|error| ServiceError::Invalid(format!("prm_seeds record: {error}")))?;
            TlvRecord::encode(tag::PRM_SEEDS, &PrmSeeds { seeds: seeds.seeds })
        }
        tag::PROVENANCE => {
            let provenance: ProvenanceJson = serde_json::from_value(edit.json.clone())
                .map_err(|error| ServiceError::Invalid(format!("provenance record: {error}")))?;
            TlvRecord::encode(
                tag::PROVENANCE,
                &Provenance {
                    tool: provenance.tool,
                    tool_version: provenance.tool_version,
                    sources: provenance.sources,
                    algo_versions: provenance.algo_versions,
                },
            )
        }
        tag::CONNECTOR_TABLE => {
            let edit: ConnectorsEdit =
                serde_json::from_value(edit.json.clone()).map_err(|error| {
                    ServiceError::Invalid(format!("connector_table record: {error}"))
                })?;
            let table = ConnectorTable {
                connectors: edit
                    .connectors
                    .iter()
                    .map(connector_of)
                    .collect::<Result<Vec<_>>>()?,
            };
            TlvRecord::encode(tag::CONNECTOR_TABLE, &table)
        }
        other => {
            return Err(ServiceError::Unsupported(format!(
                "metadata tag {other:#010x} cannot be written from JSON; this build can write {}",
                TLV_NAMES.join(", ")
            )));
        }
    };
    Ok(record)
}

/// Names of the metadata tags this build can rewrite from JSON.
const TLV_NAMES: [&str; 6] = [
    "map_info",
    "slope_model",
    "magnetic_field",
    "prm_seeds",
    "provenance",
    "connector_table",
];

/// Numeric tag of a name this build accepts.
fn tag_of_name(name: &str) -> Option<u32> {
    Some(match name {
        "map_info" => tag::MAP_INFO,
        "layer_table" => tag::LAYER_TABLE,
        "feature_schema" => tag::FEATURE_SCHEMA,
        "weight_prior" => tag::WEIGHT_PRIOR,
        "slope_model" => tag::SLOPE_MODEL,
        "connector_table" => tag::CONNECTOR_TABLE,
        "zstd_dict" => tag::ZSTD_DICT,
        "provenance" => tag::PROVENANCE,
        "magnetic_field" => tag::MAGNETIC_FIELD,
        "prm_seeds" => tag::PRM_SEEDS,
        "chunk_layout" => tag::CHUNK_LAYOUT,
        "global_stats" => tag::GLOBAL_STATS,
        "aggregation_rules" => tag::AGGREGATION_RULES,
        "derived_layers" => tag::DERIVED_LAYERS,
        _ => return None,
    })
}

/// Slope model record as JSON.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct SlopeModelJson {
    minetti_clamp: f32,
    k_down_default: f32,
    stair_v_up: f32,
    stair_v_down: f32,
}

impl SlopeModelJson {
    /// Converts to the container's own model, rejecting unusable speeds.
    fn into_model(self) -> Result<SlopeModel> {
        if self.stair_v_up < 0.0 || self.stair_v_down < 0.0 {
            return Err(ServiceError::Invalid(
                "stair speeds must not be negative".to_string(),
            ));
        }
        Ok(SlopeModel {
            minetti_clamp: self.minetti_clamp,
            k_down_default: self.k_down_default,
            stair_v_up: self.stair_v_up,
            stair_v_down: self.stair_v_down,
        })
    }
}

/// Magnetic field record as JSON.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct MagneticFieldJson {
    strength_ut: f32,
    declination_deg: f32,
    inclination_deg: f32,
}

/// PRM seeds record as JSON.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct PrmSeedsJson {
    seeds: Vec<u64>,
}

/// Provenance record as JSON.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProvenanceJson {
    tool: String,
    tool_version: String,
    #[serde(default)]
    sources: Vec<String>,
    #[serde(default)]
    algo_versions: Vec<(u16, u16)>,
}

/// Decodes a lowercase or uppercase hex string.
fn decode_hex(text: &str) -> Result<Vec<u8>> {
    let digits: Vec<u8> = text
        .bytes()
        .filter(|byte| !byte.is_ascii_whitespace())
        .collect();
    if !digits.len().is_multiple_of(2) {
        return Err(ServiceError::Invalid(
            "a hex string must have an even number of digits".to_string(),
        ));
    }
    let mut out = Vec::with_capacity(digits.len() / 2);
    for pair in digits.chunks(2) {
        let high = hex_value(pair[0])?;
        let low = hex_value(pair[1])?;
        out.push((high << 4) | low);
    }
    Ok(out)
}

/// Value of one hex digit.
fn hex_value(digit: u8) -> Result<u8> {
    match digit {
        b'0'..=b'9' => Ok(digit - b'0'),
        b'a'..=b'f' => Ok(digit - b'a' + 10),
        b'A'..=b'F' => Ok(digit - b'A' + 10),
        other => Err(ServiceError::Invalid(format!(
            "unexpected byte {other:#04x} in a hex string"
        ))),
    }
}

/// A response carrying an OMF image.
fn image_response(bytes: Vec<u8>, filename: &str) -> Response {
    let length = bytes.len();
    let mut response = Response::new(Body::from(bytes));
    response.headers_mut().insert(
        CONTENT_TYPE,
        axum::http::HeaderValue::from_static("application/octet-stream"),
    );
    if let Ok(value) =
        axum::http::HeaderValue::from_str(&format!("attachment; filename=\"{filename}\""))
    {
        response.headers_mut().insert(CONTENT_DISPOSITION, value);
    }
    tracing::debug!("returning a {length}-byte OMF image as {filename}");
    response
}

/// Filesystem-safe stem of a download name.
fn slug(name: &str) -> String {
    let mut out: String = name
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    out = out.trim_matches('-').to_string();
    if out.len() > 32 {
        out.truncate(32);
        out = out.trim_end_matches('-').to_string();
    }
    if out.is_empty() {
        "map".to_string()
    } else {
        out
    }
}
