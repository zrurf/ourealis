/*
 * Elevation chunks as vertex data.
 *
 * One mesh per `(level, chunk)` pair, built from a cell reader so a chunk's geometry can
 * reach past its own boundary where the surface continues into the next chunk. Levels are
 * chosen by the caller (`selectLevel` in `types/map.ts`), which is what makes a coarse
 * level show first and finer levels fill in.
 *
 * Three conventions hold the surface together, and each of them was a visible defect when
 * it was missing:
 *
 * - **One vertex per cell, at the cell's centre**, and the quad between two neighbouring
 *   cells is drawn by whichever chunk owns the left-hand one. The interface between two
 *   chunks is therefore drawn exactly once, from the *same* global cells on both sides. A
 *   mesh that stopped at its own cells left a one-cell hole along every boundary — the
 *   seam that made a large map look like tiles that had slid apart.
 * - **The map's rim is pushed out to the map's own edge** and dropped to a floor, so the
 *   surface is a slab with a thickness rather than a sheet floating over the background.
 * - **Terracing quantises the height** before anything else reads it, so a level is one
 *   flat block and the step between two levels is a face of its own.
 *
 * Pure geometry: a cell reader in, typed arrays out, nothing from Babylon. That is what
 * lets these conventions be tested without an engine — including the bit-for-bit
 * agreement between two neighbouring chunks. `render/terrain.ts` hands the arrays to
 * Babylon.
 */
import { normalize } from '@/types/colormap'
import {
  chunkBounds,
  inMapDims,
  levelCellSize,
  mortonDecodeChunk,
  type PlaneOrigin,
} from '@/types/map'
import type { LayerGrid } from '@/api/types'
import type { CellSampler } from './cellSampler'
import {
  SLAB_FLOOR_SHADE,
  SLAB_WALL_SHADE,
  cavityShade,
  slopeAccent,
  terrace,
} from './shading'

/** Vertex data of one mesh part, in world metres with the elevation in y. */
export interface MeshData {
  /** Vertex positions, `x, y, z` per vertex. */
  positions: Float32Array
  /** Triangle indices. */
  indices: Uint32Array
  /** Vertex normals. */
  normals: Float32Array
  /**
   * Per-vertex ramp inputs, `t, shade` per vertex.
   *
   * The surface colour is `ramp(t) * shade`, and neither term depends on the appearance,
   * so the geometry survives a light/dark switch: the ramp is applied to this buffer
   * afterwards, and an appearance change is an upload of the colour buffer alone. Baking
   * the colour here instead would make every appearance change rebuild the map.
   *
   * `t` is the elevation normalised against the whole surface's range, so a chunk drawn
   * on its own is not rescaled into a different range than its neighbours.
   */
  rampInput: Float32Array
  /**
   * Texture coordinates in *world space*, one period per metre count.
   *
   * The surface carries a fine grain texture; sampling it by world position rather than by
   * cell means the grain runs across cell and chunk boundaries instead of restarting in
   * each of them, and a chunk that arrives late continues the same texture.
   */
  uvs: Float32Array
}

/** Metres one period of the surface grain covers. */
export const GRAIN_PERIOD_M = 3.2

/** Vertex data of one chunk surface. */
export interface ChunkMeshData extends MeshData {
  /** Grid dimensions the vertices were laid out on, `(columns, rows)`. */
  grid: { columns: number; rows: number }
  /** The slab the surface sits on: the rim dropped to a floor, plus the floor. */
  slab: MeshData
}

/** Which sides of a chunk are the map's own rim. */
export interface ChunkEdges {
  /** Western side is the map's rim. */
  west: boolean
  /** Eastern side is the map's rim. */
  east: boolean
  /** Northern side (`min_y`) is the map's rim. */
  north: boolean
  /** Southern side (`max_y`) is the map's rim. */
  south: boolean
}

/**
 * Where a chunk's mesh stands and which cells it covers.
 *
 * The vertex grid has one cell per in-map cell and, on every side that is *not* the map's
 * rim, one more at the shore of the next chunk: the quad that spans the boundary belongs
 * to the left-hand chunk, and it can only be drawn by a vertex that stands on the
 * neighbour's first cell.
 */
