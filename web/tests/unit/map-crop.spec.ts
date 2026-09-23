/*
 * The chunk crop, which is what keeps the stored padding out of the picture.
 *
 * A chunk is padded to its full side length, so a chunk at the map's edge carries real
 * cells followed by zeros. Drawn as terrain those zeros put the surface past the map's
 * own extent and leave a step along the boundary — a bright line under the sun — so the
 * crop is geometry, not cosmetics, and it is worth a test of its own.
 */
import { expect, test } from '@playwright/test'
import {
  cropChunk,
  cropChunkToMap,
  inMapDims,
  mortonEncodeChunk,
  type DecodedChunk,
} from '../../src/types/map'
import type { LayerGrid } from '../../src/api/types'

/** Morton id of a chunk in the test grid, named so the cases read as coordinates. */
function chunkAt(ix: number, iy: number): number {
  return mortonEncodeChunk(ix, iy)
}

/** A grid of `dims` cells at one level, with one chunk per 4x4 cells. */
function gridOf(dims: [number, number], stored: number[] = []): LayerGrid {
  return {
    map_id: 'm',
    layer_id: 1,
    chunk_size: 4,
    level_res_m: [1],
    level_dims: [dims],
    chunks: [stored],
  } as unknown as LayerGrid
}

/** A chunk that is `width` x `height` cells, one channel, values 1..n. */
function chunkOf(width: number, height: number, chunkId: number): DecodedChunk {
  const values = new Float32Array(width * height)
  for (let index = 0; index < values.length; index += 1) {
    values[index] = index + 1
  }
  return {
    layerId: 1,
    level: 0,
    chunkId,
    width,
    height,
    channels: 1,
    values,
    scale: 1,
    bias: 0,
  }
}

test('the in-map size of a chunk is its part inside the map', () => {
  // A 10 x 6 cell map with 4-cell chunks: 3 chunks across, 2 down, and the last one in
  // each direction is partial.
  const grid = gridOf([10, 6])
  expect(inMapDims(grid, 4, 0, chunkAt(0, 0))).toEqual({ width: 4, height: 4 })
  expect(inMapDims(grid, 4, 0, chunkAt(2, 0))).toEqual({ width: 2, height: 4 })
  expect(inMapDims(grid, 4, 0, chunkAt(0, 1))).toEqual({ width: 4, height: 2 })
  expect(inMapDims(grid, 4, 0, chunkAt(2, 1))).toEqual({ width: 2, height: 2 })
  // A chunk outside the map on one axis reports nothing on that axis; a caller treats
  // either zero as "nothing to draw" rather than reasoning about which axis it was.
  expect(inMapDims(grid, 4, 0, chunkAt(3, 0))).toEqual({ width: 0, height: 4 })
  expect(inMapDims(grid, 4, 0, chunkAt(3, 2))).toEqual({ width: 0, height: 0 })
})

test('cropping keeps the cells that are in the map and drops the padding', () => {
  const chunk = chunkOf(4, 4, 0)
  const cropped = cropChunk(chunk, 2, 3)
  expect(cropped.width).toBe(2)
  expect(cropped.height).toBe(3)
  // The first three rows of the first two columns, in the chunk's own order.
  expect(Array.from(cropped.values)).toEqual([1, 2, 5, 6, 9, 10])
  // The original is untouched: a caller may still need the whole chunk.
  expect(chunk.width).toBe(4)
  expect(chunk.values.length).toBe(16)
})

test('a fully covered chunk is returned as it is', () => {
  const chunk = chunkOf(4, 4, 0)
  expect(cropChunk(chunk, 4, 4)).toBe(chunk)
  expect(cropChunk(chunk, 8, 8)).toBe(chunk)
})

test('cropping to nothing yields an empty chunk rather than an error', () => {
  const cropped = cropChunk(chunkOf(4, 4, 0), 0, 4)
  expect(cropped.width).toBe(0)
  expect(cropped.values.length).toBe(0)
})

test('cropping through the grid agrees with the in-map size', () => {
  const grid = gridOf([10, 6], [mortonEncodeChunk(2, 0)])
  const chunk = chunkOf(4, 4, mortonEncodeChunk(2, 0))
  const cropped = cropChunkToMap(chunk, grid, 4)
  expect({ width: cropped.width, height: cropped.height }).toEqual({ width: 2, height: 4 })
})
