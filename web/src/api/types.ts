/*
 * TypeScript mirror of the service DTOs.
 *
 * Field names, units and nullability follow `crates/service/src/api/dto` exactly,
 * because that layer is the single definition of every resource: lengths are
 * metres (`*_m`), angles radians (`*_rad`), durations seconds (`*_s`) and
 * timestamps RFC 3339 strings. Nothing here renames a field — a rename would make
 * a payload that validates against one side silently wrong on the other.
 */

/** A point or vector in the map's local metre plane. */
export interface Vec2 {
  /** Easting, metres. */
  x: number
  /** Northing, metres. */
  y: number
}

/** An axis-aligned bounding box in the local metre plane. */
export interface Aabb {
  /** Minimum easting, metres. */
  min_x: number
  /** Minimum northing, metres. */
  min_y: number
  /** Maximum easting, metres. */
  max_x: number
  /** Maximum northing, metres. */
  max_y: number
}

/** A page of items, as every list endpoint returns them. */
export interface Page<T> {
  /** Items of this page. */
  items: T[]
  /** Total number of items available. */
  total: number
  /** Offset this page started at. */
  offset: number
}

/** One map in the library. */
export interface MapSummary {
  /** Server-assigned identifier, stable while the entry lives. */
  id: string
  /** Name from the map's own information record, or the name given at import. */
  name: string
  /** Map extent in the local metre plane. */
  bounds: Aabb
  /** Finest resolution, metres per cell. */
  base_res_m: number
  /** Chunk side length in cells. */
  chunk_size: number
  /** Declared LOD level count. */
  lod_count: number
  /** Number of resistance feature dimensions. */
  feature_dim: number
  /** Number of registered layers. */
  layer_count: number
  /** How the map entered the library: `import`, `synthetic` or `inline`. */
  source: string
  /** Creation time of the library entry, RFC 3339. */
  created_at: string
  /** Size of the OMF image in bytes. */
  size_bytes: number
}

/** One registered layer. */
export interface LayerInfo {
  /** Layer id as it appears in the file, for example `0x1002`. */
  layer_id: number
  /** Layer kind: `raster`, `bitmap`, `graph`, `region` or similar. */
  kind: string
  /** Channels per cell. */
  channels: number
  /** Element type of the stored payload. */
  dtype: string
  /** Codec id. */
  codec: number
  /** Quantisation contract: `real = raw * scale + bias`. */
  scale: number
  /** Quantisation bias. */
  bias: number
  /** Whether the layer stores only exceptions. */
  sparse: boolean
  /** LOD levels present in the file, ascending. */
  levels: number[]
}

/** Header fields of an OMF image. */
export interface MapHeader {
  /** Semantic major version. */
  version_major: number
  /** Semantic minor version. */
  version_minor: number
  /** Flag bits as a hex string. */
  flags: string
  /** Reference longitude, degrees. */
  ref_lon_deg: number
  /** Reference latitude, degrees. */
  ref_lat_deg: number
  /** EPSG code, 0 for a pure local plane. */
  epsg: number
  /** Map extent. */
  bounds: Aabb
  /** Finest resolution, centimetres per cell. */
  base_res_cm: number
  /** Chunk side length in cells. */
  chunk_size: number
  /** Declared LOD level count. */
  lod_count: number
  /** Feature dimension count. */
  feature_dim: number
  /** Registered layer count. */
  layer_count: number
  /** Metadata section offset, bytes from the start of the file. */
  meta_offset: number
  /** Metadata section length, bytes. */
  meta_len: number
  /** Chunk directory offset, bytes. */
  dir_offset: number
  /** Chunk directory length, bytes. */
  dir_len: number
  /** Extension area offset, 0 when the file has none. */
  ext_meta_offset: number
  /** Extension area length, bytes. */
  ext_meta_len: number
}

/** Footer summary of an OMF image. */
export interface MapFooter {
  /** Semantic major version. */
  version_major: number
  /** Semantic minor version. */
  version_minor: number
  /** Directory record count. */
  dir_record_count: number
  /** Chunk count. */
  chunk_count: number
  /** Skeleton node count. */
  node_count: number
  /** Registered layer count. */
  layer_count: number
  /** Total file length in bytes. */
  file_len: number
  /** Whole-file hash, lowercase hex. */
  file_hash: string
}

/** Which optional sections a map carries. */
export interface SectionCounts {
  /** Connector entries, when the file carries connectors. */
  connectors?: number | null
  /** Region features, when the file carries regions. */
  regions?: number | null
  /** Vector shapes, when the file carries vectors. */
  vectors?: number | null
  /** Roadmap batches, when the file carries a roadmap. */
  roadmap_batches?: number | null
  /** Candidate library entries, when the file carries one. */
  library_paths?: number | null
}

/** Fingerprint state of one derived layer. */
export interface DerivedLayer {
  /** Layer id. */
  layer_id: number
  /** Layer name in human-readable form. */
  name: string
  /** `valid`, `stale` or `absent`. */
  status: string
  /** Reason a stale fingerprint is stale. */
  reason?: string | null
}

