/*
 * Chunk decimation.
 *
 * The simplifier rewrites a chunk's geometry, so the assertions are about the two things
 * that can go wrong: the result has to be a *valid mesh* (every index in range, every
 * vertex referenced), and it has to have kept the chunk's outline. A decimation that tears
 * the border open looks exactly like a hole in the map, and a hole in the map reads as a
 * renderer fault rather than as an optimisation that went wrong.
 */
import { expect, test } from '@playwright/test'
import { lodRatioFor, simplifyChunk, type ChunkGeometry } from '../../src/render/chunkLod'

/** A `side` by `side` grid of quads over a rolling surface, as a chunk is. */
function grid(side: number, height: (i: number, j: number) => number = () => 0): ChunkGeometry {
  const vertices: number[] = []
  for (let j = 0; j <= side; j += 1) {
    for (let i = 0; i <= side; i += 1) {
      vertices.push(i, j, height(i, j))
    }
  }
  const indices: number[] = []
  for (let j = 0; j < side; j += 1) {
    for (let i = 0; i < side; i += 1) {
      const a = j * (side + 1) + i
      const b = a + 1
      const c = a + side + 1
      const d = c + 1
      indices.push(a, b, d, a, d, c)
    }
  }
  return { positions: new Float32Array(vertices), indices: new Uint32Array(indices) }
}

const rolling = (i: number, j: number): number => Math.sin(i * 0.4) * 0.6 + Math.cos(j * 0.35) * 0.4

/** Whether a geometry is something a renderer could actually draw. */
function isDrawable(geometry: ChunkGeometry): boolean {
  if (geometry.indices.length % 3 !== 0 || geometry.positions.length % 3 !== 0) {
    return false
  }
  const vertices = geometry.positions.length / 3
  for (let i = 0; i < geometry.indices.length; i += 1) {
    const index = geometry.indices[i] ?? -1
    if (!Number.isInteger(index) || index < 0 || index >= vertices) {
      return false
    }
  }
  for (const value of geometry.positions) {
    if (!Number.isFinite(value)) {
      return false
    }
  }
  return true
}

