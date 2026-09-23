/*
 * Reading cells by global index, from the chunks a viewer has loaded.
 *
 * The renderer builds one mesh per chunk, but a chunk's mesh does not stop at the chunk's
 * own cells: the surface between two neighbouring chunks belongs to both of them, and a
 * mesh that ignored that left a one-cell gap along every chunk boundary — the seam that
 * made a large map read as tiles that had slid apart. Filling it needs the neighbouring
 * cell's value, which lives in another chunk, so the renderer asks by *global cell index*
 * and this is what answers.
 *
 * Pure: chunks in, numbers out. A missing chunk reads as `null` rather than as zero, so a
 * mesh can tell "the ground here is zero" from "the ground here is not known yet" — the
 * difference between a flat spot and a hole.
 */
import {
  chunkKey,
  levelCellSize,
  mortonDecodeChunk,
  type DecodedChunk,
  type PlaneOrigin,
} from '@/types/map'
import type { LayerGrid } from '@/api/types'

/** Value of one cell by global index, or `null` where no loaded chunk holds it. */
export type CellSampler = (i: number, j: number) => number | null

/** Input of {@link createCellSampler}. */
export interface CellSamplerInput {
  /** Loaded chunks, keyed by `chunkKey`. */
  chunks: ReadonlyMap<string, DecodedChunk>
  /** Cell grid of the layer being read; its level dimensions bound the sampling. */
  grid: LayerGrid
  /** Chunk side length in cells. */
  chunkSize: number
  /** Level the viewer is drawing; the indices passed to the sampler belong to it. */
  level: number
  /** Layer the values come from. */
  layerId: number
  /** Channel inside the cell. */
  channel?: number
  /**
   * Level the layer's own cells live at, when it differs from the drawn one.
   *
   * A layer stored only at a finer level than the surface is read by pooling: the cell
   * asked for is mapped onto the layer's grid, so a drape follows the layer's data instead
   * of its absence.
   */
  sourceLevel?: number
}

/**
 * Builds a reader for one layer's cells at one level.
 *
 * The chunk grid of a level is a short list, so the chunk that holds a cell is resolved
 * once per chunk-grid cell and remembered; a mesh asks for tens of thousands of cells and
 * must not scan the list each time. The indices passed in belong to the *drawn* level and
 * are mapped onto the stored one, so the same reader serves a layer that lives at another
 * level than the surface it is painted on.
 */
export function createCellSampler(input: CellSamplerInput): CellSampler {
  const { chunks, grid, chunkSize, level, layerId } = input
  const source = input.sourceLevel ?? level
  const channel = input.channel ?? 0
  const dims = grid.level_dims[source]
  const side = Math.max(1, chunkSize)
  // Stored cells per drawn cell: two when the layer is finer than the surface, one half
  // when it is coarser. A finer layer is read by taking the stored cell that contains the
  // drawn one, which is what a drape of pooled samples looks like.
  const scale = levelCellSize(grid, level) / Math.max(1e-9, levelCellSize(grid, source))
  if (dims === undefined) {
    return () => null
  }
  const ids = new Map<string, number | null>()
  const idOf = (ix: number, iy: number): number | null => {
    const key = `${ix}:${iy}`
    const cached = ids.get(key)
    if (cached !== undefined) {
      return cached
    }
    const found =
      grid.chunks[source]?.find((candidate) => {
        const decoded = mortonDecodeChunk(candidate)
        return decoded.ix === ix && decoded.iy === iy
      }) ?? null
    ids.set(key, found)
    return found
  }
  return (i: number, j: number): number | null => {
    if (!Number.isFinite(i) || !Number.isFinite(j) || i < 0 || j < 0) {
      return null
    }
    const column = Math.floor(i * scale)
    const row = Math.floor(j * scale)
    // The bounds are the *stored* level's: a drawn index outside them has no sample.
    if (column >= dims[0] || row >= dims[1]) {
      return null
    }
    const ix = Math.floor(column / side)
    const iy = Math.floor(row / side)
    const id = idOf(ix, iy)
    if (id === null) {
      return null
    }
    const chunk = chunks.get(chunkKey(layerId, source, id))
    if (chunk === undefined) {
      return null
    }
    const local = row - iy * side
    const localColumn = column - ix * side
    if (localColumn >= chunk.width || local >= chunk.height || channel >= chunk.channels) {
      return null
    }
    const value = chunk.values[(local * chunk.width + localColumn) * chunk.channels + channel]
    return value === undefined || !Number.isFinite(value) ? null : value
  }
}

/** A sampler that knows nothing; the fallback for a caller without chunks. */
export const NO_CELLS: CellSampler = () => null

/** Global cell indices of a world position, from the map's own origin. */
export function cellAt(
  point: { x: number; y: number },
  cellSize: number,
  origin: PlaneOrigin,
): { i: number; j: number } {
  return {
    i: Math.floor((point.x - origin.x) / cellSize),
    j: Math.floor((point.y - origin.y) / cellSize),
  }
}
