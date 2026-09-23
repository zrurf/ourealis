/*
 * Map geometry as a client needs it: layer ids, chunk addressing, decoding and
 * the quantisation contract.
 *
 * Everything here is pure so the node-side lanes can exercise it: the chunk
 * grid's addressing rules are the ones `crates/map-format/src/raster.rs` applies,
 * and a viewer that gets them wrong places a chunk in the wrong corner of the map
 * rather than failing visibly.
 */
import type { Aabb, ChunkPayload, LayerGrid, LayerInfo } from '@/api/types'
import { floatsFromBase64 } from './bytes'

/** Layer id of the elevation raster (map-format `LayerId::ELEVATION`). */
export const LAYER_ELEVATION = 0x0001

/** Layer id of the derived slope raster. */
export const LAYER_SLOPE = 0x0002

/** Layer id of the derived Euclidean distance transform. */
export const LAYER_EDT = 0x0003

/** First resistance feature layer id; features are `FEATURE_BASE + index`. */
export const FEATURE_BASE = 0x1000

/** Layer id of the hard-forbidden mask. */
export const LAYER_HARD_FORBIDDEN = 0x2001

/** Layer id of the direction constraint field. */
export const LAYER_DIRECTION = 0x2002

/** Layer id of the soft cost multiplier. */
export const LAYER_SOFT_MULTIPLIER = 0x2003

/** Layer id of the region mask. */
export const LAYER_REGIONS = 0x2501

/** Layer id of the PRM roadmap graph. */
export const LAYER_PRM_GRAPH = 0x3001

/** Layer id of the stored candidate path library. */
export const LAYER_KPATH_LIBRARY = 0x3002

/** Layer id of the vector shapes. */
export const LAYER_VECTORS = 0x3004

/** Name of a layer id, matching the service's own naming. */
export function layerName(layerId: number): string {
  switch (layerId) {
    case LAYER_ELEVATION:
      return 'elevation'
    case LAYER_SLOPE:
      return 'slope'
    case LAYER_EDT:
      return 'edt'
    case LAYER_HARD_FORBIDDEN:
      return 'hard_forbidden'
    case LAYER_DIRECTION:
      return 'direction'
    case LAYER_SOFT_MULTIPLIER:
      return 'soft_multiplier'
    case LAYER_REGIONS:
      return 'regions'
    case LAYER_PRM_GRAPH:
      return 'prm_graph'
    case LAYER_KPATH_LIBRARY:
      return 'kpath_library'
    case LAYER_VECTORS:
      return 'vectors'
    default:
      return layerId >= FEATURE_BASE && layerId < FEATURE_BASE + 256
        ? `feature_${layerId - FEATURE_BASE}`
        : `0x${layerId.toString(16).padStart(4, '0')}`
  }
}

/**
 * How a layer's samples are turned into colour.
 *
 * `material` is the one mapping whose colours come from the *map* rather than from this file: a
 * category dimension that declares materials (a surface type, say) is drawn as those materials,
 * and the viewer's `render/surfaces.ts` holds the table.
 */
export type ColorMapping = 'category' | 'material' | 'grey' | 'speed' | 'height' | 'cost' | 'flag'

/** Colour a mask layer draws where it is set, by layer id, in bytes. */
const FLAG_COLOURS: Readonly<Record<number, readonly [number, number, number]>> = {
  // Blocked ground: the one colour in the interface that already means "no".
  [LAYER_HARD_FORBIDDEN]: [176, 48, 44],
  [LAYER_EDT]: [34, 96, 140],
  [LAYER_DIRECTION]: [32, 122, 128],
  // A mask without a layer of its own is a restriction of some kind; amber says
  // "read the legend" rather than claiming to be forbidden or water.
  0: [183, 121, 31],
}

/** Colour a `flag` mapping draws for a layer: one strong colour where the mask is set. */
export function flagColour(layerId: number): readonly [number, number, number] {
  return FLAG_COLOURS[layerId] ?? FLAG_COLOURS[0] ?? [183, 121, 31]
}

/**
 * The mapping a layer is shown with by default.
 *
 * Semantic rather than mechanical: a restriction mask is drawn as a flag (0 stays
 * invisible, 1 is a warning colour), a feature raster keeps the discrete palette, and a
 * continuous field takes the ramp that says what it measures. The failure this avoids is
 * every layer arriving in the same grey — which is what made a map with nine layers
 * unreadable: none of them said what it was.
 */
