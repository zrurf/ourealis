//! Map library, metadata and chunk resources.
//!
//! An upload is validated by parsing the image before it enters the library, so a
//! corrupt file is a bad request while the caller can still associate the failure
//! with its own bytes. Everything a registered map exposes is read through
//! [`ourealis_map_format::Map`]'s public API; the optional sections whose types
//! are not `Serialize` are rendered into explicit JSON here, with the field names
//! map-format uses.
//!
//! The builders of this module — `*_dto` and `*_section` — are what both facades
//! call, so the HTTP and gRPC replies are two encodings of one resource.

use std::sync::Arc;

use axum::Json;
use axum::Router;
use axum::body::Bytes;
use axum::extract::rejection::{PathRejection, QueryRejection};
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::get;
use ourealis_map_format::graph::vector::VectorKind;
use ourealis_map_format::layer::{DType, LayerId, LayerKind};
use ourealis_map_format::quadtree::QNode;
use ourealis_map_format::reader::Map;
use ourealis_map_format::region::{RegionFeature, RegionTag};
use ourealis_map_format::tlv::value::{
    Connector, ConnectorDirection, ConnectorTable, ConnectorType, FeatureDim, FeatureKind,
    FeatureSchema, GlobalStats, MagneticField, MapInfo, SlopeModel, WeightPrior,
};
use ourealis_map_format::tlv::{TlvBlock, tag};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::api::dto::map::{
    ChunkData, DerivedLayerDto, FooterDto, HeaderDto, LayerGridDto, LayerInfo, MapMetadata,
    MapSummary, SectionCounts, SectionJson, SkeletonDto, SkeletonNodeDto,
};
use crate::api::dto::{Page, PageQuery};
use crate::api::error::query_rejection;
use crate::app::AppState;
use crate::error::{Result, ServiceError};
use crate::store::{MapEntry, MapStore};

/// Query of the upload and generation endpoints.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct NameQuery {
    /// Name to register the map under.
    #[serde(default)]
    pub name: Option<String>,
}

/// Query selecting a representation.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct FormatQuery {
    /// Representation of the payload: absent or `json` for a JSON array,
    /// `base64` for a base64-encoded sample block.
    #[serde(default)]
    pub format: Option<String>,
}

/// Query selecting a roadmap batch.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct BatchQuery {
    /// Batch index; absent selects the first batch.
    #[serde(default)]
    pub batch: Option<u16>,
}

/// Lists the library, oldest first.
pub async fn list(
    State(state): State<Arc<AppState>>,
    query: Result<Query<PageQuery>, QueryRejection>,
) -> Result<Json<Page<MapSummary>>> {
    let query = query.map_err(query_rejection)?.0;
    let (offset, limit) = state.page(query);
    Ok(Json(entry_page(&state, offset, limit)))
}

/// Imports an OMF image.
///
/// The body is the raw image (`application/octet-stream`); `?name=` overrides the
/// name the map carries in its own information record.
pub async fn create(
    State(state): State<Arc<AppState>>,
    query: Result<Query<NameQuery>, QueryRejection>,
    body: Bytes,
) -> Result<(StatusCode, Json<MapSummary>)> {
    let query = query.map_err(query_rejection)?.0;
    let summary = add_image(state.maps.as_ref(), &body, "import", query.name.as_deref())?;
    Ok((StatusCode::CREATED, Json(summary)))
}

/// Metadata of one map: header, footer, records, layers and derived state.
pub async fn metadata(
    State(state): State<Arc<AppState>>,
    path: Result<Path<String>, PathRejection>,
) -> Result<Json<MapMetadata>> {
    let id = path.map_err(path_rejection)?.0;
    let (entry, map) = open_map(&state, &id)?;
    Ok(Json(metadata_dto(&entry, &map)?))
}

/// Streams the stored OMF image back to the caller.
///
/// The library keeps the exact bytes it was given, so this is a download rather
/// than a rebuild: a client that exports a map gets the file it imported, byte for
/// byte, including any metadata the service itself does not interpret.
pub async fn download(
    State(state): State<Arc<AppState>>,
    path: Result<Path<String>, PathRejection>,
) -> Result<impl IntoResponse> {
    let id = path.map_err(path_rejection)?.0;
    let entry = state.maps.get(&id)?;
    let bytes = entry.bytes.as_ref().clone();
    let headers = [
        (
            axum::http::header::CONTENT_TYPE,
            axum::http::HeaderValue::from_static("application/octet-stream"),
        ),
        (
            axum::http::header::CONTENT_DISPOSITION,
            axum::http::HeaderValue::from_str(&format!("attachment; filename=\"{id}.omf\""))
                .unwrap_or_else(|_| axum::http::HeaderValue::from_static("attachment")),
        ),
    ];
    Ok((headers, bytes))
}