export interface ChunkGrid {
  /** Chunk side length in cells, as stored. */
  chunkSize: number
  /** Level the cells belong to. */
  level: number
  /** Morton id of the chunk. */
  chunkId: number
  /** Cell size at the level, metres. */
  cell: number
  /** In-map columns of this chunk. */
  inMapColumns: number
  /** In-map rows of this chunk. */
  inMapRows: number
  /** Mesh columns: in-map columns plus the shore vertex on an internal east side. */
  columns: number
  /** Mesh rows: in-map rows plus the shore vertex on an internal south side. */
  rows: number
  /** Global column of the mesh's first cell. */
  firstI: number
  /** Global row of the mesh's first cell. */
  firstJ: number
  /** World x of the mesh's first cell centre. */
  originX: number
  /** World z of the mesh's first cell centre. */
  originY: number
  /** Which sides are the map's own rim. */
  edges: ChunkEdges
}

/** Options of {@link buildChunkMesh}. */
export interface ChunkMeshOptions {
  /**
   * Elevation range the ramp spans, metres.
   *
   * Passed in by the caller rather than measured per chunk: a chunk scaled to its own
   * minimum and maximum would colour the same height differently from its neighbour,
   * which is exactly what the ramp exists to prevent.
   */
  range?: { min: number; max: number }
  /** Terrace step, metres; `0` leaves the surface as surveyed. */
  terraceM?: number
  /** World height of the slab's underside, metres; the range minimum by default. */
  slabFloorY?: number
  /**
   * Reader of the building mask, one value per cell.
   *
   * The mask rides in the elevation layer's second channel: without it a building is drawn as
   * ground at its roof height rather than as the block it is.
   */
  building?: CellSampler
}

/** Where one chunk's mesh stands, from the level's grid and the map's origin. */
export function chunkGrid(
  grid: LayerGrid,
  chunkSize: number,
  level: number,
  chunkId: number,
  origin: PlaneOrigin = { x: 0, y: 0 },
): ChunkGrid {
  const { ix, iy } = mortonDecodeChunk(chunkId)
  const side = Math.max(1, chunkSize)
  const inMap = inMapDims(grid, chunkSize, level, chunkId)
  const edges = chunkEdges(grid, chunkSize, level, chunkId)
  const cell = levelCellSize(grid, level)
  const bounds = chunkBounds(grid, chunkSize, level, chunkId, origin)
  const firstI = ix * side
  const firstJ = iy * side
  return {
    chunkSize: side,
    level,
    chunkId,
    cell,
    inMapColumns: Math.max(1, inMap.width),
    inMapRows: Math.max(1, inMap.height),
    columns: Math.max(1, inMap.width) + (edges.east ? 0 : 1),
    rows: Math.max(1, inMap.height) + (edges.south ? 0 : 1),
    firstI,
    firstJ,
    originX: bounds.min_x,
    originY: bounds.min_y,
    edges,
  }
}

