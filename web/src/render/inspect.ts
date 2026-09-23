/*
 * What one cell of the map holds.
 *
 * A pick gives a position; this turns it into everything the loaded data can say about
 * the cell there: where it is, which chunk it belongs to, which cell of that chunk it
 * is, and what every declared layer carries for it. Kept pure — no Babylon, no Vue — so
 * the panel's numbers can be tested against a decoded chunk without a canvas.
 *
 * The rule the report follows is that it never invents: a layer whose chunk is not
 * loaded is reported as not loaded rather than as a zero. A zero is a value, and a
 * viewer that shows one where there is no data is worse than one that shows nothing.
 */
import {
  LAYER_DIRECTION,
  LAYER_EDT,
  LAYER_ELEVATION,
  LAYER_HARD_FORBIDDEN,
  LAYER_SLOPE,
  LAYER_SOFT_MULTIPLIER,
  chunkKey,
  inMapDims,
  layerName,
  levelCellSize,
  mortonDecodeChunk,
  sampleAt,
  type DecodedChunk,
  type PlaneOrigin,
} from '@/types/map'
import type { LayerGrid, LayerInfo, MapMetadata } from '@/api/types'

/** Layer kinds that hold cells; everything else is one opaque payload. */
const CHUNKED_KINDS = new Set(['raster', 'bitmap'])

/** What a cell *is*, in the map's own terms, for the layers that have a known meaning. */
export type CellMeaning =
  | 'elevation'
  | 'slope'
  | 'distance'
  | 'cost'
  | 'blocked'
  | 'passable'
  | 'direction'
  | 'surface'
  | 'feature'
  | 'section'
  | 'unknown'

/** One layer's reading for the picked cell. */
export interface CellLayerReport {
  /** Layer id as the file names it. */
  layerId: number
  /** Canonical name of the layer, from its id. */
  name: string
  /** Kind as the file names it (`raster`, `bitmap`, `region`, …). */
  kind: string
  /** Channels per cell. */
  channels: number
  /** One value per channel, in the file's order; empty when the cell was not read. */
  values: number[]
  /** The direction constraint decoded from the layer's packed value. */
  direction?: { angleDeg: number; strength: number }
  /** For the hard mask: whether the cell is blocked. */
  blocked?: boolean
  /**
   * What the value *means*, so the panel can say "blocked" rather than "1".
   *
   * The panel is what a reader consults when a colour on the map surprises them, and a number
   * does not explain a colour; the meaning is the answer to the question they came with.
   */
  meaning: CellMeaning
  /** Material the value names, when the layer carries a material table (a catalog key). */
  materialKey?: string
  /** Whether the layer's chunk for this cell is loaded. */
  loaded: boolean
  /** Whether the layer is a cell layer at all; a section layer has no cells. */
  chunked: boolean
  /** Quantisation the layer's stored samples used; the service has already undone it. */
  quantisation: { scale: number; bias: number }
  /** Extent of the layer's samples in the loaded chunks, for a sense of the value's scale. */
  loadedRange: { min: number; max: number } | null
}

/** Everything known about one picked cell. */
export interface CellReport {
  /** Position in the map's local metre plane. */
  position: { x: number; y: number }
  /** Elevation in metres, when the elevation layer is loaded at this level. */
  elevation: number | null
  /** Global cell index from the map's own origin. */
  cell: { i: number; j: number }
  /** Metre extent of the cell the pick fell in. */
  cellBounds: { min_x: number; min_y: number; max_x: number; max_y: number }
  /** Cells per axis at this level, so the reader can see where the cell sits in the map. */
  levelDims: { width: number; height: number }
  /** Whether the cell is inside the map proper; a padded cell holds no data. */
  inMap: boolean
  /** Level the cell was read at, and the cell size there. */
  level: { level: number; cellSizeM: number }
  /** Chunk the cell falls in. */
  chunk: {
    /** Morton id of the chunk, or `null` when the layer stores no chunk there. */
    id: number | null
    /** Chunk coordinates in the level's chunk grid. */
    ix: number
    iy: number
    /** Cell index inside the chunk. */
    local: { x: number; y: number }
    /** Shape of the chunk as loaded, or `null` when none of the layers has it. */
    shape: { width: number; height: number; channels: number } | null
    /** Cells of the chunk that lie inside the map; the rest is storage padding. */
    inMap: { width: number; height: number } | null
  }
  /** One entry per declared layer, in the order the map declares them. */
  layers: CellLayerReport[]
}

