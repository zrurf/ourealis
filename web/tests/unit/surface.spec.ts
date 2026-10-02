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
  SHADE_RANGE,
  SLAB_FLOOR_SHADE,
  SLAB_WALL_SHADE,
  autoTerraceStep,
  cavityShade,
  slabThickness,
  slopeAccent,
  sunDirection,
  terrace,
  terrainColours,
} from '../../src/render/shading'
import { buildChunkMesh, chunkEdges, chunkGrid } from '../../src/render/terrainMesh'
import { drapedMesh, texelsPerCell } from '../../src/render/layerTexture'
import { createCellSampler } from '../../src/render/cellSampler'
import {
  effectiveTerraceStep,
  slabFloorOf,
  surfaceKey,
  surfaceStyle,
} from '../../src/render/surfaceStyle'
import { drapePath, gridPaths, meshSurfaceSampler, resample } from '../../src/render/drape'
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

  test('a vertex colour is the ramp colour at its height, scaled by its shade', () => {
    // Two vertices at the top of the range: the lit one keeps the ramp's colour, the one in
    // shadow is exactly half of it. The same pair at the bottom of the range takes the ramp's
    // first stop instead.
    const litres = terrainColours(new Float32Array([1, 1, 1, 0.5, 0, 1]), TERRAIN_RAMP)
    const last = TERRAIN_RAMP[TERRAIN_RAMP.length - 1] ?? [0, 0, 0]
    expect(litres[0]).toBeCloseTo((last[0] ?? 0) / 255, 6)
    expect(litres[4]).toBeCloseTo(((last[0] ?? 0) / 255) * 0.5, 6)
    expect(litres[8]).toBeCloseTo((TERRAIN_RAMP[0]?.[0] ?? 0) / 255, 6)
    expect(litres[3]).toBe(1)
  })

  test('the same geometry takes either ramp, which is what makes a theme switch cheap', () => {
    const input = new Float32Array([1, 1])
    const light = terrainColours(input, TERRAIN_RAMP)
    const dark = terrainColours(input, TERRAIN_RAMP_DARK)
    expect(dark[0]).toBeLessThan(light[0] ?? 0)
    expect(dark.length).toBe(light.length)
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

/** A field with one raised cell at (2, 2), the footprint the flatten test builds. */
function footprintField(i: number, j: number): number {
  return i === 2 && j === 2 ? 20 : 10
}

/** Mask reader that marks that one cell as built. */
function isFootprintCell(i: number, j: number): number {
  return i === 2 && j === 2 ? 1 : 0
}

/** Height of one vertex of a built mesh. */
function vertexHeight(
  mesh: { positions: Float32Array; grid: { columns: number } },
  i: number,
  j: number,
): number {
  return mesh.positions[(j * mesh.grid.columns + i) * 3 + 1] ?? 0
}

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

  test('a chunk carries the shape of its surface, not its colour', () => {
    const grid = gridOf([8, 4])
    const sample = createCellSampler({
      chunks: chunksOf(grid),
      grid,
      chunkSize: 4,
      level: 0,
      layerId: 1,
    })
    const mesh = buildChunkMesh(chunkGrid(grid, 4, 0, 0), sample, { range: { min: 10, max: 20 } })
    const vertices = mesh.positions.length / 3
    expect(mesh.rampInput).toHaveLength(vertices * 2)
    for (let vertex = 0; vertex < vertices; vertex += 1) {
      const t = mesh.rampInput[vertex * 2] ?? -1
      const shade = mesh.rampInput[vertex * 2 + 1] ?? -1
      // The height is normalised against the whole surface's range, so the ramp is
      // comparable across chunks; the shade is the product of every lighting term, so it
      // only has to stay positive and inside the range the relief term clamps to.
      expect(t).toBeGreaterThanOrEqual(0)
      expect(t).toBeLessThanOrEqual(1)
      expect(shade).toBeGreaterThan(0)
      expect(shade).toBeLessThanOrEqual(SHADE_RANGE[1])
    }
  })

  test('a footprint is flattened to its high-water mark, and nothing else changes', () => {
    const spec = chunkGrid(gridOf([6, 6]), 6, 0, 0)
    const plain = buildChunkMesh(spec, footprintField)
    const built = buildChunkMesh(spec, footprintField, { building: isFootprintCell })
    // The footprint cell stands on its own roof, because the height field under a block is
    // a plateau — the block's walls are separate geometry now, but the ground under it must
    // not poke a mesa through the prism.
    expect(vertexHeight(built, 2, 2)).toBe(20)
    // The corner is untouched ground, mask or no mask.
    expect(vertexHeight(built, 0, 0)).toBeCloseTo(10, 5)
    expect(vertexHeight(plain, 2, 2)).toBe(20)
    expect(vertexHeight(plain, 0, 0)).toBeCloseTo(10, 5)
  })

  test('the slab under a chunk is shaded below the surface it carries', () => {
    const grid = gridOf([4, 4])
    const mesh = buildChunkMesh(chunkGrid(grid, 4, 0, 0), () => 7, {
      range: { min: 0, max: 10 },
      slabFloorY: 0,
    })
    const shades = new Set<number>()
    for (let vertex = 0; vertex < mesh.slab.rampInput.length / 2; vertex += 1) {
      shades.add(mesh.slab.rampInput[vertex * 2 + 1] ?? 0)
    }
    // Two levels of shade and nothing else: the walls, and the floor beneath them.
    const sorted = [...shades].toSorted((a, b) => a - b)
    expect(sorted).toHaveLength(2)
    expect(sorted[0]).toBeCloseTo(SLAB_FLOOR_SHADE, 5)
    expect(sorted[1]).toBeCloseTo(SLAB_WALL_SHADE, 5)
    expect(SLAB_FLOOR_SHADE).toBeLessThan(SLAB_WALL_SHADE)
    expect(SLAB_WALL_SHADE).toBeLessThan(1)
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

  test('the style carries no appearance, so switching it rebuilds nothing', () => {
    // The key is what a view watches to decide a rebuild. It holds the values that move
    // vertices and nothing else, so a light/dark switch cannot appear in it.
    expect(surfaceKey(base)).toBe(JSON.stringify([0, 10, 1]))
    expect('ramp' in surfaceStyle(base)).toBe(false)
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
      { lift: 1 },
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
      { lift: 0 },
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

/** Cell (i, j) is `i + 10 * j`, so the surface climbs 10 m per cell along x and 1 m along y. */
function climb(i: number, j: number): number {
  return i + 10 * j
}

test.describe('the drawn surface', () => {
  /*
   * The terrain mesh puts one vertex at each cell's centre and fills the quads between
   * them, so what a reader sees between two cells is a ramp. A sampler that answers with
   * the value of the single cell containing a point returns a staircase instead, and a line
   * draped on it vanishes under every rise and hangs in the air over every dip — which is
   * exactly the "route half-buried in the ground" report these assertions guard.
   */
  const origin = { x: 0, y: 0 }

  test('a point between two cells reads the ramp, not the cell that contains it', () => {
    const surface = meshSurfaceSampler(climb, 1, origin)
    // Cell centres sit at 0.5, 1.5, ... so x = 1.0 is the exact midpoint of centres 0.5
    // and 1.5, which read 0 and 1.
    expect(surface(1, 0.5)).toBeCloseTo(0.5, 9)
    expect(surface(0.5, 0.5)).toBeCloseTo(0, 9)
    expect(surface(1.5, 0.5)).toBeCloseTo(1, 9)
  })

  test('a cell centre reads that cell exactly, so the mesh and the drape agree there', () => {
    const surface = meshSurfaceSampler(climb, 1, origin)
    expect(surface(0.5, 0.5)).toBe(0)
    expect(surface(1.5, 0.5)).toBe(1)
    expect(surface(2.5, 1.5)).toBe(12)
  })

  test('an unloaded cell reads as unknown, not as zero height', () => {
    const holed = (i: number, j: number) => (i < 0 ? null : climb(i, j))
    const surface = meshSurfaceSampler(holed, 1, origin)
    expect(surface(-0.5, 0.5)).toBeNull()
    // A column that is missing degrades to the one that loaded rather than losing the point.
    expect(surface(3.5, 0.5)).toBeCloseTo(3, 9)
  })

  test('a terrace step is applied per cell before the interpolation, not after it', () => {
    // A step of 10 quantises {0, 1, 2} to {0, 0, 0}, so the whole ramp flattens; the
    // midpoint must read the same 0 rather than an interpolated half.
    const flat = meshSurfaceSampler(climb, 1, origin, 10)
    expect(flat(1, 0.5)).toBe(0)
    expect(flat(2.5, 0.5)).toBe(0)
  })

  test('the surface follows the map origin, not the world zero plane', () => {
    const shifted = meshSurfaceSampler(climb, 1, { x: 100, y: 200 })
    expect(shifted(100.5, 200.5)).toBe(0)
    expect(shifted(101.5, 200.5)).toBe(1)
  })
})

test.describe('resampling before a drape', () => {
  test('a long segment is subdivided so it cannot cut through a rise', () => {
    const out = resample(
      [
        { x: 0, y: 0 },
        { x: 10, y: 0 },
      ],
      2,
    )
    expect(out.map((point) => point.x)).toEqual([0, 2, 4, 6, 8, 10])
  })

  test('a segment already shorter than the spacing is left alone', () => {
    const out = resample(
      [
        { x: 0, y: 0 },
        { x: 1, y: 0 },
      ],
      5,
    )
    expect(out).toEqual([
      { x: 0, y: 0 },
      { x: 1, y: 0 },
    ])
  })

  test('no spacing means the vertices are used as they are', () => {
    const path = [
      { x: 0, y: 0 },
      { x: 30, y: 0 },
    ]
    expect(resample(path, 0)).toEqual(path)
  })

  test('a drape over a ridge follows the ridge rather than the chord across it', () => {
    // A cell that is five metres high between two that are flat: sampling only the two ends
    // would draw a straight line through the base of the hill.
    const surface = meshSurfaceSampler((i) => (i === 1 ? 5 : 0), 1, { x: 0, y: 0 })
    const draped = drapePath(
      [
        { x: 0.5, y: 0.5 },
        { x: 2.5, y: 0.5 },
      ],
      surface,
      { lift: 0, spacingM: 0.5 },
    )
    const peak = Math.max(...draped.map((point) => point[1]))
    expect(peak).toBeCloseTo(5, 9)
  })
})

test.describe('texel resolution follows the zoom', () => {
  const patterned = { pattern: 'asphalt', materials: null } as unknown as NonNullable<
    ReturnType<(typeof import('../../src/render/surfaces'))['layerSurfacePlan']>
  >
  const flat = { pattern: 'flat', materials: null } as unknown as typeof patterned

  test('a pattern needs room when the camera is close, and not when it is far', () => {
    // A 2 m cell seen at 0.25 m/px fills eight pixels, so the hatch is resolved; the
    // same cell seen at 20 m/px is a fraction of a pixel and cannot show one.
    expect(texelsPerCell(patterned, 0.25, 2)).toBe(4)
    expect(texelsPerCell(patterned, 20, 2)).toBe(1)
  })

  test('a flat layer is one texel a cell however close the camera is', () => {
    expect(texelsPerCell(flat, 0.01, 100)).toBe(1)
  })

  test('an unknown zoom falls back to the closest resolution rather than guessing low', () => {
    expect(texelsPerCell(patterned)).toBe(4)
    expect(texelsPerCell(null, 0.25, 2)).toBe(1)
  })

  test('the resolution never falls below one texel a cell', () => {
    expect(texelsPerCell(patterned, 1000, 0.5)).toBe(1)
  })
})
