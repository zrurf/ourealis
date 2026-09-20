/*
 * Elevation chunks as meshes.
 *
 * One mesh per `(level, chunk)` pair, built straight from the decoded samples, so
 * a chunk that arrives late is added beside the ones already drawn rather than
 * triggering a rebuild of the surface. Levels are chosen by the caller
 * (`selectLevel` in `types/map.ts`), which is what makes a coarse level show
 * first and finer levels fill in.
 *
 * The geometry is built by pure functions that return typed arrays; the class at
 * the bottom is the thin part that hands them to Babylon.
 */
import { StandardMaterial } from '@babylonjs/core/Materials/standardMaterial'
import { Color3 } from '@babylonjs/core/Maths/math.color'
import { Mesh } from '@babylonjs/core/Meshes/mesh'
import { VertexData } from '@babylonjs/core/Meshes/mesh.vertexData'
import type { Scene } from '@babylonjs/core/scene'
import { HEIGHT_RAMP, normalize, rampAt, valueRange } from '@/types/colormap'
import { chunkBounds, levelCellSize, type DecodedChunk } from '@/types/map'
import type { LayerGrid } from '@/api/types'
import type { MapScene } from './scene'

/** Vertex data of one chunk mesh, in world metres with the elevation in y. */
export interface ChunkMeshData {
  /** Vertex positions, `x, y, z` per vertex. */
  positions: Float32Array
  /** Triangle indices. */
  indices: Uint32Array
  /** Vertex normals. */
  normals: Float32Array
  /** Vertex colours, `r, g, b, a` per vertex, from the elevation ramp. */
  colors: Float32Array
  /** Grid dimensions the vertices were laid out on, `(columns, rows)`. */
  grid: { columns: number; rows: number }
}

/** Options of {@link buildChunkMesh}. */
export interface ChunkMeshOptions {
  /** Origin of the map's local plane, which the chunk offsets are relative to. */
  origin?: { x: number; y: number }
  /** Vertical offset of the whole surface, metres. */
  baseY?: number
  /** Colour ramp for the elevation; the shared height ramp applies by default. */
  ramp?: readonly (readonly [number, number, number])[]
}

/**
 * Builds the mesh of one elevation chunk.
 *
 * A chunk of `W × H` cells becomes `(W + 1) × (H + 1)` vertices at the cell
 * corners, each taking the elevation of the nearest stored cell. Corners are used
 * rather than cell centres so two adjacent chunks share an edge line instead of
 * leaving a half-cell gap; the two sides of that edge sample different cells, so a
 * faint step can remain at a chunk boundary, which is the price of not carrying a
 * one-cell apron through the whole grid.
 */