/** Input of {@link cellReport}. */
export interface CellReportInput {
  /** Map metadata, for the layer list. */
  metadata: MapMetadata
  /** Elevation layer grid, which defines the chunk geometry. */
  grid: LayerGrid | null
  /** Chunk side length in cells. */
  chunkSize: number
  /** Level the viewer is drawing. */
  level: number
  /** Loaded chunks, keyed by `chunkKey`. */
  chunks: ReadonlyMap<string, DecodedChunk>
  /** Picked position in the metre plane. */
  point: { x: number; y: number }
}

/**
 * Reads one cell, or `null` when the pick is outside the layer's grid.
 *
 * Every layer is read at the same level as the elevation, so the numbers in the panel
 * belong to one grid rather than mixing a coarse elevation with fine features.
 */
export function cellReport(input: CellReportInput): CellReport | null {
  const { metadata, grid, chunkSize, level, chunks, point } = input
  if (grid === null) {
    return null
  }
  const cellSize = levelCellSize(grid, level)
  const dims = grid.level_dims[level]
  if (!(cellSize > 0) || dims === undefined) {
    return null
  }
  const origin = metadata.summary.bounds
  const i = Math.floor((point.x - origin.min_x) / cellSize)
  const j = Math.floor((point.y - origin.min_y) / cellSize)
  if (i < 0 || j < 0 || i >= dims[0] || j >= dims[1]) {
    return null
  }

  const side = Math.max(1, chunkSize)
  const ix = Math.floor(i / side)
  const iy = Math.floor(j / side)
  const local = { x: i - ix * side, y: j - iy * side }
  // The directory lists the chunks the level actually stores, so the id is a lookup
  // rather than a computation: a map whose extent does not fill its last chunk has no
  // entry for the missing one.
  const id =
    grid.chunks[level]?.find((candidate) => {
      const decoded = mortonDecodeChunk(candidate)
      return decoded.ix === ix && decoded.iy === iy
    }) ?? null

  const layers = metadata.layers.map((layer) => readLayer(layer, { level, chunks, id, local }))
  const elevation = layers.find((layer) => layer.layerId === LAYER_ELEVATION)
  return {
    position: { x: point.x, y: point.y },
    elevation: elevation?.values[0] ?? null,
    cell: { i, j },
    cellBounds: {
      min_x: origin.min_x + i * cellSize,
      min_y: origin.min_y + j * cellSize,
      max_x: origin.min_x + (i + 1) * cellSize,
      max_y: origin.min_y + (j + 1) * cellSize,
    },
    levelDims: { width: dims[0], height: dims[1] },
    inMap: i < dims[0] && j < dims[1],
    level: { level, cellSizeM: cellSize },
    chunk: {
      id,
      ix,
      iy,
      local,
      shape: shapeOf(id, level, chunks),
      inMap: id === null ? null : inMapDims(grid, chunkSize, level, id),
    },
    layers,
  }
}

/** The shape of a chunk, from whichever layer has it loaded. */
function shapeOf(
  id: number | null,
  level: number,
  chunks: ReadonlyMap<string, DecodedChunk>,
): { width: number; height: number; channels: number } | null {
  if (id === null) {
    return null
  }
  for (const chunk of chunks.values()) {
    if (chunk.level === level && chunk.chunkId === id) {
      return { width: chunk.width, height: chunk.height, channels: chunk.channels }
    }
  }
  return null
}