/** Everything the inspector shows about a map. */
export interface MapMetadata {
  /** Library entry. */
  summary: MapSummary
  /** Header fields. */
  header: MapHeader
  /** Footer summary. */
  footer: MapFooter
  /** Map information record, when present. */
  map_info?: unknown
  /** Resistance feature schema, when present. */
  feature_schema?: unknown
  /** Weight priors per motion mode, when present. */
  weight_prior?: unknown
  /** Slope model in effect. */
  slope_model?: unknown
  /** Magnetic field declaration, when present. */
  magnetic_field?: unknown
  /** Global statistics, when present. */
  global_stats?: GlobalStats | null
  /** Registered layers. */
  layers: LayerInfo[]
  /** Optional sections and their sizes. */
  sections: SectionCounts
  /** Skeleton node count. */
  skeleton_nodes: number
  /** Derived layers and whether their fingerprints still check out. */
  derived: DerivedLayer[]
}

/** One channel's whole-map statistics, as `GLOBAL_STATS` carries them. */
export interface ChannelStats {
  /** Layer the channel belongs to. */
  layer_id: number
  /** Channel index inside that layer. */
  channel: number
  /** Minimum real value over the map's own cells. */
  min: number
  /** Maximum real value over the map's own cells. */
  max: number
  /** Mean real value over the map's own cells. */
  mean: number
  /** Fraction of the map's cells the channel covers. */
  coverage: number
}

/** Whole-map statistics, used to give a ramp a range the whole surface shares. */
export interface GlobalStats {
  /** One entry per channel of every cell layer. */
  channels: ChannelStats[]
  /** Fraction of the map that is forbidden, per layer. */
  forbidden_ratio: Array<[number, number]>
}

/** Range of one layer's channel, or `null` when the map does not declare one. */
export function channelRange(
  stats: GlobalStats | null | undefined,
  layerId: number,
  channel = 0,
): { min: number; max: number } | null {
  const entry = stats?.channels?.find(
    (candidate) => candidate.layer_id === layerId && candidate.channel === channel,
  )
  if (entry === undefined || !(entry.max > entry.min)) {
    return null
  }
  return { min: entry.min, max: entry.max }
}

/** Cell and chunk geometry of one raster layer. */
export interface LayerGrid {
  /** Layer id. */
  layer_id: number
  /** Cell size at each level, metres, indexed by level. */
  level_res_m: number[]
  /** Cell dimensions at each level as `(x, y)` pairs. */
  level_dims: Array<[number, number]>
  /** Chunk grid dimensions at level 0. */
  chunk_dim: [number, number]
  /** Chunk ids stored at each level, indexed by level. */
  chunks: number[][]
}

/**
 * One raster chunk as it arrives from the service.
 *
 * With the default representation `data` is a JSON array in `[y][x][channel]`
 * order; with `?format=base64` it is a base64 block of little-endian `f32`
 * values in that same order.
 */
export interface ChunkPayload {
  /** Layer id. */
  layer_id: number
  /** Level the chunk belongs to. */
  level: number
  /** Chunk id. */
  chunk_id: number
  /** Chunk width in cells. */
  width: number
  /** Chunk height in cells. */
  height: number
  /** Channels per cell. */
  channels: number
  /** Element type of the stored payload. */
  dtype: string
  /** Quantisation scale. */
  scale: number
  /** Quantisation bias. */
  bias: number
  /** Dequantised samples, or their base64 encoding. */
  data: number[] | string
  /** Present and `base64` on the encoded representation. */
  encoding?: string
  /** Present and `little_endian_f32` on the encoded representation. */
  byte_order?: string
}

/** One skeleton node. */
export interface SkeletonNode {
  /** Linear-quadtree key. */
  key: number
  /** Depth of the node. */
  depth: number
  /** Node extent. */
  bounds: Aabb
  /** Whether the node is a leaf. */
  leaf: boolean
  /** Whether the node carries an aggregate maximum. */
  has_aggregate_max: boolean
  /** Whether the node asks to be refined to the fine grid. */
  drill_hint: boolean
  /** Whether the block looks impassable. */
  suspect_forbidden: boolean
  /** Whether the block carries a direction constraint. */
  direction_constrained: boolean
  /** Quantised aggregate mean, when present. */
  aggregate_mean?: number | null
  /** Quantised aggregate maximum, when present. */
  aggregate_max?: number | null
}

/** The quadtree skeleton. */
export interface Skeleton {
  /** Nodes, in Morton order. */
  nodes: SkeletonNode[]
}

/**
 * A section passed through as JSON.
 *
 * The optional sections whose Rust types are not `Serialize` — regions,
 * connectors, vectors, the PRM graph — are written out by the service with
 * map-format's own field names and handed over here without a second schema.
 * `json` is `null` when the map carries no such section.
 */
export interface SectionJson<T = unknown> {
  /** Section name. */
  section: string
  /** Section content, or `null` when the map does not carry the section. */
  json: T | null
}