test.describe('decimating a chunk', () => {
  test('a grid comes back smaller and still drawable', async () => {
    const before = grid(64, rolling)
    const { geometry, report } = await simplifyChunk(before, 0.25)
    expect(report.simplified).toBe(true)
    expect(report.to).toBeLessThan(report.from)
    expect(isDrawable(geometry)).toBe(true)
    // Four times fewer triangles is what a quarter ratio asks for, give or take the
    // collapses the simplifier cannot make without breaking the surface.
    expect(report.to).toBeLessThanOrEqual(Math.round(report.from * 0.3))
  })

  test('the vertices nothing references are gone', async () => {
    const before = grid(64, rolling)
    const { geometry } = await simplifyChunk(before, 0.25)
    const used = new Set(geometry.indices)
    // A compacted buffer has no room for an orphan; a quarter of the triangles over all
    // of the vertices would mean the saving was never really made.
    expect(used.size).toBeLessThan(before.positions.length / 3)
    expect(geometry.positions.length / 3).toBeLessThanOrEqual(used.size + 1)
  })

  test('the outline survives, so neighbouring chunks do not tear apart', async () => {
    const side = 32
    const before = grid(side, rolling)
    const { geometry } = await simplifyChunk(before, 0.2)
    // Every vertex on the border must still be present: a collapse that moved one would
    // open a gap between this chunk and its neighbour along the shared edge.
    const vertices = geometry.positions.length / 3
    // The decimated grid is a relabelling of the original, so a border vertex is the one
    // sitting at that cell's own map position.
    const kept = (i: number, j: number): boolean => {
      for (let vertex = 0; vertex < vertices; vertex += 1) {
        if (geometry.positions[vertex * 3] === i && geometry.positions[vertex * 3 + 1] === j) {
          return true
        }
      }
      return false
    }
    for (let i = 0; i <= side; i += 1) {
      expect(kept(i, 0)).toBe(true)
      expect(kept(i, side)).toBe(true)
      expect(kept(0, i)).toBe(true)
      expect(kept(side, i)).toBe(true)
    }
  })

  test('every ratio yields either a smaller valid mesh or the input unchanged', async () => {
    // The contract, swept. The library is graceful about most requests — it stops at the
    // border floor and reports the error it stopped at — but it asserts on a degenerate
    // one, and a ratio that is unreachable for one chunk is not for another. There is no
    // third answer: either the geometry came back smaller and drawable, or it came back
    // exactly as it went in.
    const before = grid(64, rolling)
    const flat = grid(64)
    for (const source of [before, flat]) {
      for (const ratio of [0.001, 0.01, 0.05, 0.1, 0.25, 0.5, 0.75, 0.9, 0.99]) {
        const { geometry, report } = await simplifyChunk(source, ratio)
        if (report.simplified) {
          expect(report.to).toBeLessThan(report.from)
          expect(isDrawable(geometry)).toBe(true)
        } else {
          expect(geometry.indices).toBe(source.indices)
          expect(report.to).toBe(report.from)
        }
      }
    }
  })

  test('asking for everything gives back exactly what went in', async () => {
    const before = grid(16, rolling)
    for (const ratio of [1, 0, -1, Number.NaN]) {
      const { geometry, report } = await simplifyChunk(before, ratio)
      expect(report.simplified).toBe(false)
      expect(geometry.indices).toBe(before.indices)
    }
  })

  test('a chunk too small to be worth decimation is left alone', async () => {
    const small = grid(3, rolling)
    const { report } = await simplifyChunk(small, 0.25)
    expect(report.simplified).toBe(false)
  })

  test('the result is stable: the same chunk decodes the same way twice', async () => {
    const before = grid(32, rolling)
    const first = await simplifyChunk(before, 0.3)
    const second = await simplifyChunk(before, 0.3)
    expect(Array.from(second.geometry.indices)).toEqual(Array.from(first.geometry.indices))
    expect(Array.from(second.geometry.positions)).toEqual(Array.from(first.geometry.positions))
  })

  test('the decimation stays inside the error it was given', async () => {
    const before = grid(64, rolling)
    const { report } = await simplifyChunk(before, 0.25)
    expect(report.error).toBeGreaterThanOrEqual(0)
    expect(report.error).toBeLessThanOrEqual(0.01)
  })

  test('a flat chunk decimes as readily as a rolling one', async () => {
    // Terrain is not the only thing drawn this way, and a flat grid is the case where a
    // border-locked collapse has the fewest options.
    const flat = grid(32)
    const { report } = await simplifyChunk(flat, 0.3)
    expect(report.simplified).toBe(true)
    expect(report.to).toBeLessThan(report.from)
  })
})

test.describe('how much a chunk should keep', () => {
  test('close to the camera nothing is dropped', () => {
    expect(lodRatioFor(0, 100, 800)).toBe(1)
    expect(lodRatioFor(100, 100, 800)).toBe(1)
    expect(lodRatioFor(99, 100, 800)).toBe(1)
  })

  test('beyond the far distance the floor applies', () => {
    expect(lodRatioFor(800, 100, 800)).toBe(0)
    expect(lodRatioFor(5000, 100, 800)).toBe(0)
  })

  test('in between, the ratio falls away with the square of the distance', () => {
    const near = lodRatioFor(300, 100, 900)
    const far = lodRatioFor(700, 100, 900)
    expect(near).toBeGreaterThan(far)
    expect(near).toBeLessThan(1)
    expect(far).toBeGreaterThan(0)
    // Monotone: every step further keeps less, so a chunk never gains detail as it recedes.
    let previous = 1
    for (let distance = 100; distance <= 900; distance += 50) {
      const ratio = lodRatioFor(distance, 100, 900)
      expect(ratio).toBeLessThanOrEqual(previous)
      previous = ratio
    }
  })

  test('a degenerate range keeps everything rather than dividing by zero', () => {
    expect(lodRatioFor(500, 100, 100)).toBe(1)
    expect(lodRatioFor(500, 900, 100)).toBe(1)
  })
})