/// Removes one map.
pub async fn remove(
    State(state): State<Arc<AppState>>,
    path: Result<Path<String>, PathRejection>,
) -> Result<StatusCode> {
    let id = path.map_err(path_rejection)?.0;
    state.maps.remove(&id)?;
    tracing::info!("map {id} removed");
    Ok(StatusCode::NO_CONTENT)
}

/// Chunk geometry and stored chunk ids of one layer.
pub async fn layer_grid(
    State(state): State<Arc<AppState>>,
    path: Result<Path<(String, u16)>, PathRejection>,
) -> Result<Json<LayerGridDto>> {
    let (id, layer) = path.map_err(path_rejection)?.0;
    let (_, map) = open_map(&state, &id)?;
    Ok(Json(grid_dto(&map, &id, layer)?))
}

/// One raster chunk.
///
/// `?format=base64` renders the samples as a base64 block of little-endian `f32`
/// values in the same `[y][x][channel]` order, which is what the web client
/// uploads to the GPU; the default is a JSON array.
pub async fn layer_chunk(
    State(state): State<Arc<AppState>>,
    path: Result<Path<(String, u16, u8, u32)>, PathRejection>,
    query: Result<Query<FormatQuery>, QueryRejection>,
) -> Result<axum::response::Response> {
    let (id, layer, level, chunk_id) = path.map_err(path_rejection)?.0;
    let format = query.map_err(query_rejection)?.0.format;
    let (_, map) = open_map(&state, &id)?;
    let chunk = chunk_dto(&map, &id, layer, level, chunk_id)?;
    match format.as_deref() {
        Some("base64") => {
            let mut bytes = Vec::with_capacity(chunk.data.len() * 4);
            for sample in &chunk.data {
                bytes.extend_from_slice(&sample.to_le_bytes());
            }
            Ok(Json(json!({
                "layer_id": chunk.layer_id,
                "level": chunk.level,
                "chunk_id": chunk.chunk_id,
                "width": chunk.width,
                "height": chunk.height,
                "channels": chunk.channels,
                "dtype": chunk.dtype,
                "scale": chunk.scale,
                "bias": chunk.bias,
                "encoding": "base64",
                "byte_order": "little_endian_f32",
                "data": encode_base64(&bytes),
            }))
            .into_response())
        }
        Some(other) if !other.is_empty() && other != "json" => Err(ServiceError::Invalid(format!(
            "unknown chunk format {other:?}; expected json or base64"
        ))),
        _ => Ok(Json(chunk).into_response()),
    }
}

/// Region annotations and their outlines.
///
/// The section is passed through as JSON with the field names map-format uses:
/// its types are not `Serialize`, so the tree is written explicitly.
pub async fn regions(
    State(state): State<Arc<AppState>>,
    path: Result<Path<String>, PathRejection>,
) -> Result<Json<SectionJson>> {
    let id = path.map_err(path_rejection)?.0;
    let (_, map) = open_map(&state, &id)?;
    Ok(Json(SectionJson {
        section: "regions".to_string(),
        json: regions_section(&map)?,
    }))
}

/// Z-axis connector table.
pub async fn connectors(
    State(state): State<Arc<AppState>>,
    path: Result<Path<String>, PathRejection>,
) -> Result<Json<SectionJson>> {
    let id = path.map_err(path_rejection)?.0;
    let (_, map) = open_map(&state, &id)?;
    Ok(Json(SectionJson {
        section: "connectors".to_string(),
        json: connectors_section(&map)?,
    }))
}

/// Vector polylines and polygons.
pub async fn vectors(
    State(state): State<Arc<AppState>>,
    path: Result<Path<String>, PathRejection>,
) -> Result<Json<SectionJson>> {
    let id = path.map_err(path_rejection)?.0;
    let (_, map) = open_map(&state, &id)?;
    Ok(Json(SectionJson {
        section: "vectors".to_string(),
        json: vectors_section(&map)?,
    }))
}

/// One PRM roadmap batch, `?batch=` selecting which.
pub async fn graph_prm(
    State(state): State<Arc<AppState>>,
    path: Result<Path<String>, PathRejection>,
    query: Result<Query<BatchQuery>, QueryRejection>,
) -> Result<Json<SectionJson>> {
    let id = path.map_err(path_rejection)?.0;
    let batch = query.map_err(query_rejection)?.0.batch.unwrap_or(0);
    let (_, map) = open_map(&state, &id)?;
    Ok(Json(SectionJson {
        section: "graph/prm".to_string(),
        json: prm_section(&map, &id, batch)?,
    }))
}

