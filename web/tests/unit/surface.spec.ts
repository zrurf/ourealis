/*
 * The surface style: what a viewport parameter does to the terrain.
 *
 * The model style is the part of the viewer a reader cannot check by reading code, so the
 * invariants it rests on are pinned here instead: the ramp stays neutral (a coloured
 * ground competes with the layers drawn on it), the terracing quantises to its own step,
 * the shading responds to the sun the way a surface does, and two neighbouring chunks
 * agree *bit for bit* about the vertex they share — which is the property that keeps a
 * large map from reading as a patchwork.
 */
import { expect, test } from '@playwright/test'
import {
  autoTerraceStep,
  cavityShade,
  reliefShade,
  slabColour,
  slabThickness,
  slopeAccent,
  sunDirection,
  terrace,
  terrainColour,
} from '../../src/render/shading'
import { buildChunkMesh, chunkEdges, chunkGrid } from '../../src/render/terrainMesh'
import { drapedMesh } from '../../src/render/layerTexture'
import { createCellSampler } from '../../src/render/cellSampler'
import { effectiveTerraceStep, slabFloorOf, surfaceStyle } from '../../src/render/surfaceStyle'
import { drapePath, gridPaths } from '../../src/render/drape'
import { TERRAIN_RAMP, TERRAIN_RAMP_DARK } from '../../src/types/colormap'
import { drapeSourceLevel, mortonEncodeChunk, type DecodedChunk } from '../../src/types/map'
import type { LayerGrid } from '../../src/api/types'

/** A grid of `dims` cells at one level, 4x4 cells per chunk, default origin. */
function gridOf(dims: [number, number], resolution = 1): LayerGrid {
  return {
    map_id: 'm',
    layer_id: 1,
    chunk_size: 4,
    level_res_m: [resolution],
    level_dims: [dims],
    chunks: [Array.from({ length: 8 }, (_, index) => mortonEncodeChunk(index % 4, index >> 2))],
  } as unknown as LayerGrid
}

/**
 * A sloped field over the whole grid, so two chunks that share a column agree about it.
 */
function sloped(i: number, j: number): number {
  return 10 + i * 0.3 + j * 0.1
}

/** Every chunk of a grid, filled with {@link sloped}, keyed the way the store keys them. */
function chunksOf(grid: LayerGrid, size = 4): Map<string, DecodedChunk> {
  const chunks = new Map<string, DecodedChunk>()
  for (const id of grid.chunks[0] ?? []) {
    const ix = id % 4
    const iy = Math.floor(id / 4)
    const values = new Float32Array(size * size)
    for (let row = 0; row < size; row += 1) {
      for (let column = 0; column < size; column += 1) {
        values[row * size + column] = sloped(ix * size + column, iy * size + row)
      }
    }
    chunks.set(`${grid.layer_id}/0/${id}`, {
      layerId: grid.layer_id,
      level: 0,
      chunkId: id,
      width: size,
      height: size,
      channels: 1,
      values,
      scale: 1,
      bias: 0,
    })
  }
  return chunks
}

/** Extent of a mesh along x, for the "no gap and no overlap" check. */
function spanOf(positions: Float32Array): { min: number; max: number } {
  let min = Number.POSITIVE_INFINITY
  let max = Number.NEGATIVE_INFINITY
  for (let index = 0; index < positions.length; index += 3) {
    min = Math.min(min, positions[index] ?? 0)
    max = Math.max(max, positions[index] ?? 0)
  }
  return { min, max }
}

/**
 * A surface that rises to the east and has no ground west of zero, so both the sampling
 * and the "no data" branch of a drape can be exercised.
 */
function groundAt(x: number): number | null {
  return x < 0 ? null : 10 + x
}