/** Reads one layer's value for a cell of a chunk. */
function readLayer(
  layer: LayerInfo,
  context: {
    level: number
    chunks: ReadonlyMap<string, DecodedChunk>
    id: number | null
    local: { x: number; y: number }
  },
): CellLayerReport {
  const chunked = CHUNKED_KINDS.has(layer.kind)
  const base: CellLayerReport = {
    layerId: layer.layer_id,
    name: layerName(layer.layer_id),
    kind: layer.kind,
    channels: layer.channels,
    values: [],
    loaded: false,
    chunked,
    meaning: meaningOf(layer.layer_id, layer.kind),
    quantisation: { scale: layer.scale, bias: layer.bias },
    loadedRange: null,
  }
  if (!chunked || context.id === null) {
    return base
  }
  const chunk = context.chunks.get(chunkKey(layer.layer_id, context.level, context.id))
  if (chunk === undefined) {
    return base
  }
  const values: number[] = []
  for (let channel = 0; channel < chunk.channels; channel += 1) {
    const value = sampleAt(chunk, context.local.x, context.local.y, channel)
    if (value !== undefined) {
      values.push(value)
    }
  }
  const report: CellLayerReport = {
    ...base,
    values,
    loaded: values.length > 0,
    loadedRange: values.length > 0 ? valueRange(values) : null,
  }
  const first = values[0]
  if (layer.layer_id === LAYER_DIRECTION && first !== undefined) {
    const packed = Math.trunc(first)
    if (packed > 0) {
      report.direction = { angleDeg: ((packed >> 8) / 256) * 360, strength: packed & 0xff }
    }
  }
  if (layer.layer_id === LAYER_HARD_FORBIDDEN && first !== undefined) {
    report.blocked = first > 0.5
  }
  return report
}

/**
 * What a layer's value means for the cell it was read at.
 *
 * The reader asked "what is this cell"; a colour and a number are not an answer, so the layers
 * whose meaning the map format defines get a word for it. A layer the format does not define
 * reads as `unknown`, which is honest rather than inventing a meaning from its id alone.
 */
export function meaningOf(layerId: number, kind: string): CellMeaning {
  if (kind === 'region' || kind === 'vector' || kind === 'graph') {
    return 'section'
  }
  switch (layerId) {
    case LAYER_ELEVATION:
      return 'elevation'
    case LAYER_SLOPE:
      return 'slope'
    case LAYER_EDT:
      return 'distance'
    case LAYER_HARD_FORBIDDEN:
      return 'blocked'
    case LAYER_DIRECTION:
      return 'direction'
    case LAYER_SOFT_MULTIPLIER:
      return 'cost'
    default:
      return isFeatureLayer(layerId) ? 'feature' : 'unknown'
  }
}

/** Range of a set of samples, for a panel that has no wider statistics at hand. */
function valueRange(values: readonly number[]): { min: number; max: number } {
  let min = Number.POSITIVE_INFINITY
  let max = Number.NEGATIVE_INFINITY
  for (const value of values) {
    min = Math.min(min, value)
    max = Math.max(max, value)
  }
  return Number.isFinite(min) ? { min, max } : { min: 0, max: 0 }
}

/**
 * Reads a direction sample by world position, for the arrow field.
 *
 * Direction is the one layer whose meaning is a *vector*, so it is read by position rather than by
 * cell index: the arrows are laid out on their own grid.
 */
export function sampleDirection(
  chunks: ReadonlyMap<string, DecodedChunk>,
  grid: LayerGrid,
  chunkSize: number,
  level: number,
  point: { x: number; y: number },
  origin: PlaneOrigin = { x: 0, y: 0 },
): { angleDeg: number; strength: number } | null {
  const cellSize = levelCellSize(grid, level)
  const dims = grid.level_dims[level]
  if (!(cellSize > 0) || dims === undefined) {
    return null
  }
  const i = Math.floor((point.x - origin.x) / cellSize)
  const j = Math.floor((point.y - origin.y) / cellSize)
  if (i < 0 || j < 0 || i >= dims[0] || j >= dims[1]) {
    return null
  }
  const side = Math.max(1, chunkSize)
  const ix = Math.floor(i / side)
  const iy = Math.floor(j / side)
  const id = grid.chunks[level]?.find((candidate) => {
    const decoded = mortonDecodeChunk(candidate)
    return decoded.ix === ix && decoded.iy === iy
  })
  if (id === undefined) {
    return null
  }
  const chunk = chunks.get(chunkKey(LAYER_DIRECTION, level, id))
  if (chunk === undefined) {
    return null
  }
  const packed = Math.trunc(sampleAt(chunk, i - ix * side, j - iy * side, 0) ?? 0)
  const strength = packed & 0xff
  return strength > 0 ? { angleDeg: ((packed >> 8) / 256) * 360, strength } : null
}