/// Quadtree skeleton nodes.
pub async fn skeleton(
    State(state): State<Arc<AppState>>,
    path: Result<Path<String>, PathRejection>,
) -> Result<Json<SkeletonDto>> {
    let id = path.map_err(path_rejection)?.0;
    let (_, map) = open_map(&state, &id)?;
    Ok(Json(skeleton_dto(&map)))
}

/// Every route of this module.
pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/maps", get(list).post(create))
        .route("/maps/{id}", get(metadata).delete(remove))
        .route("/maps/{id}/image", get(download))
        .route("/maps/{id}/layers/{layer}/grid", get(layer_grid))
        .route(
            "/maps/{id}/layers/{layer}/chunks/{level}/{chunk_id}",
            get(layer_chunk),
        )
        .route("/maps/{id}/regions", get(regions))
        .route("/maps/{id}/connectors", get(connectors))
        .route("/maps/{id}/vectors", get(vectors))
        .route("/maps/{id}/graph/prm", get(graph_prm))
        .route("/maps/{id}/skeleton", get(skeleton))
}

/// Turns a path extraction failure into a classified error.
pub(crate) fn path_rejection(error: axum::extract::rejection::PathRejection) -> ServiceError {
    ServiceError::Invalid(format!("path parameters are not usable: {error}"))
}

/// Builds a library summary from an OMF image.
///
/// The image is parsed here, which is what rejects a corrupt file before it can
/// enter the library: `Map::from_bytes` checks the magic, both CRCs, the
/// directory and the whole-file hash.
pub fn summarise(id: &str, bytes: &[u8], source: &str, created_at_ms: i64) -> Result<MapSummary> {
    let map = Map::from_bytes(bytes.to_vec())?;
    Ok(summary_of(
        id,
        &map,
        bytes.len() as u64,
        source,
        created_at_ms,
    ))
}

/// The summary of an already parsed image.
fn summary_of(
    id: &str,
    map: &Map,
    size_bytes: u64,
    source: &str,
    created_at_ms: i64,
) -> MapSummary {
    let header = map.header();
    let info_name = map
        .map_info()
        .ok()
        .flatten()
        .map(|info| info.name)
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| id.to_string());
    MapSummary {
        id: id.to_string(),
        name: info_name,
        bounds: header.bounds.into(),
        base_res_m: header.base_res_m(),
        chunk_size: header.chunk_size as u32,
        lod_count: header.lod_count as u32,
        feature_dim: header.feature_dim as u32,
        layer_count: map.layers().len() as u32,
        source: source.to_string(),
        created_at: crate::api::time::unix_ms_to_rfc3339(created_at_ms),
        size_bytes,
    }
}

/// Adds an image to the library and returns its summary.
///
/// Import and generation differ only in where the bytes come from, so both go
/// through this: the name resolution is the same for the two — an explicit name
/// wins, then the map's own, then the identifier — and the image is parsed before
/// it is stored, so a corrupt file never enters the library.
pub(crate) fn add_image(
    maps: &dyn MapStore,
    bytes: &[u8],
    source: &str,
    name: Option<&str>,
) -> Result<MapSummary> {
    if bytes.is_empty() {
        return Err(ServiceError::Invalid(
            "the request body is empty; send the OMF image as the body".to_string(),
        ));
    }
    let created_at_ms = crate::store::created_at_ms();
    let mut summary = summarise("pending", bytes, source, created_at_ms)?;
    // The library id is derived from the name the map will be listed under, so the
    // summary is built once to learn that name and then keyed by the chosen id.
    let id = maps.next_id(&summary.name);
    summary.id = id.clone();
    summary.name = preferred_name(name, &summary.name, &id);
    maps.insert(MapEntry {
        id: id.clone(),
        name: summary.name.clone(),
        source: source.to_string(),
        created_at_ms,
        summary: summary.clone(),
        bytes: Arc::new(bytes.to_vec()),
    })?;
    tracing::info!("map {id} added from {source}");
    Ok(summary)
}

/// A page of library summaries.
pub(crate) fn entry_page(state: &AppState, offset: usize, limit: usize) -> Page<MapSummary> {
    let all = state.maps.list();
    let total = all.len();
    let items = all.into_iter().skip(offset).take(limit).collect();
    Page::new(items, total, offset)
}

/// A name for an image, when the caller gave one.
///
/// The map's own information record is only consulted when the request does not
/// name the map: a caller importing a file under a name expects that name.
fn preferred_name(given: Option<&str>, summary_name: &str, id: &str) -> String {
    match given.map(str::trim).filter(|name| !name.is_empty()) {
        Some(name) => name.to_string(),
        None => match summary_name.trim() {
            "" => id.to_string(),
            name => name.to_string(),
        },
    }
}