export function defaultMapping(layer: LayerInfo): ColorMapping {
  if (layer.kind === 'bitmap') {
    return 'flag'
  }
  if (layer.kind === 'region' || layer.channels > 1) {
    return 'category'
  }
  switch (layer.layer_id) {
    case LAYER_SLOPE:
      return 'height'
    case LAYER_EDT:
    case LAYER_DIRECTION:
    case LAYER_SOFT_MULTIPLIER:
      return 'cost'
    default:
      // A resistance feature is a continuous dimension — a normalised cost or a preference
      // weight — so it takes a ramp. Painting it with the discrete palette collapsed every
      // value into a handful of buckets and made a gradient look like land cover.
      if (layer.layer_id >= FEATURE_BASE && layer.layer_id < FEATURE_BASE + 256) {
        return 'cost'
      }
      return layer.channels === 1 ? 'grey' : 'category'
  }
}

/** Options of {@link decodeChunk}. */
export interface DecodeOptions {
  /**
   * Whether the payload's samples are still quantised.
   *
   * The service dequantises at its boundary — `map-format`'s unpacker already
   * applied `real = raw * scale + bias` before the numbers were serialised — so
   * the default is false and the samples are real metres. Set this for a payload
   * that carries stored samples, where the contract still applies.
   */
  quantised?: boolean
}

/** One decoded chunk in real units, in `[y][x][channel]` order. */
export interface DecodedChunk {
  /** Layer the chunk belongs to. */
  layerId: number
  /** LOD level the chunk belongs to. */
  level: number
  /** Morton chunk id inside that level's chunk grid. */
  chunkId: number
  /** Chunk width in cells. */
  width: number
  /** Chunk height in cells. */
  height: number
  /** Channels per cell. */
  channels: number
  /** Samples in `[y][x][channel]` order, channel-continuous. */
  values: Float32Array
  /** Quantisation scale of the layer. */
  scale: number
  /** Quantisation bias of the layer. */
  bias: number
}

/** Applies the quantisation contract `real = raw * scale + bias`. */
export function dequantise(raw: number, scale: number, bias: number): number {
  return raw * scale + bias
}

/**
 * Decodes a chunk payload into real units.
 *
 * The base64 representation is a little-endian `f32` block; the JSON one is a
 * plain array. Both are flattened in `[y][x][channel]` order, so the index of
 * cell `(x, y)` channel `c` is `(y * width + x) * channels + c`.
 */
export function decodeChunk(payload: ChunkPayload, options: DecodeOptions = {}): DecodedChunk {
  const raw = rawSamples(payload)
  const values =
    options.quantised === true ? applyQuantisation(raw, payload.scale, payload.bias) : raw
  return {
    layerId: payload.layer_id,
    level: payload.level,
    chunkId: payload.chunk_id,
    width: payload.width,
    height: payload.height,
    channels: payload.channels,
    values,
    scale: payload.scale,
    bias: payload.bias,
  }
}

/** Reads the samples of a payload in either representation. */
function rawSamples(payload: ChunkPayload): Float32Array {
  if (typeof payload.data === 'string') {
    return floatsFromBase64(payload.data)
  }
  return Float32Array.from(payload.data)
}

/** Maps every sample through the quantisation contract. */
function applyQuantisation(values: Float32Array, scale: number, bias: number): Float32Array {
  const out = new Float32Array(values.length)
  for (let index = 0; index < values.length; index += 1) {
    out[index] = dequantise(values[index] ?? 0, scale, bias)
  }
  return out
}

/** Value of one cell of a decoded chunk, or `undefined` when the cell is outside it. */
export function sampleAt(
  chunk: DecodedChunk,
  x: number,
  y: number,
  channel = 0,
): number | undefined {
  if (x < 0 || y < 0 || x >= chunk.width || y >= chunk.height || channel >= chunk.channels) {
    return undefined
  }
  return chunk.values[(y * chunk.width + x) * chunk.channels + channel]
}

/**
 * Key a chunk is cached and de-duplicated by.
 *
 * The level is part of the key because the same Morton id names a different area
 * at every level, and the layer because every layer has its own chunk grid.
 */
export function chunkKey(layerId: number, level: number, chunkId: number): string {
  return `${layerId}/${level}/${chunkId}`
}

/** Decodes the 16-bit Morton code the file uses for chunk ids: even bits are x, odd bits y. */
export function mortonDecodeChunk(code: number): { ix: number; iy: number } {
  let ix = 0
  let iy = 0
  for (let bit = 0; bit < 16; bit += 1) {
    ix |= ((code >> (2 * bit)) & 1) << bit
    iy |= ((code >> (2 * bit + 1)) & 1) << bit
  }
  return { ix, iy }
}