test.describe('shading', () => {
  test('the surface ramp carries almost no colour', () => {
    for (const ramp of [TERRAIN_RAMP, TERRAIN_RAMP_DARK]) {
      for (const [r, g, b] of ramp) {
        // A cool grey: the channels differ by a few percent, which reads as neutral. A
        // saturated stop — the green ramp's [46, 84, 66] — differs by a third.
        const spread = Math.max(r, g, b) - Math.min(r, g, b)
        expect(spread).toBeLessThanOrEqual(24)
      }
    }
  })

  test('the ramp runs dark to light, so height reads without colour', () => {
    for (const ramp of [TERRAIN_RAMP, TERRAIN_RAMP_DARK]) {
      const luminance = ramp.map(([r, g, b]) => 0.2126 * r + 0.7152 * g + 0.0722 * b)
      expect(luminance).toEqual([...luminance].toSorted((a, b) => a - b))
    }
  })

  test('a face toward the sun is brighter than one turned away from it', () => {
    const sun = sunDirection()
    const facing = reliefShade([sun[0], sun[1], sun[2]], sun)
    const away = reliefShade([-sun[0], -sun[1], -sun[2]], sun)
    expect(facing).toBeGreaterThan(away)
    expect(away).toBeGreaterThan(0)
  })

  test('the sun vector is a unit vector whatever the azimuth', () => {
    for (const azimuth of [0, 45, 90, 180, 270, 359]) {
      const [x, y, z] = sunDirection(azimuth, 52)
      expect(Math.hypot(x, y, z)).toBeCloseTo(1, 6)
      expect(y).toBeGreaterThan(0)
    }
  })

  test('terracing quantises to its own step, and zero leaves the height alone', () => {
    expect(terrace(12.4, 0.5)).toBeCloseTo(12.5, 6)
    expect(terrace(12.2, 0.5)).toBeCloseTo(12, 6)
    expect(terrace(12.2, 0)).toBe(12.2)
    expect(terrace(-3.3, 0.5)).toBeCloseTo(-3.5, 6)
  })

  test('a hollow is occluded and a ridge is not', () => {
    expect(cavityShade(1)).toBeLessThan(1)
    expect(cavityShade(0)).toBe(1)
    expect(cavityShade(-1)).toBe(1)
    expect(cavityShade(0.5)).toBeGreaterThan(cavityShade(1))
  })

  test('a cliff is darkened and flat ground is not', () => {
    expect(slopeAccent(1)).toBeCloseTo(1, 6)
    expect(slopeAccent(0.2)).toBeLessThan(1)
  })

  test('a vertex colour stays inside its ramp, scaled by the shade', () => {
    const range = { min: 0, max: 10 }
    const dark = terrainColour(0, 0.5, TERRAIN_RAMP, range)
    const light = terrainColour(10, 1, TERRAIN_RAMP, range)
    expect(light[0]).toBeGreaterThan(dark[0])
    expect(TERRAIN_RAMP[0]?.[0]).toBe(Math.min(...TERRAIN_RAMP.map((stop) => stop[0])))
  })

  test('a slab face is darker than the surface it belongs to', () => {
    const top = terrainColour(5, 1, TERRAIN_RAMP, { min: 0, max: 10 })
    const slab = slabColour(top)
    expect(slab[0]).toBeLessThanOrEqual(top[0])
    expect(slab[1]).toBeLessThanOrEqual(top[1])
  })

  test('the automatic terrace step is a round number that fits the relief', () => {
    // A dozen levels across the relief, rounded up to one, two or five times a power of
    // ten: an 11 m campus gets 1 m steps, a 100 m valley gets 10 m ones.
    expect(autoTerraceStep({ min: 0, max: 11 })).toBe(1)
    expect(autoTerraceStep({ min: 0, max: 100 })).toBe(10)
    expect(autoTerraceStep({ min: 0, max: 22.8 })).toBe(2)
    expect(autoTerraceStep({ min: 4, max: 4 })).toBe(0)
  })
})

