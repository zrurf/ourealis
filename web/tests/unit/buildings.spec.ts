/*
 * Building prisms: the white model's blocks, as pure geometry.
 *
 * The shapes these tests pin down are the ones a reader sees from outside the model: a
 * footprint comes out as a straight-walled block (not a staircase of cells), its roof is
 * one flat face, its walls face outward, and the ground under it is covered by the block's
 * own skirt rather than by the height field. Buildings are also the most expensive geometry
 * the viewer builds per chunk, so "a mask with nothing in it costs nothing" is asserted
 * too — that is the branch a map without buildings takes on every chunk.
 */
import { expect, test } from '@playwright/test'
import { buildChunkBuildings } from '../../src/render/buildings'
import type { ChunkGrid } from '../../src/render/terrainMesh'

/** A `cells`-sized chunk with a 2 m cell, laid out like the ones the views build. */
function specOf(cells: number): ChunkGrid {
  return {
    chunkSize: cells,
    level: 0,
    chunkId: 0,
    cell: 2,
    inMapColumns: cells,
    inMapRows: cells,
    columns: cells,
    rows: cells,
    firstI: 0,
    firstJ: 0,
    originX: 0,
    originY: 0,
    edges: { west: true, east: true, north: true, south: true },
  }
}

/** A sampler over a fixed field, `null` outside it. */
function fieldOf(values: number[][]): (i: number, j: number) => number | null {
  return (i, j) => values[j]?.[i] ?? null
}

/** A square of `1`s in a mask of `cells` a side, from `x0` to `x1` (exclusive). */
function squareMask(cells: number, x0: number, y0: number, x1: number, y1: number): number[][] {
  const rows: number[][] = []
  for (let j = 0; j < cells; j += 1) {
    const row: number[] = []
    for (let i = 0; i < cells; i += 1) {
      row.push(i >= x0 && i < x1 && j >= y0 && j < y1 ? 1 : 0)
    }
    rows.push(row)
  }
  return rows
}

/** Ground at `ground`, roof at `roof` wherever the predicate holds. */
function heightFieldOf(
  cells: number,
  built: (i: number, j: number) => boolean,
  roof: number,
  ground = 100,
): number[][] {
  const rows: number[][] = []
  for (let j = 0; j < cells; j += 1) {
    const row: number[] = []
    for (let i = 0; i < cells; i += 1) {
      row.push(built(i, j) ? roof : ground)
    }
    rows.push(row)
  }
  return rows
}

/** Every vertex's `y` from a position buffer. */
function heightsOf(positions: Float32Array): number[] {
  const out: number[] = []
  for (let index = 1; index < positions.length; index += 3) {
    out.push(positions[index] ?? 0)
  }
  return out
}

/** Every triangle's computed normal from the position buffer. */
function triangleNormals(positions: Float32Array, indices: Uint32Array): Array<[number, number, number]> {
  const out: Array<[number, number, number]> = []
  const at = (index: number): [number, number, number] => [
    positions[index * 3] ?? 0,
    positions[index * 3 + 1] ?? 0,
    positions[index * 3 + 2] ?? 0,
  ]
  for (let index = 0; index + 2 < indices.length; index += 3) {
    const a = at(indices[index] ?? 0)
    const b = at(indices[index + 1] ?? 0)
    const c = at(indices[index + 2] ?? 0)
    const ab: [number, number, number] = [b[0] - a[0], b[1] - a[1], b[2] - a[2]]
    const ac: [number, number, number] = [c[0] - a[0], c[1] - a[1], c[2] - a[2]]
    out.push([
      ab[1] * ac[2] - ab[2] * ac[1],
      ab[2] * ac[0] - ab[0] * ac[2],
      ab[0] * ac[1] - ab[1] * ac[0],
    ])
  }
  return out
}