/// Opens a library entry and parses its image.
///
/// Both the store lookup and the parse failure travel as classified errors, so a
/// map that was stored correctly but cannot be read is reported as a bad image
/// rather than as a missing resource.
pub(crate) fn open_map(state: &AppState, id: &str) -> Result<(Arc<MapEntry>, Map)> {
    let entry = state.maps.get(id)?;
    // A library entry that no longer parses is a storage failure, not a bad request:
    // the caller chose a map the service said it had.
    let map = entry
        .open()
        .map_err(|error| stored_map_error(&entry.id, error))?;
    Ok((entry, map))
}

/// Classifies a container failure on a library entry as a storage failure.
fn stored_map_error(id: &str, error: ServiceError) -> ServiceError {
    match error {
        ServiceError::Map(inner) => ServiceError::from_stored_map(id, inner),
        other => other,
    }
}

/// Everything the inspector and the metadata endpoint show about a map.
pub(crate) fn metadata_dto(entry: &MapEntry, map: &Map) -> Result<MapMetadata> {
    Ok(MapMetadata {
        summary: entry.summary.clone(),
        map_info: map.map_info()?.map(|info| map_info_json(&info)),
        feature_schema: map
            .feature_schema()?
            .map(|schema| feature_schema_json(&schema)),
        weight_prior: map.weight_prior()?.map(|prior| weight_prior_json(&prior)),
        slope_model: Some(slope_model_json(&map.slope_model()?)),
        magnetic_field: map
            .magnetic_field()?
            .map(|field| magnetic_field_json(&field)),
        global_stats: map.global_stats()?.map(|stats| global_stats_json(&stats)),
        layers: layer_infos(map),
        sections: section_counts(map),
        skeleton_nodes: map.skeleton().len(),
        derived: derived_infos(map)?,
        header: header_json(map.header()),
        footer: footer_json(map.footer()),
    })
}

/// Geometry and stored chunk ids of one layer.
pub(crate) fn grid_dto(map: &Map, id: &str, layer: u16) -> Result<LayerGridDto> {
    let layer_id = LayerId(layer);
    let view = map
        .layer(layer_id)
        .ok_or_else(|| ServiceError::not_found(format!("layer {layer} of map {id}")))?;
    let grid = view.grid();
    let levels = map.header().lod_count.max(1) as usize;
    let level_res_m = (0..levels)
        .map(|level| grid.cell_size_m(level as u8))
        .collect();
    let level_dims = (0..levels)
        .map(|level| {
            let (x, y) = grid.cell_dims(level as u8);
            [x, y]
        })
        .collect();
    let (chunk_x, chunk_y) = grid.chunk_dims(0);
    let chunks = (0..levels)
        .map(|level| {
            map.records(layer_id, level as u8)
                .iter()
                .map(|record| record.chunk_id)
                .collect()
        })
        .collect();
    Ok(LayerGridDto {
        layer_id: layer,
        level_res_m,
        level_dims,
        chunk_dim: [chunk_x, chunk_y],
        chunks,
    })
}

/// One decoded raster chunk.
pub(crate) fn chunk_dto(
    map: &Map,
    id: &str,
    layer: u16,
    level: u8,
    chunk_id: u32,
) -> Result<ChunkData> {
    let layer_id = LayerId(layer);
    let desc = *map
        .layer_desc(layer_id)
        .ok_or_else(|| ServiceError::not_found(format!("layer {layer} of map {id}")))?;
    let chunk = map
        .chunk(layer_id, level, chunk_id)
        .map_err(|error| stored_map_error(id, ServiceError::Map(error)))?
        .ok_or_else(|| {
            ServiceError::not_found(format!(
                "chunk {chunk_id} of layer {layer} at level {level}"
            ))
        })?;
    Ok(ChunkData {
        layer_id: layer,
        level,
        chunk_id,
        width: chunk.width,
        height: chunk.height,
        channels: chunk.channels,
        dtype: dtype_name(desc.dtype).to_string(),
        scale: desc.scale as f64,
        bias: desc.bias as f64,
        data: chunk.data,
    })
}

/// Region annotations, as JSON.
pub(crate) fn regions_section(map: &Map) -> Result<Value> {
    Ok(match map.regions()? {
        Some(set) => json!({
            "features": set
                .features()
                .iter()
                .enumerate()
                .map(|(index, feature)| region_feature_json(index, feature))
                .collect::<Vec<_>>(),
            "outlines": set
                .polygons()
                .iter()
                .enumerate()
                .map(|(index, points)| json!({
                    "index": index,
                    "points": points.iter().map(|point| [point[0], point[1]]).collect::<Vec<_>>(),
                }))
                .collect::<Vec<_>>(),
        }),
        None => Value::Null,
    })
}