/**
 * Elevation at a point, from the chunks already loaded, or `null` when unknown.
 *
 * Used to place a route handle on the ground rather than at the map's origin: a marker
 * drawn at a fixed height sits under the terrain, and its position on screen is then the
 * projection of an underground point — tens of pixels away from where the reader
 * clicked, which is exactly what makes a marker impossible to grab.
 */
export function elevationAt(
  chunks: ReadonlyMap<string, DecodedChunk>,
  grid: LayerGrid,
  chunkSize: number,
  level: number,
  point: { x: number; y: number },
  origin: PlaneOrigin = { x: 0, y: 0 },
): number | null {
  const cellSize = levelCellSize(grid, level)
  const dims = grid.level_dims[level]
  if (!(cellSize > 0) || dims === undefined) {
    return null
  }
  const side = Math.max(1, chunkSize)
  const i = Math.floor((point.x - origin.x) / cellSize)
  const j = Math.floor((point.y - origin.y) / cellSize)
  if (i < 0 || j < 0) {
    return null
  }
  const ix = Math.floor(i / side)
  const iy = Math.floor(j / side)
  const id = grid.chunks[level]?.find((candidate) => {
    const decoded = mortonDecodeChunk(candidate)
    return decoded.ix === ix && decoded.iy === iy
  })
  if (id === undefined) {
    return null
  }
  const chunk = chunks.get(chunkKey(LAYER_ELEVATION, level, id))
  if (chunk === undefined) {
    return null
  }
  const value = sampleAt(chunk, i - ix * side, j - iy * side, 0)
  return value === undefined || !Number.isFinite(value) ? null : value
}

/**
 * A cached elevation lookup for many points.
 *
 * {@link elevationAt} resolves a chunk by scanning the level's chunk list on every
 * call, which is fine for a handful of handles and wasteful for an overlay of
 * thousands of vertices — a region outline, a roadmap or a candidate path asks for
 * heights thousands of times in one build. This resolves each cell's chunk once and
 * remembers the answer, so a build costs one pass over the points instead of a scan
 * per point.
 *
 * A point outside the loaded chunks reads as `null`, which callers treat as "the
 * ground plane": an annotation is never hidden because its chunk is late.
 */
export function createSurfaceSampler(
  chunks: ReadonlyMap<string, DecodedChunk>,
  grid: LayerGrid,
  chunkSize: number,
  level: number,
  origin: PlaneOrigin = { x: 0, y: 0 },
): (x: number, y: number) => number | null {
  const cellSize = levelCellSize(grid, level)
  const dims = grid.level_dims[level]
  const side = Math.max(1, chunkSize)
  if (!(cellSize > 0) || dims === undefined) {
    return () => null
  }
  // Chunk id per chunk-grid cell, so the Morton decode happens once per chunk rather
  // than once per sample.
  const ids = new Map<string, number | null>()
  const idOf = (ix: number, iy: number): number | null => {
    const cacheKey = `${ix}:${iy}`
    const cached = ids.get(cacheKey)
    if (cached !== undefined) {
      return cached
    }
    const found =
      grid.chunks[level]?.find((candidate) => {
        const decoded = mortonDecodeChunk(candidate)
        return decoded.ix === ix && decoded.iy === iy
      }) ?? null
    ids.set(cacheKey, found)
    return found
  }

  return (x: number, y: number): number | null => {
    const i = Math.floor((x - origin.x) / cellSize)
    const j = Math.floor((y - origin.y) / cellSize)
    if (i < 0 || j < 0 || i >= dims[0] || j >= dims[1]) {
      return null
    }
    const ix = Math.floor(i / side)
    const iy = Math.floor(j / side)
    const id = idOf(ix, iy)
    if (id === null) {
      return null
    }
    const chunk = chunks.get(chunkKey(LAYER_ELEVATION, level, id))
    if (chunk === undefined) {
      return null
    }
    const value = sampleAt(chunk, i - ix * side, j - iy * side, 0)
    return value === undefined || !Number.isFinite(value) ? null : value
  }
}

/** Whether a layer id names a resistance feature dimension. */
export function isFeatureLayer(layerId: number): boolean {
  return layerId >= 0x1000 && layerId < 0x1100
}