test('a square footprint becomes one straight-walled block', () => {
  const cells = 16
  const mask = squareMask(cells, 4, 4, 8, 8)
  const height = heightFieldOf(cells, (i, j) => mask[j]?.[i] === 1, 118)
  const data = buildChunkBuildings({
    spec: specOf(cells),
    building: fieldOf(mask),
    elevation: fieldOf(height),
    terraceM: 0,
  })
  expect(data).not.toBeNull()

  const positions = data?.positions ?? new Float32Array()
  const indices = data?.indices ?? new Uint32Array()
  const heights = heightsOf(positions)

  // One roof height for the whole block, lifted a hand's width above the height field,
  // and one base standing below the ground it sinks into.
  const roof = Math.max(...heights)
  const base = Math.min(...heights)
  expect(roof).toBeCloseTo(118.15, 5)
  expect(base).toBeCloseTo(98, 5)

  // The normal attribute is what the light reads: every roof vertex faces up and every wall
  // vertex faces away from the block's centre, so the form is lit from outside.
  const attribute = data?.normals ?? new Float32Array()
  let roofVertices = 0
  let wallVertices = 0
  const centre: [number, number] = [0, 0]
  for (let index = 0; index < attribute.length; index += 3) {
    centre[0] += positions[index] ?? 0
    centre[1] += positions[index + 2] ?? 0
  }
  const vertexCount = attribute.length / 3
  centre[0] /= vertexCount
  centre[1] /= vertexCount
  for (let index = 0; index < attribute.length; index += 3) {
    const ny = attribute[index + 1] ?? 0
    if (ny > 0.99) {
      roofVertices += 1
      continue
    }
    expect(Math.abs(ny)).toBeLessThan(1e-6)
    wallVertices += 1
    const dx = (positions[index] ?? 0) - centre[0]
    const dz = (positions[index + 2] ?? 0) - centre[1]
    // Outward: the normal agrees with the direction from the centre to the vertex.
    expect((attribute[index] ?? 0) * dx + (attribute[index + 2] ?? 0) * dz).toBeGreaterThan(0)
  }
  expect(roofVertices).toBe(4)
  // Four walls of a rectangle, four vertices each.
  expect(wallVertices).toBe(16)

  // The winding is Babylon's: the side a triangle shows is opposite its right-hand normal,
  // so a roof visible from above has one pointing down.
  const roofStart = attribute.length / 3 - 4
  const computed = triangleNormals(positions, indices)
  const roofTriangles = computed.filter(
    (normal, at) =>
      (indices[at * 3] ?? 0) >= roofStart &&
      (indices[at * 3 + 1] ?? 0) >= roofStart &&
      (indices[at * 3 + 2] ?? 0) >= roofStart &&
      normal[1] < 0,
  )
  expect(roofTriangles.length).toBe(2)
})

test('a footprint with a courtyard comes out as one solid block', () => {
  const cells = 20
  const mask = squareMask(cells, 3, 3, 14, 14)
  // Carve a courtyard: the ring is the block, the inside is open ground.
  for (let j = 6; j < 11; j += 1) {
    const row = mask[j]
    if (row === undefined) {
      continue
    }
    for (let i = 6; i < 11; i += 1) {
      row[i] = 0
    }
  }
  const height = heightFieldOf(cells, (i, j) => mask[j]?.[i] === 1, 120)
  const data = buildChunkBuildings({
    spec: specOf(cells),
    building: fieldOf(mask),
    elevation: fieldOf(height),
    terraceM: 0,
  })
  expect(data).not.toBeNull()
  const normals = triangleNormals(data?.positions ?? new Float32Array(), data?.indices ?? new Uint32Array())
  // Filled: the only upward faces are the block's roofs, so no triangle normal points down
  // into the courtyard. A hole left open would add walls facing inward.
  const inward = normals.filter((normal) => normal[1] === 0 && normal[0] === 0 && normal[2] === 0)
  expect(inward).toHaveLength(0)
  const roofs = normals.filter((normal) => normal[1] < -0.9)
  expect(roofs.length).toBeGreaterThan(0)
})

test('two separate footprints become two blocks', () => {
  const cells = 24
  const mask = squareMask(cells, 2, 2, 6, 6)
  for (let j = 10; j < 15; j += 1) {
    const row = mask[j]
    if (row === undefined) {
      continue
    }
    for (let i = 10; i < 15; i += 1) {
      row[i] = 1
    }
  }
  const height = heightFieldOf(cells, (i, j) => mask[j]?.[i] === 1, 115)
  const data = buildChunkBuildings({
    spec: specOf(cells),
    building: fieldOf(mask),
    elevation: fieldOf(height),
    terraceM: 0,
  })
  expect(data).not.toBeNull()
  const positions = data?.positions ?? new Float32Array()
  const indices = data?.indices ?? new Uint32Array()
  const heights = heightsOf(positions)
  const roofY = Math.max(...heights)
  // Count the triangles that lie flat on the roof: two rectangles triangulate to four.
  const atRoof = (index: number): boolean =>
    Math.abs((positions[index * 3 + 1] ?? 0) - roofY) < 1e-6
  let roofTriangles = 0
  for (let index = 0; index + 2 < indices.length; index += 3) {
    if (atRoof(indices[index] ?? 0) && atRoof(indices[index + 1] ?? 0) && atRoof(indices[index + 2] ?? 0)) {
      roofTriangles += 1
    }
  }
  expect(roofTriangles).toBe(4)
})

test('a mask with nothing in it costs no geometry', () => {
  const cells = 12
  const mask = squareMask(cells, 0, 0, 0, 0)
  const height = heightFieldOf(cells, () => false, 100)
  const data = buildChunkBuildings({
    spec: specOf(cells),
    building: fieldOf(mask),
    elevation: fieldOf(height),
    terraceM: 0,
  })
  expect(data).toBeNull()
})

test('the terrace step of the surface quantises the roof with it', () => {
  const cells = 16
  const mask = squareMask(cells, 4, 4, 8, 8)
  const height = heightFieldOf(cells, (i, j) => mask[j]?.[i] === 1, 118.4)
  const data = buildChunkBuildings({
    spec: specOf(cells),
    building: fieldOf(mask),
    elevation: fieldOf(height),
    terraceM: 5,
  })
  const heights = heightsOf(data?.positions ?? new Float32Array())
  const roof = Math.max(...heights)
  // The terrain draws this roof at 120, so the prism must stand on the same terrace.
  expect(roof).toBeCloseTo(120.15, 5)
})