test.describe('chunk meshes', () => {
  test('two neighbouring chunks draw the interface between them exactly once', () => {
    const grid = gridOf([8, 4])
    const sample = createCellSampler({
      chunks: chunksOf(grid),
      grid,
      chunkSize: 4,
      level: 0,
      layerId: 1,
    })
    const left = buildChunkMesh(chunkGrid(grid, 4, 0, 0), sample, { terraceM: 0.5 })
    const right = buildChunkMesh(chunkGrid(grid, 4, 0, 1), sample, { terraceM: 0.5 })
    // The left chunk reaches one cell into the right one to draw the boundary quad; the
    // right chunk starts at its own first cell. Together they cover the map with no gap
    // and no overlap.
    const leftSpan = spanOf(left.positions)
    const rightSpan = spanOf(right.positions)
    // The left chunk's last vertex *is* the right chunk's first one, so the two meshes
    // meet exactly: no gap to see through and no strip drawn twice.
    expect(rightSpan.min).toBe(leftSpan.max)
    for (let row = 0; row < left.grid.rows; row += 1) {
      const leftWall = (row * left.grid.columns + (left.grid.columns - 1)) * 3
      const rightStart = row * right.grid.columns * 3
      expect(left.positions[leftWall]).toBe(right.positions[rightStart])
      expect(left.positions[leftWall + 1]).toBe(right.positions[rightStart + 1])
    }
  })

  test('a chunk at the map rim is closed by a slab wall, an interior one is not', () => {
    const grid = gridOf([12, 12])
    const sample = createCellSampler({
      chunks: chunksOf(grid),
      grid,
      chunkSize: 4,
      level: 0,
      layerId: 1,
    })
    const edge = buildChunkMesh(chunkGrid(grid, 4, 0, 0), sample, {})
    const interior = buildChunkMesh(chunkGrid(grid, 4, 0, mortonEncodeChunk(1, 1)), sample, {
      slabFloorY: 0,
    })
    // A rim chunk has walls and a floor; an interior one only has the floor.
    expect(edge.slab.indices.length).toBeGreaterThan(6)
    expect(interior.slab.indices.length).toBe(6)
  })

  test('the terracing is visible in the geometry: one step, one height', () => {
    const grid = gridOf([4, 4])
    const flat: ReadonlyMap<string, DecodedChunk> = new Map()
    const sample = createCellSampler({ chunks: flat, grid, chunkSize: 4, level: 0, layerId: 1 })
    expect(sample(0, 0)).toBeNull()
    const heights = new Set<number>()
    const mesh = buildChunkMesh(chunkGrid(grid, 4, 0, 0), () => 7.2, { terraceM: 0.5 })
    for (let index = 1; index < mesh.positions.length; index += 3) {
      heights.add(mesh.positions[index] ?? 0)
    }
    expect([...heights]).toEqual([7])
  })

  test('the rim of the map is recognised from the chunk grid', () => {
    const grid = gridOf([8, 4])
    expect(chunkEdges(grid, 4, 0, mortonEncodeChunk(0, 0))).toEqual({
      west: true,
      east: false,
      north: true,
      south: true,
    })
    expect(chunkEdges(grid, 4, 0, mortonEncodeChunk(1, 0)).east).toBe(true)
  })

  test('a missing chunk reads as unknown, not as zero', () => {
    const grid = gridOf([8, 4])
    const sample = createCellSampler({
      chunks: new Map(),
      grid,
      chunkSize: 4,
      level: 0,
      layerId: 1,
    })
    expect(sample(0, 0)).toBeNull()
    expect(sample(-1, 0)).toBeNull()
    expect(sample(8, 0)).toBeNull()
  })

  test('cells read back at the global indices the mesh asks for', () => {
    const grid = gridOf([8, 4])
    const sample = createCellSampler({
      chunks: chunksOf(grid),
      grid,
      chunkSize: 4,
      level: 0,
      layerId: 1,
    })
    // The field is 10 + i*0.3 + j*0.1, so the global index is what makes the value.
    expect(sample(4, 0)).toBeCloseTo(11.2, 5)
    expect(sample(7, 3)).toBeCloseTo(12.4, 5)
  })

  test('a flat map still gets a slab with a minimum thickness', () => {
    expect(slabThickness({ min: 3, max: 3 })).toBe(4)
    expect(slabThickness({ min: 0, max: 100 })).toBe(60)
  })
})

test.describe('the drape of a layer', () => {
  test('a vertex samples the texel of the cell it stands on, on both axes', () => {
    const grid = gridOf([8, 4])
    const mesh = buildChunkMesh(
      chunkGrid(grid, 4, 0, 0),
      createCellSampler({ chunks: chunksOf(grid), grid, chunkSize: 4, level: 0, layerId: 1 }),
    )
    const drape = drapedMesh(mesh, 0)
    const { columns, rows } = mesh.grid
    for (let row = 0; row < rows; row += 1) {
      for (let column = 0; column < columns; column += 1) {
        const vertex = row * columns + column
        const u = drape.uvs[vertex * 2] ?? 0
        const v = drape.uvs[vertex * 2 + 1] ?? 0
        // The texture has one texel per cell, written in the mesh's own row order, and it is
        // uploaded without a flip: so the texel a vertex must sample is its own cell. Reading
        // the *other* row (a mirrored v) drew every layer upside down against the ground it
        // was draped on, which is what "the feature layer does not line up" looked like.
        expect(Math.floor(u * columns)).toBe(column)
        expect(Math.floor(v * rows)).toBe(row)
      }
    }
  })

  test('a drape keeps the terrain heights, plus its lift', () => {
    const grid = gridOf([8, 4])
    const mesh = buildChunkMesh(
      chunkGrid(grid, 4, 0, 0),
      createCellSampler({ chunks: chunksOf(grid), grid, chunkSize: 4, level: 0, layerId: 1 }),
    )
    const drape = drapedMesh(mesh, 0.5)
    for (let index = 1; index < mesh.positions.length; index += 3) {
      expect((drape.positions[index] ?? 0) - (mesh.positions[index] ?? 0)).toBeCloseTo(0.5, 6)
    }
  })
})

test.describe('surface style', () => {
  const base = {
    range: { min: 0, max: 10 },
    dark: false,
    terraceM: null,
    sunAzimuthDeg: 315,
    origin: { x: 0, y: 0 },
  }

  test('the terrace follows the relief until the reader overrides it', () => {
    expect(effectiveTerraceStep(base)).toBe(autoTerraceStep(base.range))
    expect(effectiveTerraceStep({ ...base, terraceM: 0 })).toBe(0)
    expect(effectiveTerraceStep({ ...base, terraceM: 2.5 })).toBe(2.5)
  })

  test('the slab sits below the lowest ground by a fraction of the relief', () => {
    expect(slabFloorOf(base)).toBe(-6)
    expect(slabFloorOf({ ...base, range: null })).toBe(0)
  })

  test('the dark appearance uses the dark ramp', () => {
    expect(surfaceStyle(base).ramp).toBe(TERRAIN_RAMP)
    expect(surfaceStyle({ ...base, dark: true }).ramp).toBe(TERRAIN_RAMP_DARK)
  })
})