/// Connector table, as JSON.
pub(crate) fn connectors_section(map: &Map) -> Result<Value> {
    Ok(match map.connectors()? {
        Some(table) => connector_table_json(&table),
        None => Value::Null,
    })
}

/// Vector layer, as JSON.
pub(crate) fn vectors_section(map: &Map) -> Result<Value> {
    Ok(match map.vectors()? {
        Some(layer) => json!({
            "shapes": layer
                .shapes
                .iter()
                .map(|shape| json!({
                    "id": shape.id,
                    "kind": vector_kind_name(shape.kind),
                    "points": shape.points.iter().map(|point| [point[0], point[1]]).collect::<Vec<_>>(),
                    "attributes": shape
                        .attributes
                        .iter()
                        .map(|(key, value)| json!({ "key": key, "value": value }))
                        .collect::<Vec<_>>(),
                }))
                .collect::<Vec<_>>(),
        }),
        None => Value::Null,
    })
}

/// One PRM roadmap batch, as JSON.
pub(crate) fn prm_section(map: &Map, id: &str, batch: u16) -> Result<Value> {
    if map.layer_desc(LayerId::PRM_GRAPH).is_none() {
        return Ok(Value::Null);
    }
    let graph = map
        .prm_graph(batch)?
        .ok_or_else(|| ServiceError::not_found(format!("PRM batch {batch} of map {id}")))?;
    let nodes: Vec<Value> = graph
        .nodes
        .iter()
        .map(|node| {
            json!({
                "position": node.position,
                "flags": node.flags,
                "interface": node.flags & ourealis_map_format::PrmNode::INTERFACE != 0,
                "connector_endpoint": node.flags & ourealis_map_format::PrmNode::CONNECTOR_ENDPOINT != 0,
                "direction_constrained": node.flags & ourealis_map_format::PrmNode::DIRECTION_CONSTRAINED != 0,
            })
        })
        .collect();
    // The stored edge array is CSR; the wire shape is a flat edge list with the
    // source node spelled out, which is what a viewer wants.
    let mut edges = Vec::new();
    for from in 0..graph.nodes.len() {
        let start = graph.offsets.get(from).copied().unwrap_or(0) as usize;
        let end = graph.offsets.get(from + 1).copied().unwrap_or(start as u32) as usize;
        for edge in graph.edges.get(start..end).unwrap_or(&[]) {
            edges.push(json!({
                "from": from,
                "to": edge.to,
                "cost_equiv_m": edge.cost_equiv_m,
                "len_m": edge.len_m,
                "dir": edge.dir,
                "flags": edge.flags,
            }));
        }
    }
    let interfaces: Vec<Value> = graph
        .interfaces
        .iter()
        .map(|link| {
            json!({
                "prm_node": link.prm_node,
                "grid_cell": link.grid_cell,
                "cost_equiv_m": link.cost_equiv_m,
                "len_m": link.len_m,
            })
        })
        .collect();
    Ok(json!({
        "batch": graph.batch,
        "seed": graph.seed,
        "nodes": nodes,
        "edges": edges,
        "interfaces": interfaces,
    }))
}

/// Quadtree skeleton nodes.
pub(crate) fn skeleton_dto(map: &Map) -> SkeletonDto {
    let bounds = map.header().bounds;
    let base_res_m = map.header().base_res_m();
    let nodes = map
        .skeleton()
        .iter()
        .map(|node| SkeletonNodeDto {
            key: node.morton,
            depth: node.depth,
            bounds: node.bounds(&bounds, base_res_m).into(),
            leaf: node.is_leaf(),
            has_aggregate_max: node.flags & QNode::HAS_AGGR_MAX != 0,
            drill_hint: node.flags & QNode::DRILL_HINT != 0,
            suspect_forbidden: node.flags & QNode::LIKELY_FORBIDDEN != 0,
            direction_constrained: node.flags & QNode::DIRECTION_CONSTRAINED != 0,
            aggregate_mean: Some(node.aggr_mean),
            aggregate_max: (node.flags & QNode::HAS_AGGR_MAX != 0).then_some(node.aggr_max),
        })
        .collect();
    SkeletonDto { nodes }
}