/** Inverse of {@link mortonDecodeChunk}. */
export function mortonEncodeChunk(ix: number, iy: number): number {
  let code = 0
  for (let bit = 0; bit < 16; bit += 1) {
    code |= ((ix >> bit) & 1) << (2 * bit)
    code |= ((iy >> bit) & 1) << (2 * bit + 1)
  }
  return code
}

/** Chunk grid dimensions at one level: cells divided by the chunk side, rounded up. */
export function chunkDims(
  grid: LayerGrid,
  chunkSize: number,
  level: number,
): { x: number; y: number } {
  const dims = grid.level_dims[level]
  const side = Math.max(1, chunkSize)
  return {
    x: Math.ceil((dims?.[0] ?? 1) / side),
    y: Math.ceil((dims?.[1] ?? 1) / side),
  }
}

/** Cell size at one level, metres; falls back to the finest level for an unknown one. */
export function levelCellSize(grid: LayerGrid, level: number): number {
  return grid.level_res_m[level] ?? grid.level_res_m[0] ?? 1
}

/** Number of levels the layer reports. */
export function levelCount(grid: LayerGrid): number {
  return Math.max(1, Math.min(grid.level_res_m.length, grid.level_dims.length))
}

/** Area one chunk covers at a level, in the map's metre plane. */
export function chunkBounds(
  grid: LayerGrid,
  chunkSize: number,
  level: number,
  chunkId: number,
  origin: { x: number; y: number } = { x: 0, y: 0 },
): Aabb {
  const { ix, iy } = mortonDecodeChunk(chunkId)
  const span = levelCellSize(grid, level) * Math.max(1, chunkSize)
  const minX = origin.x + ix * span
  const minY = origin.y + iy * span
  return { min_x: minX, min_y: minY, max_x: minX + span, max_y: minY + span }
}

/** Centre of a chunk in the metre plane, which is where its label or marker goes. */
export function chunkCenter(
  grid: LayerGrid,
  chunkSize: number,
  level: number,
  chunkId: number,
  origin: { x: number; y: number } = { x: 0, y: 0 },
): { x: number; y: number } {
  const bounds = chunkBounds(grid, chunkSize, level, chunkId, origin)
  return {
    x: (bounds.min_x + bounds.max_x) / 2,
    y: (bounds.min_y + bounds.max_y) / 2,
  }
}

/**
 * Finest level worth drawing at a given ground resolution.
 *
 * A level's cells are `level_res_m` metres across, so a level whose cells are
 * smaller than a screen pixel costs vertices without showing detail. The coarsest
 * level satisfying that is chosen, and the level count clamps the answer to the
 * levels the layer actually stores.
 */
export function selectLevel(
  grid: LayerGrid,
  metresPerPixel: number,
  options: { minLevel?: number; maxLevel?: number } = {},
): number {
  const levels = levelCount(grid)
  const minLevel = Math.max(0, options.minLevel ?? 0)
  const maxLevel = Math.min(levels - 1, options.maxLevel ?? levels - 1)
  const target = Math.max(1e-6, metresPerPixel)
  // The coarsest level whose cells are not smaller than a screen pixel: scanning
  // downwards finds it, and a finer level would only add vertices. When even the
  // finest level's cells are larger than a pixel, no level is fine enough and the
  // finest stored one is the answer.
  let chosen = minLevel
  for (let level = maxLevel; level >= minLevel; level -= 1) {
    if (levelCellSize(grid, level) <= target) {
      chosen = level
      break
    }
  }
  return Math.min(maxLevel, Math.max(minLevel, chosen))
}

/**
 * Level a layer should be read at to be drawn on a surface of another level.
 *
 * Layers do not all store every level: the derived fields of a synthetic map live at the
 * finest one, while the elevation carries three. A drape that only looked at the surface's
 * own level therefore drew *nothing* for such a layer — a switch that changed nothing on
 * screen, which is the worst way to tell a reader a layer is empty.
 *
 * The answer is the finest level the layer stores that is no finer than the surface, so a
 * drape is built from real samples rather than by inventing detail; when the layer is only
 * finer than the surface, its finest stored level is used and the samples are pooled per
 * drawn cell.
 */
export function drapeSourceLevel(grid: LayerGrid, level: number): number | null {
  const stored: number[] = []
  for (let candidate = 0; candidate < levelCount(grid); candidate += 1) {
    if ((grid.chunks[candidate]?.length ?? 0) > 0) {
      stored.push(candidate)
    }
  }
  if (stored.length === 0) {
    return null
  }
  const target = levelCellSize(grid, level)
  let best: number | null = null
  for (const candidate of stored) {
    if (levelCellSize(grid, candidate) <= target) {
      if (best === null || levelCellSize(grid, candidate) > levelCellSize(grid, best)) {
        best = candidate
      }
    }
  }
  if (best !== null) {
    return best
  }
  let finest = stored[0] ?? null
  for (const candidate of stored) {
    if (finest === null || levelCellSize(grid, candidate) < levelCellSize(grid, finest)) {
      finest = candidate
    }
  }
  return finest
}

