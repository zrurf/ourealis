//! Map library, metadata and chunk resources.

use serde::{Deserialize, Serialize};

use super::AabbDto;

/// One map in the library.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MapSummary {
    /// Server-assigned identifier, stable while the entry lives.
    pub id: String,
    /// Name from the map's own information record, or the name given at import.
    pub name: String,
    /// Map extent in the local metre plane.
    pub bounds: AabbDto,
    /// Finest resolution, metres per cell.
    pub base_res_m: f64,
    /// Chunk side length in cells.
    pub chunk_size: u32,
    /// Declared LOD level count.
    pub lod_count: u32,
    /// Number of resistance feature dimensions.
    pub feature_dim: u32,
    /// Number of registered layers.
    pub layer_count: u32,
    /// How the map entered the library: `import`, `synthetic` or `inline`.
    pub source: String,
    /// Creation time of the library entry, RFC 3339.
    pub created_at: String,
    /// Size of the OMF image in bytes.
    pub size_bytes: u64,
}

/// One registered layer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LayerInfo {
    /// Layer id as it appears in the file, for example `0x1002`.
    pub layer_id: u16,
    /// Layer kind: `raster`, `bitmap`, `graph`, `region` or similar.
    pub kind: String,
    /// Channels per cell.
    pub channels: u8,
    /// Element type of the stored payload.
    pub dtype: String,
    /// Codec id.
    pub codec: u8,
    /// Quantisation contract: `real = raw * scale + bias`.
    pub scale: f64,
    /// Quantisation bias.
    pub bias: f64,
    /// Whether the layer stores only exceptions.
    pub sparse: bool,
    /// LOD levels present in the file, ascending.
    pub levels: Vec<u8>,
}

/// Everything the inspector shows about a map.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MapMetadata {
    /// Library entry.
    pub summary: MapSummary,
    /// Header fields.
    pub header: HeaderDto,
    /// Footer summary.
    pub footer: FooterDto,
    /// Map information record, when present.
    pub map_info: Option<serde_json::Value>,
    /// Resistance feature schema, when present.
    pub feature_schema: Option<serde_json::Value>,
    /// Weight priors per motion mode, when present.
    pub weight_prior: Option<serde_json::Value>,
    /// Slope model in effect.
    pub slope_model: Option<serde_json::Value>,
    /// Magnetic field declaration, when present.
    pub magnetic_field: Option<serde_json::Value>,
    /// Global statistics, when present.
    pub global_stats: Option<serde_json::Value>,
    /// Registered layers.
    pub layers: Vec<LayerInfo>,
    /// Optional sections and their sizes.
    pub sections: SectionCounts,
    /// Skeleton node count.
    pub skeleton_nodes: usize,
    /// Derived layers and whether their fingerprints still check out.
    pub derived: Vec<DerivedLayerDto>,
}

/// Header fields of an OMF image.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HeaderDto {
    /// Semantic major version.
    pub version_major: u16,
    /// Semantic minor version.
    pub version_minor: u16,
    /// Flag bits as a hex string.
    pub flags: String,
    /// Reference longitude, degrees.
    pub ref_lon_deg: f64,
    /// Reference latitude, degrees.
    pub ref_lat_deg: f64,
    /// EPSG code, 0 for a pure local plane.
    pub epsg: u32,
    /// Map extent.
    pub bounds: AabbDto,
    /// Finest resolution, centimetres per cell.
    pub base_res_cm: u16,
    /// Chunk side length in cells.
    pub chunk_size: u16,
    /// Declared LOD level count.
    pub lod_count: u8,
    /// Feature dimension count.
    pub feature_dim: u8,
    /// Registered layer count.
    pub layer_count: u16,
    /// Metadata section offset.
    pub meta_offset: u64,
    /// Metadata section length.
    pub meta_len: u32,
    /// Directory offset.
    pub dir_offset: u64,
    /// Directory length.
    pub dir_len: u32,
    /// Extension area offset, 0 when absent.
    pub ext_meta_offset: u64,
    /// Extension area length.
    pub ext_meta_len: u32,
}