/// Header fields of an image.
pub(crate) fn header_json(header: &ourealis_map_format::Header) -> HeaderDto {
    HeaderDto {
        version_major: header.version_major,
        version_minor: header.version_minor,
        flags: format!("{:#010x}", header.flags.0),
        ref_lon_deg: header.ref_lon.to_degrees(),
        ref_lat_deg: header.ref_lat.to_degrees(),
        epsg: header.epsg,
        bounds: header.bounds.into(),
        base_res_cm: header.base_res_cm,
        chunk_size: header.chunk_size,
        lod_count: header.lod_count,
        feature_dim: header.feature_dim,
        layer_count: header.layer_count,
        meta_offset: header.meta_offset,
        meta_len: header.meta_len,
        dir_offset: header.dir_offset,
        dir_len: header.dir_len,
        ext_meta_offset: header.ext_meta_offset,
        ext_meta_len: header.ext_meta_len,
    }
}

/// Footer summary of an image.
pub(crate) fn footer_json(footer: &ourealis_map_format::Footer) -> FooterDto {
    FooterDto {
        version_major: footer.version_major,
        version_minor: footer.version_minor,
        dir_record_count: footer.dir_record_count,
        chunk_count: footer.chunk_count,
        node_count: footer.node_count,
        layer_count: footer.layer_count,
        file_len: footer.file_len,
        file_hash: hex(&footer.file_hash),
    }
}

/// Registered layers with their geometry.
pub(crate) fn layer_infos(map: &Map) -> Vec<LayerInfo> {
    map.layers()
        .iter()
        .map(|desc| LayerInfo {
            layer_id: desc.layer_id.raw(),
            kind: layer_kind_name(desc.kind).to_string(),
            channels: desc.channels,
            dtype: dtype_name(desc.dtype).to_string(),
            codec: desc.codec,
            scale: desc.scale as f64,
            bias: desc.bias as f64,
            sparse: desc.sparse,
            levels: map
                .layer(desc.layer_id)
                .map(|view| view.levels().to_vec())
                .unwrap_or_default(),
        })
        .collect()
}

/// Counts of the optional sections a map carries.
pub(crate) fn section_counts(map: &Map) -> SectionCounts {
    SectionCounts {
        connectors: map.connectors().ok().flatten().map(|t| t.connectors.len()),
        regions: map.regions().ok().flatten().map(|set| set.features().len()),
        vectors: map.vectors().ok().flatten().map(|layer| layer.shapes.len()),
        roadmap_batches: map
            .layer_desc(LayerId::PRM_GRAPH)
            .map(|_| map.batch_count(LayerId::PRM_GRAPH)),
        // Decoding the library is the only way to count its entries, and a stale
        // fingerprint must not be reported as a count, so an unusable library
        // reads as absent rather than as zero.
        library_paths: map
            .kpath_library()
            .ok()
            .flatten()
            .map(|library| library.od_index.len()),
    }
}

/// Fingerprint state of every derived layer of a map.
pub(crate) fn derived_infos(map: &Map) -> Result<Vec<DerivedLayerDto>> {
    let mut out = Vec::new();
    for desc in map.layers() {
        let layer_id = desc.layer_id;
        if !layer_id.is_derived() {
            continue;
        }
        let (status, reason) = match map.verify_derived(layer_id)? {
            ourealis_map_format::DerivedStatus::Valid => ("valid".to_string(), None),
            ourealis_map_format::DerivedStatus::Stale { reason } => {
                ("stale".to_string(), Some(reason))
            }
        };
        out.push(DerivedLayerDto {
            layer_id: layer_id.raw(),
            name: layer_name(layer_id),
            status,
            reason,
        });
    }
    // A fingerprint table entry without a registered layer is reported as absent
    // rather than ignored: the file declares a cache it does not carry.
    if let Some(table) = map.derived_layers()? {
        for entry in &table.entries {
            if map.layer_desc(entry.layer_id).is_none() {
                out.push(DerivedLayerDto {
                    layer_id: entry.layer_id.raw(),
                    name: layer_name(entry.layer_id),
                    status: "absent".to_string(),
                    reason: Some(format!(
                        "the fingerprint table names layer {} which the registry does not carry",
                        entry.layer_id
                    )),
                });
            }
        }
    }
    Ok(out)
}