/**
 * Spacing of a ground grid drawn under a map, metres.
 *
 * The step is one, two or five times a power of ten, chosen so an extent gets
 * roughly `targetLines` lines per axis: a fixed step would draw a single line
 * across a large map and thousands across a small one.
 */
export function gridSpacing(width: number, depth: number, targetLines = 40): number {
  const span = Math.max(width, depth, 1)
  const raw = span / Math.max(1, targetLines)
  const magnitude = 10 ** Math.floor(Math.log10(raw))
  const normalized = raw / magnitude
  const step = normalized >= 5 ? 5 : normalized >= 2 ? 2 : 1
  return Math.max(1, step * magnitude)
}

/**
 * Cells of a chunk that lie inside the map.
 *
 * A chunk is stored padded to its full side length — the format trades a few padding
 * cells for simple codec arithmetic — so a chunk at the map's edge holds real data for
 * the first `width` columns and zeros for the rest. Those zeros are not terrain: drawn
 * as ground they extend the map past its own extent and, because the padding decodes to
 * the layer's bias rather than to a plausible height, they leave a step along the chunk
 * boundary that catches the light as a bright line.
 */
export function inMapDims(
  grid: LayerGrid,
  chunkSize: number,
  level: number,
  chunkId: number,
): { width: number; height: number } {
  const dims = grid.level_dims[level]
  const side = Math.max(1, chunkSize)
  const { ix, iy } = mortonDecodeChunk(chunkId)
  return {
    width: Math.max(0, Math.min(side, (dims?.[0] ?? 0) - ix * side)),
    height: Math.max(0, Math.min(side, (dims?.[1] ?? 0) - iy * side)),
  }
}

/**
 * A chunk limited to the cells the map actually covers.
 *
 * The samples are copied rather than viewed: every consumer indexes them with
 * `chunk.width`, so a view that kept the padded stride would have to carry the stride
 * everywhere it is read.
 */
export function cropChunk(chunk: DecodedChunk, width: number, height: number): DecodedChunk {
  const w = Math.max(0, Math.min(width, chunk.width))
  const h = Math.max(0, Math.min(height, chunk.height))
  if (w === chunk.width && h === chunk.height) {
    return chunk
  }
  if (w === 0 || h === 0) {
    return { ...chunk, width: 0, height: 0, values: new Float32Array(0) }
  }
  const values = new Float32Array(w * h * chunk.channels)
  for (let row = 0; row < h; row += 1) {
    const from = row * chunk.width * chunk.channels
    values.set(chunk.values.subarray(from, from + w * chunk.channels), row * w * chunk.channels)
  }
  return { ...chunk, width: w, height: h, values }
}

/** The chunk cropped to the map's extent, given the grid it was stored against. */
export function cropChunkToMap(
  chunk: DecodedChunk,
  grid: LayerGrid,
  chunkSize: number,
): DecodedChunk {
  const dims = inMapDims(grid, chunkSize, chunk.level, chunk.chunkId)
  return cropChunk(chunk, dims.width, dims.height)
}

/**
 * Origin of a map's local plane.
 *
 * A map's own coordinates start at `summary.bounds`, which is not necessarily `(0, 0)`:
 * an imported image keeps the plane its producer used. Every conversion between a cell
 * index and a metre position has to subtract it, or a map with a shifted origin draws
 * its geometry in one place and reports its cells in another.
 */
export interface PlaneOrigin {
  /** Metre coordinate of cell column zero. */
  x: number
  /** Metre coordinate of cell row zero. */
  y: number
}

/** Chunk ids of one level that intersect an area, in the order the grid lists them. */
export function chunksInBounds(
  grid: LayerGrid,
  chunkSize: number,
  level: number,
  area: Aabb,
  origin: PlaneOrigin = { x: 0, y: 0 },
): number[] {
  const stored = grid.chunks[level] ?? []
  return stored.filter((chunkId) => {
    const bounds = chunkBounds(grid, chunkSize, level, chunkId, origin)
    return (
      bounds.max_x > area.min_x &&
      bounds.min_x < area.max_x &&
      bounds.max_y > area.min_y &&
      bounds.min_y < area.max_y
    )
  })
}