export function buildChunkMesh(
  chunk: DecodedChunk,
  grid: LayerGrid,
  chunkSize: number,
  options: ChunkMeshOptions = {},
): ChunkMeshData {
  const columns = chunk.width + 1
  const rows = chunk.height + 1
  const cell = levelCellSize(grid, chunk.level)
  const bounds = chunkBounds(grid, chunkSize, chunk.level, chunk.chunkId, options.origin)
  const channel = 0
  const heights = new Float32Array(columns * rows)
  for (let row = 0; row < rows; row += 1) {
    for (let column = 0; column < columns; column += 1) {
      // Clamping the sample cell keeps the border vertices on the last stored cell.
      const sourceX = Math.min(chunk.width - 1, column)
      const sourceY = Math.min(chunk.height - 1, row)
      const index = (sourceY * chunk.width + sourceX) * chunk.channels + channel
      heights[row * columns + column] = chunk.values[index] ?? 0
    }
  }

  const positions = new Float32Array(columns * rows * 3)
  const baseY = options.baseY ?? 0
  for (let row = 0; row < rows; row += 1) {
    for (let column = 0; column < columns; column += 1) {
      const vertex = row * columns + column
      positions[vertex * 3] = bounds.min_x + column * cell
      positions[vertex * 3 + 1] = baseY + (heights[vertex] ?? 0)
      positions[vertex * 3 + 2] = bounds.min_y + row * cell
    }
  }

  const indices = new Uint32Array(chunk.width * chunk.height * 6)
  let cursor = 0
  for (let row = 0; row < chunk.height; row += 1) {
    for (let column = 0; column < chunk.width; column += 1) {
      const topLeft = row * columns + column
      const topRight = topLeft + 1
      const bottomLeft = topLeft + columns
      const bottomRight = bottomLeft + 1
      // Babylon culls the side that the vertex order's right-hand normal points away
      // from, so this order — which puts that normal downwards — leaves the front
      // face up, toward the height normals and the default camera. Reversing it puts
      // the front face down and hides the whole surface from above.
      indices[cursor] = topLeft
      indices[cursor + 1] = topRight
      indices[cursor + 2] = bottomLeft
      indices[cursor + 3] = topRight
      indices[cursor + 4] = bottomRight
      indices[cursor + 5] = bottomLeft
      cursor += 6
    }
  }

  const range = valueRange(heights)
  const ramp = options.ramp ?? HEIGHT_RAMP
  const colors = new Float32Array(columns * rows * 4)
  for (let vertex = 0; vertex < columns * rows; vertex += 1) {
    const height = heights[vertex] ?? 0
    const color = rampAt(ramp, normalize(height, range.min, range.max))
    colors[vertex * 4] = color[0] / 255
    colors[vertex * 4 + 1] = color[1] / 255
    colors[vertex * 4 + 2] = color[2] / 255
    colors[vertex * 4 + 3] = 1
  }

  return {
    positions,
    indices,
    normals: heightFieldNormals(positions, columns, rows, cell),
    colors,
    grid: { columns, rows },
  }
}

/**
 * Normals of a height field, from the difference to the neighbouring vertices.
 *
 * Edge vertices repeat their inward neighbour's difference, which is cheaper than
 * a normal of fewer vertices and avoids a black seam on the chunk border.
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

/** The elevation surface of one map, one mesh per loaded chunk. */
export class TerrainLayer {
  private readonly scene: Scene
  private readonly mapScene: MapScene
  private readonly material: StandardMaterial
  private readonly meshes = new Map<string, Mesh>()

  constructor(scene: Scene, mapScene: MapScene) {
    this.scene = scene
    this.mapScene = mapScene
    this.material = new StandardMaterial('terrainMaterial', scene)
    this.material.specularColor = new Color3(0, 0, 0)
    this.material.diffuseColor = new Color3(1, 1, 1)
    this.material.backFaceCulling = true
  }

  /** Adds or replaces the mesh of one chunk. */
  setChunk(key: string, data: ChunkMeshData): Mesh {
    this.meshes.get(key)?.dispose()
    const mesh = new Mesh(`terrain:${key}`, this.scene)
    const vertexData = new VertexData()
    vertexData.positions = data.positions
    vertexData.indices = data.indices
    vertexData.normals = data.normals
    vertexData.colors = data.colors
    vertexData.applyToMesh(mesh, false)
    mesh.material = this.material
    mesh.isPickable = true
    this.mapScene.trackHeightMesh(mesh)
    this.meshes.set(key, mesh)
    return mesh
  }

  /** Shows or hides the whole surface without dropping its meshes. */
  setVisible(visible: boolean): void {
    for (const mesh of this.meshes.values()) {
      mesh.setEnabled(visible)
    }
  }

  /** True when the chunk already has a mesh. */
  has(key: string): boolean {
    return this.meshes.has(key)
  }

  /** Meshes currently drawn, for picking and for a bounds fit. */
  get list(): Mesh[] {
    return [...this.meshes.values()]
  }

  /** Removes every mesh; a level change or a map change rebuilds from scratch. */
  clear(): void {
    for (const mesh of this.meshes.values()) {
      this.mapScene.untrackHeightMesh(mesh)
      mesh.dispose()
    }
    this.meshes.clear()
  }

  /** Disposes the meshes and the material. */
  dispose(): void {
    this.clear()
    this.material.dispose()
  }
}