/** Builds the vertex data of one elevation chunk from its cell reader. */
export function buildChunkMesh(
  spec: ChunkGrid,
  sample: CellSampler,
  options: ChunkMeshOptions = {},
): ChunkMeshData {
  const { columns, rows, cell, originX, originY, edges } = spec
  const step = options.terraceM ?? 0
  const heights = new Float32Array(columns * rows)
  for (let row = 0; row < rows; row += 1) {
    for (let column = 0; column < columns; column += 1) {
      heights[row * columns + column] = terrace(cellHeight(spec, sample, column, row), step)
    }
  }

  const positions = new Float32Array(columns * rows * 3)
  const uvs = new Float32Array(columns * rows * 2)
  for (let row = 0; row < rows; row += 1) {
    for (let column = 0; column < columns; column += 1) {
      const vertex = row * columns + column
      positions[vertex * 3] = originX + vertexOffset(column, columns, cell, edges.west, edges.east)
      positions[vertex * 3 + 1] = heights[vertex] ?? 0
      positions[vertex * 3 + 2] = originY + vertexOffset(row, rows, cell, edges.north, edges.south)
      uvs[vertex * 2] = (positions[vertex * 3] ?? 0) / GRAIN_PERIOD_M
      uvs[vertex * 2 + 1] = (positions[vertex * 3 + 2] ?? 0) / GRAIN_PERIOD_M
    }
  }

  const quadsX = columns - 1
  const quadsY = rows - 1
  const indices = new Uint32Array(Math.max(0, quadsX * quadsY * 6))
  let cursor = 0
  for (let row = 0; row < quadsY; row += 1) {
    for (let column = 0; column < quadsX; column += 1) {
      const topLeft = row * columns + column
      const topRight = topLeft + 1
      const bottomLeft = topLeft + columns
      const bottomRight = bottomLeft + 1
      // Babylon culls the side the vertex order's right-hand normal points away from,
      // so this order — which puts that normal downwards — leaves the front face up,
      // toward the height normals and the default camera. Reversing it puts the front
      // face down and hides the whole surface from above.
      indices[cursor] = topLeft
      indices[cursor + 1] = topRight
      indices[cursor + 2] = bottomLeft
      indices[cursor + 3] = topRight
      indices[cursor + 4] = bottomRight
      indices[cursor + 5] = bottomLeft
      cursor += 6
    }
  }

  const normals = heightFieldNormals(positions, columns, rows, cell)
  const range = options.range ?? surfaceRange(heights)
  // The footprints are flattened so the height field under each block is a plateau rather
  // than a mesa; the block itself — its colour and its walls — is separate geometry, built
  // by `render/buildings.ts`, and the terrain stays ground everywhere it is visible.
  const mask = options.building === undefined ? undefined : buildingMask(spec, options.building)
  if (mask !== null && mask !== undefined) {
    flattenFootprints(heights, mask.coverage, columns, rows)
  }
  // Shading is applied here rather than by the material: the ramp spans the surface, not
  // the chunk, so a chunk drawn on its own is not rescaled into a different range than its
  // neighbours.
  const rampInput = new Float32Array(columns * rows * 2)
  for (let row = 0; row < rows; row += 1) {
    for (let column = 0; column < columns; column += 1) {
      const vertex = row * columns + column
      const normal: [number, number, number] = [
        normals[vertex * 3] ?? 0,
        normals[vertex * 3 + 1] ?? 1,
        normals[vertex * 3 + 2] ?? 0,
      ]
      // `reliefShade` is deliberately not here. It was the only directional term the
      // surface had when the scene had no shadow, and the price of baking it per vertex is
      // that it cannot be smooth: the sun's angle is sampled at cell corners on a grid that
      // steps, so a terrace shades as a band of flat facets rather than as a slope. The
      // scene's directional light now does that term per fragment against the same
      // direction, and the shadow map makes it mean something — what is left here is the
      // part a light cannot produce, which is the occlusion of a hollow and the darkening of
      // a face too steep to hold much sky.
      const ground =
        slopeAccent(normal[1]) *
        cavityShade(relativeDepth(heights, columns, rows, column, row, cell))
      rampInput[vertex * 2] = normalize(heights[vertex] ?? 0, range.min, range.max)
      rampInput[vertex * 2 + 1] = ground
    }
  }

  return {
    positions,
    indices,
    normals,
    rampInput,
    uvs,
    grid: { columns, rows },
    slab: buildSlab({
      minX: originX,
      minZ: originY,
      inMapColumns: spec.inMapColumns,
      inMapRows: spec.inMapRows,
      stride: columns,
      cell,
      edges,
      heights,
      floorY: options.slabFloorY ?? range.min,
      range,
    }),
  }
}

/**
 * The global cell a mesh vertex reads from.
 *
 * The grid carries one vertex per in-map cell plus, on an internal side, the neighbour's
 * first cell — that is the vertex the boundary quad needs, and reading the neighbour's own
 * cell is what makes the two chunks agree bit for bit. On the map's rim there is nothing
 * beyond to read, so the last cell repeats itself and the slab wall closes the model.
 */
function cellIndices(spec: ChunkGrid, column: number, row: number): { i: number; j: number } {
  const atRimEast = spec.edges.east && column === spec.columns - 1
  const atRimSouth = spec.edges.south && row === spec.rows - 1
  return {
    i: spec.firstI + (atRimEast ? column - 1 : column),
    j: spec.firstJ + (atRimSouth ? row - 1 : row),
  }
}