/// Footer summary of an OMF image.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FooterDto {
    /// Semantic major version.
    pub version_major: u16,
    /// Semantic minor version.
    pub version_minor: u16,
    /// Directory record count.
    pub dir_record_count: u32,
    /// Chunk count.
    pub chunk_count: u32,
    /// Skeleton node count.
    pub node_count: u32,
    /// Registered layer count.
    pub layer_count: u32,
    /// Total file length in bytes.
    pub file_len: u64,
    /// Whole-file hash, lowercase hex.
    pub file_hash: String,
}

/// Which optional sections a map carries.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SectionCounts {
    /// Connector entries, when the file carries connectors.
    pub connectors: Option<usize>,
    /// Region features, when the file carries regions.
    pub regions: Option<usize>,
    /// Vector shapes, when the file carries vectors.
    pub vectors: Option<usize>,
    /// Roadmap batches, when the file carries a roadmap.
    pub roadmap_batches: Option<usize>,
    /// Candidate library entries, when the file carries one.
    pub library_paths: Option<usize>,
}

/// Fingerprint state of one derived layer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DerivedLayerDto {
    /// Layer id.
    pub layer_id: u16,
    /// Layer name in human-readable form.
    pub name: String,
    /// Result of comparing the stored fingerprint against the sources: `valid`,
    /// `stale` or `absent`.
    pub status: String,
    /// Reason a stale fingerprint is stale.
    pub reason: Option<String>,
}

/// Geometry of one raster layer plus the chunks it stores.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LayerGridDto {
    /// Layer id.
    pub layer_id: u16,
    /// Cell size at each level, metres, indexed by level.
    pub level_res_m: Vec<f64>,
    /// Cell dimensions at each level as `(x, y)` pairs.
    pub level_dims: Vec<[u32; 2]>,
    /// Chunk grid dimensions at level 0.
    pub chunk_dim: [u32; 2],
    /// Chunk ids stored at each level, indexed by level.
    pub chunks: Vec<Vec<u32>>,
}

/// One raster chunk.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChunkData {
    /// Layer id.
    pub layer_id: u16,
    /// Level the chunk belongs to.
    pub level: u8,
    /// Chunk id.
    pub chunk_id: u32,
    /// Chunk width in cells.
    pub width: u32,
    /// Chunk height in cells.
    pub height: u32,
    /// Channels per cell.
    pub channels: u8,
    /// Element type of the stored payload.
    pub dtype: String,
    /// Quantisation scale, so a client can reconstruct the raw value.
    pub scale: f64,
    /// Quantisation bias.
    pub bias: f64,
    /// Dequantised samples in `[y][x][channel]` order.
    pub data: Vec<f32>,
}

/// The quadtree skeleton.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SkeletonDto {
    /// Nodes, in Morton order.
    pub nodes: Vec<SkeletonNodeDto>,
}

/// One skeleton node.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SkeletonNodeDto {
    /// Linear-quadtree key.
    pub key: u64,
    /// Depth of the node.
    pub depth: u8,
    /// Node extent.
    pub bounds: AabbDto,
    /// Whether the node is a leaf.
    pub leaf: bool,
    /// Whether the node carries an aggregate maximum.
    pub has_aggregate_max: bool,
    /// Whether the node asks to be refined to the fine grid.
    pub drill_hint: bool,
    /// Whether the block looks impassable.
    pub suspect_forbidden: bool,
    /// Whether the block carries a direction constraint and must not be
    /// aggregated.
    pub direction_constrained: bool,
    /// Quantised aggregate mean, when present.
    pub aggregate_mean: Option<u16>,
    /// Quantised aggregate maximum, when present.
    pub aggregate_max: Option<u16>,
}

/// A section passed through as JSON.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SectionJson {
    /// Section name.
    pub section: String,
    /// Section content.
    pub json: serde_json::Value,
}

/// A page of items.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Page<T> {
    /// Items of this page.
    pub items: Vec<T>,
    /// Total number of items available.
    pub total: usize,
    /// Offset this page started at.
    pub offset: usize,
}

impl<T> Page<T> {
    /// Wraps a page of items.
    pub fn new(items: Vec<T>, total: usize, offset: usize) -> Self {
        Self {
            items,
            total,
            offset,
        }
    }
}