/// Human-readable name of a layer id.
pub(crate) fn layer_name(layer_id: LayerId) -> String {
    match layer_id {
        LayerId::ELEVATION => "elevation".to_string(),
        LayerId::SLOPE => "slope".to_string(),
        LayerId::EDT => "edt".to_string(),
        LayerId::HARD_FORBIDDEN => "hard_forbidden".to_string(),
        LayerId::DIRECTION => "direction".to_string(),
        LayerId::SOFT_MULTIPLIER => "soft_multiplier".to_string(),
        LayerId::REGIONS => "regions".to_string(),
        LayerId::PRM_GRAPH => "prm_graph".to_string(),
        LayerId::KPATH_LIBRARY => "kpath_library".to_string(),
        LayerId::REGION_INTERFACE => "region_interface".to_string(),
        LayerId::VECTORS => "vectors".to_string(),
        LayerId::COST_CACHE => "cost_cache".to_string(),
        LayerId(id) if (LayerId::FEATURE_BASE..LayerId::FEATURE_BASE + 256).contains(&id) => {
            format!("feature_{}", id - LayerId::FEATURE_BASE)
        }
        _ => format!("{layer_id}"),
    }
}

/// Wire name of a layer kind.
pub(crate) fn layer_kind_name(kind: LayerKind) -> &'static str {
    match kind {
        LayerKind::Raster => "raster",
        LayerKind::Vector => "vector",
        LayerKind::Graph => "graph",
        LayerKind::Bitmap => "bitmap",
        LayerKind::RegionPolygons => "region",
    }
}

/// Wire name of an element type.
pub(crate) fn dtype_name(dtype: DType) -> &'static str {
    match dtype {
        DType::F32 => "f32",
        DType::I32 => "i32",
        DType::I16 => "i16",
        DType::U8 => "u8",
        DType::Bit => "bit",
        DType::F16 => "f16",
    }
}

/// Lowercase hex of a byte string.
pub(crate) fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(char::from_digit((byte >> 4) as u32, 16).unwrap_or('0'));
        out.push(char::from_digit((byte & 0x0F) as u32, 16).unwrap_or('0'));
    }
    out
}

/// Encodes bytes as standard base64 with padding.
pub(crate) fn encode_base64(bytes: &[u8]) -> String {
    crate::api::base64::encode(bytes)
}

/// Decodes standard or URL-safe base64, ignoring whitespace.
pub(crate) fn decode_base64(text: &str) -> Result<Vec<u8>> {
    crate::api::base64::decode(text, "the image")
}

/// Map information record.
fn map_info_json(info: &MapInfo) -> Value {
    json!({
        "name": info.name,
        "author": info.author,
        "built_unix": info.built_unix,
        "upstream_hash": hex(&info.upstream_hash),
        "description": info.description,
    })
}

/// Resistance feature schema.
fn feature_schema_json(schema: &FeatureSchema) -> Value {
    json!({
        "dimension": schema.dim(),
        "dims": schema.dims.iter().map(feature_dim_json).collect::<Vec<_>>(),
    })
}

/// One feature dimension.
fn feature_dim_json(dim: &FeatureDim) -> Value {
    json!({
        "name": dim.name,
        "unit": dim.unit,
        "kind": feature_kind_name(dim.kind),
        "layer_id": dim.layer_id.raw(),
        "channel": dim.channel,
        "scale": dim.scale,
        "bias": dim.bias,
        "norm_min": dim.norm_min,
        "norm_max": dim.norm_max,
        "palette": dim.palette,
    })
}

/// Weight priors per motion mode.
fn weight_prior_json(prior: &WeightPrior) -> Value {
    json!({
        "entries": prior
            .entries
            .iter()
            .map(|entry| json!({
                "mode": mode_name(entry.mode),
                "weights": entry.weights,
                "tau": entry.tau,
                "scale": entry.scale,
            }))
            .collect::<Vec<_>>(),
    })
}

/// Slope model in effect.
fn slope_model_json(model: &SlopeModel) -> Value {
    json!({
        "minetti_clamp": model.minetti_clamp,
        "k_down_default": model.k_down_default,
        "stair_v_up": model.stair_v_up,
        "stair_v_down": model.stair_v_down,
    })
}

/// Magnetic field declaration.
fn magnetic_field_json(field: &MagneticField) -> Value {
    json!({
        "strength_ut": field.strength_ut,
        "declination_deg": field.declination_deg,
        "inclination_deg": field.inclination_deg,
    })
}

/// Global statistics.
fn global_stats_json(stats: &GlobalStats) -> Value {
    json!({
        "channels": stats
            .channels
            .iter()
            .map(|channel| json!({
                "layer_id": channel.layer_id.raw(),
                "channel": channel.channel,
                "min": channel.min,
                "max": channel.max,
                "mean": channel.mean,
                "coverage": channel.coverage,
            }))
            .collect::<Vec<_>>(),
        "forbidden_ratio": stats
            .forbidden_ratio
            .iter()
            .map(|(layer_id, ratio)| json!({ "layer_id": layer_id.raw(), "ratio": ratio }))
            .collect::<Vec<_>>(),
        "connector_unit_cost_min": stats.connector_unit_cost_min,
    })
}