/** Height a mesh vertex takes, from the cell under it. */
function cellHeight(spec: ChunkGrid, sample: CellSampler, column: number, row: number): number {
  const { i, j } = cellIndices(spec, column, row)
  return sample(i, j) ?? 0
}

/** The building mask a mesh reads: how much of each cell a footprint covers. */
export interface BuildingMask {
  /**
   * Coverage per vertex, `0` to `1`.
   *
   * The footprint itself. This is what decides *which cells belong to the building*, and so
   * which of them take the block's own flattened height. A map whose elevation layer has no
   * mask channel reads `null` throughout and comes back all zeros.
   */
  coverage: Float32Array
}

export function buildingMask(spec: ChunkGrid, sample: CellSampler): BuildingMask {
  const { columns, rows } = spec
  const coverage = new Float32Array(columns * rows)
  for (let row = 0; row < rows; row += 1) {
    for (let column = 0; column < columns; column += 1) {
      const { i, j } = cellIndices(spec, column, row)
      const value = sample(i, j)
      coverage[row * columns + column] = value === null ? 0 : Math.min(1, Math.max(0, value))
    }
  }
  return { coverage }
}

function flattenFootprints(
  heights: Float32Array,
  coverage: Float32Array,
  columns: number,
  rows: number,
): void {
  const roof = new Float32Array(heights.length)
  for (let row = 0; row < rows; row += 1) {
    for (let column = 0; column < columns; column += 1) {
      let best = heights[row * columns + column] ?? 0
      // A window a little wider than the rim the pyramid leaves, so a cell at the inside
      // edge of a footprint still finds the roof it belongs to rather than its own average.
      for (let dy = -FOOTPRINT_REACH; dy <= FOOTPRINT_REACH; dy += 1) {
        for (let dx = -FOOTPRINT_REACH; dx <= FOOTPRINT_REACH; dx += 1) {
          const x = column + dx
          const y = row + dy
          if (x < 0 || y < 0 || x >= columns || y >= rows) {
            continue
          }
          const at = y * columns + x
          if ((coverage[at] ?? 0) < FOOTPRINT_INTERIOR) {
            continue
          }
          const height = heights[at] ?? 0
          if (height > best) {
            best = height
          }
        }
      }
      roof[row * columns + column] = best
    }
  }
  for (let vertex = 0; vertex < heights.length; vertex += 1) {
    if ((coverage[vertex] ?? 0) >= FOOTPRINT_INTERIOR) {
      heights[vertex] = roof[vertex] ?? heights[vertex] ?? 0
    }
  }
}

/** Coverage at which a cell is part of a footprint rather than beside one. */
const FOOTPRINT_INTERIOR = 0.5

/**
 * Cells the roof search reaches, in cells.
 *
 * Two, because that is the widest rim a single level of the pyramid can leave: an edge cell
 * is an average of building and ground, and one level is all the averaging there is between
 * a level and the one above it.
 */
const FOOTPRINT_REACH = 2

/**
 * Which sides of a chunk are the map's rim.
 *
 * A side is the rim when the map's cells end there: the chunk's own cells run out before
 * its full extent, or it is the last chunk of the level's grid.
 */
export function chunkEdges(
  grid: LayerGrid,
  chunkSize: number,
  level: number,
  chunkId: number,
): ChunkEdges {
  const { ix, iy } = mortonDecodeChunk(chunkId)
  const dims = inMapDims(grid, chunkSize, level, chunkId)
  const side = Math.max(1, chunkSize)
  const mapWidth = grid.level_dims[level]?.[0] ?? dims.width + ix * side
  const mapHeight = grid.level_dims[level]?.[1] ?? dims.height + iy * side
  return {
    west: ix === 0,
    north: iy === 0,
    east: ix * side + dims.width >= mapWidth,
    south: iy * side + dims.height >= mapHeight,
  }
}

/**
 * World coordinate of one vertex along an axis.
 *
 * The centre of cell `index` is at `(index + 0.5) * cell`; on the map's rim the last cell
 * is placed on the map's edge instead, which is what closes the half-cell ring the
 * cell-centre convention would otherwise leave uncovered.
 */