test.describe('draping', () => {
  test('a path takes its height from the ground under it', () => {
    const path = drapePath(
      [
        { x: 0, y: 0 },
        { x: 2, y: 0 },
      ],
      groundAt,
      1,
    )
    expect(path).toEqual([
      [0, 11, 0],
      [2, 13, 0],
    ])
  })

  test('a point without ground keeps the previous height rather than dropping', () => {
    const path = drapePath(
      [
        { x: 1, y: 0 },
        { x: -5, y: 0 },
        { x: 3, y: 0 },
      ],
      groundAt,
      0,
    )
    expect(path[1]?.[1]).toBe(11)
    expect(path[2]?.[1]).toBe(13)
  })

  test('a grid covers the extent and follows the terrain between its ends', () => {
    const lines = gridPaths({ min_x: 0, min_y: 0, max_x: 10, max_y: 10 }, 5, groundAt, {
      samples: 4,
      lift: 0,
    })
    // Three lines each way for a step of five over ten metres, with no duplicate at the
    // rim because the step lands on it.
    expect(lines).toHaveLength(6)
    expect(lines[0]?.length).toBe(5)
    expect(lines[0]?.[0]?.[1]).toBe(10)
  })

  test('a step that misses the far edge still closes the grid', () => {
    const lines = gridPaths({ min_x: 0, min_y: 0, max_x: 7, max_y: 7 }, 5, groundAt, { samples: 2 })
    expect(lines).toHaveLength(6)
  })

  test('a grid without a step draws nothing', () => {
    expect(gridPaths({ min_x: 0, min_y: 0, max_x: 10, max_y: 10 }, 0, groundAt)).toEqual([])
  })
})

/**
 * A grid whose only stored level is the finest one, as the derived layers of a map are: the
 * mask and the features exist at level 0 while the elevation carries three levels.
 */
function crossLevelGrid(): LayerGrid {
  return {
    map_id: 'm',
    layer_id: 8193,
    chunk_size: 4,
    level_res_m: [1, 2, 4],
    level_dims: [
      [8, 8],
      [4, 4],
      [2, 2],
    ],
    chunks: [
      [
        mortonEncodeChunk(0, 0),
        mortonEncodeChunk(1, 0),
        mortonEncodeChunk(0, 1),
        mortonEncodeChunk(1, 1),
      ],
      [],
      [],
    ],
  } as unknown as LayerGrid
}

/** The stored chunks of {@link crossLevelGrid}, each with its first cell blocked. */
function crossLevelChunks(grid: LayerGrid): Map<string, DecodedChunk> {
  const chunks = new Map<string, DecodedChunk>()
  for (const id of grid.chunks[0] ?? []) {
    const values = new Float32Array(16)
    values[0] = 1
    chunks.set(`8193/0/${id}`, {
      layerId: 8193,
      level: 0,
      chunkId: id,
      width: 4,
      height: 4,
      channels: 1,
      values,
      scale: 1,
      bias: 0,
    })
  }
  return chunks
}

test.describe('reading a layer that lives at another level', () => {
  test('the source level is the finest one the layer stores', () => {
    const grid = crossLevelGrid()
    expect(drapeSourceLevel(grid, 1)).toBe(0)
    expect(drapeSourceLevel(grid, 0)).toBe(0)
    const empty = { ...grid, chunks: [[], [], []] } as unknown as LayerGrid
    expect(drapeSourceLevel(empty, 1)).toBeNull()
  })

  test('a cell of the drawn level is read from the stored level', () => {
    const grid = crossLevelGrid()
    const sample = createCellSampler({
      chunks: crossLevelChunks(grid),
      grid,
      chunkSize: 4,
      level: 1,
      layerId: 8193,
      sourceLevel: 0,
    })
    // A drawn cell at level 1 spans two stored cells at level 0 on each axis, and reads
    // the first of them: (0, 0) is stored (0, 0), which carries the blocked value.
    expect(sample(0, 0)).toBe(1)
    expect(sample(1, 1)).toBe(0)
    expect(sample(3, 3)).toBe(0)
    // Stored (4, 4) is the first cell of the chunk beside it, which is blocked as well.
    expect(sample(2, 2)).toBe(1)
    // Beyond the stored extent there is no sample to read.
    expect(sample(4, 4)).toBeNull()
  })
})