/// One region feature.
fn region_feature_json(index: usize, feature: &RegionFeature) -> Value {
    json!({
        "index": index,
        "tag_id": feature.tag_id,
        "tag": feature.tag().map(region_tag_name),
        "geom_ref": feature.geom_ref,
        "p_mp": feature.p_mp,
        "mp_bias_m": feature.mp_bias_m,
        "p_loss": feature.p_loss,
        "trigger_mode": match feature.mp_mode {
            ourealis_map_format::TriggerMode::Probabilistic => "probabilistic",
            ourealis_map_format::TriggerMode::SpatialDeterministic => "spatial_deterministic",
        },
    })
}

/// Z-axis connector table.
fn connector_table_json(table: &ConnectorTable) -> Value {
    json!({
        "connectors": table.connectors.iter().map(connector_json).collect::<Vec<_>>(),
        "unit_cost_min": table.min_unit_cost(),
    })
}

/// One connector.
fn connector_json(connector: &Connector) -> Value {
    json!({
        "type_id": connector.type_id,
        "type": ConnectorType::from_u16(connector.type_id).map(|kind| match kind {
            ConnectorType::Stair => "stair",
            ConnectorType::Elevator => "elevator",
            ConnectorType::Overpass => "overpass",
            ConnectorType::Underpass => "underpass",
            ConnectorType::Loop => "loop",
        }),
        "direction": match connector.dir_flag {
            ConnectorDirection::Both => "both",
            ConnectorDirection::AToB => "a_to_b",
            ConnectorDirection::BToA => "b_to_a",
        },
        "a": connector.a,
        "b": connector.b,
        "v_up": connector.v_up,
        "v_down": connector.v_down,
        "wait_time_s": connector.wait_time,
        "attr_ref": connector.attr_ref,
        "unit_cost": connector.unit_cost,
        "length_3d_m": connector.length_3d(),
        "length_horizontal_m": connector.length_horizontal(),
    })
}

/// Wire name of a feature kind.
fn feature_kind_name(kind: FeatureKind) -> &'static str {
    match kind {
        FeatureKind::Scalar => "scalar",
        FeatureKind::Category => "category",
        FeatureKind::Direction => "direction",
        FeatureKind::Boolean => "boolean",
    }
}

/// Wire name of a motion mode.
fn mode_name(mode: ourealis_map_format::MotionMode) -> &'static str {
    match mode.as_u8() {
        0 => "jog",
        1 => "moderate",
        _ => "race",
    }
}

/// Wire name of a region tag, when the tag is known.
fn region_tag_name(tag: RegionTag) -> &'static str {
    match tag {
        RegionTag::HighRise => "high_rise",
        RegionTag::Overpass => "overpass",
        RegionTag::Canyon => "canyon",
        RegionTag::Tunnel => "tunnel",
        RegionTag::Indoor => "indoor",
        RegionTag::MagneticDisturbance => "magnetic_disturbance",
    }
}

/// Wire name of a vector geometry kind.
fn vector_kind_name(kind: VectorKind) -> &'static str {
    match kind {
        VectorKind::Polyline => "polyline",
        VectorKind::Polygon => "polygon",
    }
}

/// Tags of the meta block, for the inspector's record list.
pub(crate) fn tlv_tag_name(tag_value: u32) -> Option<&'static str> {
    Some(match tag_value {
        tag::MAP_INFO => "map_info",
        tag::LAYER_TABLE => "layer_table",
        tag::FEATURE_SCHEMA => "feature_schema",
        tag::WEIGHT_PRIOR => "weight_prior",
        tag::SLOPE_MODEL => "slope_model",
        tag::CONNECTOR_TABLE => "connector_table",
        tag::ZSTD_DICT => "zstd_dict",
        tag::PROVENANCE => "provenance",
        tag::MAGNETIC_FIELD => "magnetic_field",
        tag::PRM_SEEDS => "prm_seeds",
        tag::CHUNK_LAYOUT => "chunk_layout",
        tag::GLOBAL_STATS => "global_stats",
        tag::AGGREGATION_RULES => "aggregation_rules",
        tag::DERIVED_LAYERS => "derived_layers",
        _ => return None,
    })
}

/// Records of a meta block, as `(tag, name, byte length)` triples.
pub(crate) fn tlv_record_list(block: &TlvBlock) -> Vec<Value> {
    block
        .records()
        .iter()
        .map(|record| {
            json!({
                "tag": record.tag,
                "name": tlv_tag_name(record.tag),
                "len": record.value.len(),
            })
        })
        .collect()
}