function vertexOffset(
  index: number,
  count: number,
  cell: number,
  atStart: boolean,
  atEnd: boolean,
): number {
  if (atStart && index === 0) {
    return 0
  }
  if (atEnd && index === count - 1) {
    return count * cell
  }
  return (index + 0.5) * cell
}

/**
 * How far below its surroundings a cell stands, in cell heights.
 *
 * Positive in a hollow and negative on a ridge, which is what {@link cavityShade} turns
 * into occlusion. Neighbours outside the grid read as the cell itself: inventing a value
 * would put a false shadow line along a boundary.
 */
function relativeDepth(
  heights: Float32Array,
  columns: number,
  rows: number,
  column: number,
  row: number,
  cell: number,
): number {
  const own = heights[row * columns + column] ?? 0
  const left = heights[row * columns + Math.max(0, column - 1)] ?? own
  const right = heights[row * columns + Math.min(columns - 1, column + 1)] ?? own
  const up = heights[Math.max(0, row - 1) * columns + column] ?? own
  const down = heights[Math.min(rows - 1, row + 1) * columns + column] ?? own
  const mean = (left + right + up + down) / 4
  return (mean - own) / Math.max(cell, 1e-6)
}

/** Input of {@link buildSlab}. */
interface SlabInput {
  /** World x of the chunk's first cell centre. */
  minX: number
  /** World z of the chunk's first cell centre. */
  minZ: number
  /** In-map columns of the chunk, which the floor covers. */
  inMapColumns: number
  /** In-map rows of the chunk. */
  inMapRows: number
  /** Row stride of `heights`, which includes the shore vertices. */
  stride: number
  /** Cell size, metres. */
  cell: number
  /** Which sides are the map's rim. */
  edges: ChunkEdges
  /** Quantised surface heights, in `[row][column]` order over the mesh grid. */
  heights: Float32Array
  /** World y of the slab's underside. */
  floorY: number
  /** Elevation range the wall heights are normalised against. */
  range: { min: number; max: number }
}

/**
 * Builds the slab under one chunk: a wall along every rim side, plus the floor.
 *
 * The walls start at the surface's own rim vertices, so there is no gap between the top of
 * the model and its side; the floor is one quad over the chunk's in-map footprint, and
 * since chunks tile the map their floors tile it too.
 */
function buildSlab(input: SlabInput): MeshData {
  const { inMapColumns, inMapRows, stride, cell, edges, heights, floorY, range } = input
  const positions: number[] = []
  const indices: number[] = []
  const normals: number[] = []
  const rampInput: number[] = []

  const uvs: number[] = []
  const pushVertex = (
    x: number,
    y: number,
    z: number,
    normal: [number, number, number],
    height: number,
    shade: number,
  ): number => {
    positions.push(x, y, z)
    normals.push(normal[0], normal[1], normal[2])
    rampInput.push(normalize(height, range.min, range.max), shade)
    uvs.push(x / GRAIN_PERIOD_M, z / GRAIN_PERIOD_M)
    return positions.length / 3 - 1
  }
  // A wall is the boundary of the chunk's own *footprint*, so it runs on cell corners from one
  // edge of the footprint to the other. Stopping at the last cell's centre instead — which is
  // where the surface's rim vertex sits — left a half-cell slit in the side of the model at
  // every chunk boundary: the slabs of two neighbouring chunks did not meet.
  const clampedColumn = (column: number): number => Math.min(column, inMapColumns - 1)
  const clampedRow = (row: number): number => Math.min(row, inMapRows - 1)
  const heightAt = (column: number, row: number): number =>
    heights[clampedRow(row) * stride + clampedColumn(column)] ?? 0
  const worldX = (column: number): number => input.minX + column * cell
  const worldZ = (row: number): number => input.minZ + row * cell

  // One wall per rim side: every pair of neighbouring rim vertices, dropped to the floor.
  // A side that is not the map's rim has the next chunk's surface continuing the ground,
  // so no wall belongs there.
  const sides: Array<{
    normal: [number, number, number]
    pairs: Array<[number, number, number, number]>
  }> = []
  if (edges.west) {
    sides.push({ normal: [-1, 0, 0], pairs: columnPairs(0, inMapRows) })
  }
  if (edges.east) {
    sides.push({ normal: [1, 0, 0], pairs: columnPairs(inMapColumns, inMapRows) })
  }
  if (edges.north) {
    sides.push({ normal: [0, 0, -1], pairs: rowPairs(0, inMapColumns) })
  }
  if (edges.south) {
    sides.push({ normal: [0, 0, 1], pairs: rowPairs(inMapRows, inMapColumns) })
  }
  for (const side of sides) {
    for (const [columnA, rowA, columnB, rowB] of side.pairs) {
      // Both ends of a wall take the height of the surface above them, so a wall reads as
      // the side of the block it belongs to rather than as a gradient of its own.
      const a = pushVertex(
        worldX(columnA),
        heightAt(columnA, rowA),
        worldZ(rowA),
        side.normal,
        heightAt(columnA, rowA),
        SLAB_WALL_SHADE,
      )
      const b = pushVertex(
        worldX(columnB),
        heightAt(columnB, rowB),
        worldZ(rowB),
        side.normal,
        heightAt(columnB, rowB),
        SLAB_WALL_SHADE,
      )
      const c = pushVertex(
        worldX(columnB),
        floorY,
        worldZ(rowB),
        side.normal,
        heightAt(columnB, rowB),
        SLAB_WALL_SHADE,
      )
      const d = pushVertex(
        worldX(columnA),
        floorY,
        worldZ(rowA),
        side.normal,
        heightAt(columnA, rowA),
        SLAB_WALL_SHADE,
      )
      indices.push(a, b, c, a, c, d)
    }
  }

  // The floor: one quad over the chunk's own footprint, facing down.
  const minX = input.minX
  const minZ = input.minZ
  const spanX = inMapColumns * cell
  const spanZ = inMapRows * cell
  const floorNormal: [number, number, number] = [0, -1, 0]
  const floorHeight = heightAt(0, 0)
  const floorA = pushVertex(minX, floorY, minZ, floorNormal, floorHeight, SLAB_FLOOR_SHADE)
  const floorB = pushVertex(minX + spanX, floorY, minZ, floorNormal, floorHeight, SLAB_FLOOR_SHADE)
  const floorC = pushVertex(
    minX + spanX,
    floorY,
    minZ + spanZ,
    floorNormal,
    floorHeight,
    SLAB_FLOOR_SHADE,
  )
  const floorD = pushVertex(minX, floorY, minZ + spanZ, floorNormal, floorHeight, SLAB_FLOOR_SHADE)
  indices.push(floorA, floorC, floorB, floorA, floorD, floorC)

  return {
    positions: new Float32Array(positions),
    indices: new Uint32Array(indices),
    normals: new Float32Array(normals),
    rampInput: new Float32Array(rampInput),
    uvs: new Float32Array(uvs),
  }
}

/**
 * Vertical pairs along a column of the wall: `[column, row, column, row + 1]`.
 *
 * Indices are *corners* of the chunk's footprint, so a wall of `rows` cells has `rows + 1`
 * corners and reaches the boundary on both ends.
 */
function columnPairs(column: number, rows: number): Array<[number, number, number, number]> {
  const pairs: Array<[number, number, number, number]> = []
  for (let row = 0; row <= rows; row += 1) {
    pairs.push([column, row, column, row + 1])
  }
  return pairs
}

/** Horizontal pairs along a row of the wall: `[column, row, column + 1, row]`. */
function rowPairs(row: number, columns: number): Array<[number, number, number, number]> {
  const pairs: Array<[number, number, number, number]> = []
  for (let column = 0; column <= columns; column += 1) {
    pairs.push([column, row, column + 1, row])
  }
  return pairs
}

/**
 * Normals of a height field, from the difference to the neighbouring vertices.
 *
 * Edge vertices repeat their inward neighbour's difference, which is cheaper than a normal
 * of fewer vertices and avoids a black seam on the chunk border.
 */
export function heightFieldNormals(
  positions: Float32Array,
  columns: number,
  rows: number,
  cell: number,
): Float32Array {
  const normals = new Float32Array(columns * rows * 3)
  const step = Math.max(cell, 1e-6)
  const at = (column: number, row: number): number =>
    positions[(row * columns + column) * 3 + 1] ?? 0
  for (let row = 0; row < rows; row += 1) {
    for (let column = 0; column < columns; column += 1) {
      const left = Math.max(0, column - 1)
      const right = Math.min(columns - 1, column + 1)
      const up = Math.max(0, row - 1)
      const down = Math.min(rows - 1, row + 1)
      const dhdx = (at(right, row) - at(left, row)) / (step * (right - left || 1))
      const dhdz = (at(column, down) - at(column, up)) / (step * (down - up || 1))
      // A height field's normal is (-dh/dx, 1, -dh/dz) before normalisation.
      const length = Math.hypot(dhdx, 1, dhdz)
      const vertex = (row * columns + column) * 3
      normals[vertex] = -dhdx / length
      normals[vertex + 1] = 1 / length
      normals[vertex + 2] = -dhdz / length
    }
  }
  return normals
}

/** Elevation range of a set of samples, metres. */
function surfaceRange(heights: Float32Array): { min: number; max: number } {
  let min = Number.POSITIVE_INFINITY
  let max = Number.NEGATIVE_INFINITY
  for (const value of heights) {
    min = Math.min(min, value)
    max = Math.max(max, value)
  }
  if (min === Number.POSITIVE_INFINITY) {
    return { min: 0, max: 1 }
  }
  return max - min < 1e-6 ? { min, max: min + 1 } : { min, max }
}

/** Elevation of a mesh vertex grid, in metres; used to frame the camera and to drape layers. */
export function elevationRange(mesh: ChunkMeshData): { min: number; max: number } {
  let min = Number.POSITIVE_INFINITY
  let max = Number.NEGATIVE_INFINITY
  for (let index = 1; index < mesh.positions.length; index += 3) {
    const value = mesh.positions[index] ?? 0
    min = Math.min(min, value)
    max = Math.max(max, value)
  }
  return min === Number.POSITIVE_INFINITY ? { min: 0, max: 0 } : { min, max }
}

/**
 * The height the surface *draws* at a map position, from the meshes already built.
 *
 * The drawn mesh differs from the raw cells in two ways a reader can see a line cross:
 * terracing quantises every height to a step, and a building footprint is flattened to its
 * roof — so a track draped on raw cells dives under the drawn roof and vanishes. Reading
 * the mesh's own vertex grid and interpolating across its quads gives the height the
 * reader's eye compares against.
 *
 * The parts tile the map, so the sampler walks them per query; a caller that redraws on
 * chunk arrival rebuilds the sampler the same way it rebuilds its mesh cache.
 */
export function drawnSurfaceSampler(
  parts: Iterable<{ spec: ChunkGrid; mesh: MeshData }>,
): (x: number, y: number) => number | null {
  const fields = [...parts]
  return (x: number, y: number): number | null => {
    for (const { spec, mesh } of fields) {
      const { columns, rows, cell, originX, originY } = spec
      const fx = (x - originX) / cell - 0.5
      const fy = (y - originY) / cell - 0.5
      if (fx < -0.5 || fy < -0.5 || fx > columns - 0.5 || fy > rows - 0.5) {
        continue
      }
      const i0 = Math.min(columns - 1, Math.max(0, Math.floor(fx)))
      const j0 = Math.min(rows - 1, Math.max(0, Math.floor(fy)))
      const i1 = Math.min(columns - 1, i0 + 1)
      const j1 = Math.min(rows - 1, j0 + 1)
      const tx = Math.min(1, Math.max(0, fx - i0))
      const ty = Math.min(1, Math.max(0, fy - j0))
      const at = (column: number, row: number): number =>
        mesh.positions[(row * columns + column) * 3 + 1] ?? 0
      const top = at(i0, j0) * (1 - tx) + at(i1, j0) * tx
      const bottom = at(i0, j1) * (1 - tx) + at(i1, j1) * tx
      const height = top * (1 - ty) + bottom * ty
      return Number.isFinite(height) ? height : null
    }
    return null
  }
}
