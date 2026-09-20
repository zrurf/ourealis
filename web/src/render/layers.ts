/*
 * Feature layers as textures.
 *
 * A raster layer is drawn as one RGBA texture per chunk, draped over the
 * elevation: the mesh reuses the terrain's own vertex heights so the overlay
 * follows the ground instead of floating over it. Category-shaped data takes the
 * discrete palette (with an alpha that hides zero when the layer is sparse) and
 * scalar data takes a ramp, which is what the layer panel offers as
 * category / grey / speed / height.
 *
 * The pixel conversion is a pure function so the mapping can be tested without a
 * GPU, and so the same numbers feed a legend.
 */
import { StandardMaterial } from '@babylonjs/core/Materials/standardMaterial'
import { RawTexture } from '@babylonjs/core/Materials/Textures/rawTexture'
import { Color3 } from '@babylonjs/core/Maths/math.color'
import { Mesh } from '@babylonjs/core/Meshes/mesh'
import { VertexData } from '@babylonjs/core/Meshes/mesh.vertexData'
import type { Scene } from '@babylonjs/core/scene'
import type { ColorMapping, DecodedChunk } from '@/types/map'
import {
  CATEGORY_PALETTE,
  SCALAR_RAMPS,
  colorForCategory,
  normalize,
  rampAt,
  valueRange,
} from '@/types/colormap'

/** RGBA texture of one layer chunk, in the chunk's own `[y][x]` order. */
export interface LayerTextureData {
  /** Pixels, four bytes per cell, row-major from the chunk's first row. */
  pixels: Uint8ClampedArray
  /** Texture width in cells. */
  width: number
  /** Texture height in cells. */
  height: number
}

/** Options of {@link layerTextureData}. */
export interface LayerTextureOptions {
  /** How the samples are coloured. */
  mapping: ColorMapping
  /** Value range the ramp spans; computed from the samples when absent. */
  range?: { min: number; max: number }
  /** Alpha of a cell whose value is missing or zero; `0` hides it, `1` fills the mesh. */
  emptyAlpha?: number
}

/**
 * Converts one layer chunk into RGBA bytes.
 *
 * Only the first channel is shown: a multi-channel raster holds one channel per
 * preference dimension, and a viewer draws one at a time rather than inventing a
 * blend. A sparse layer's absent cells are stored as zero and are left
 * transparent, which is what keeps a mask readable over the terrain.
 */
export function layerTextureData(
  chunk: DecodedChunk,
  options: LayerTextureOptions,
): LayerTextureData {
  const width = Math.max(1, chunk.width)
  const height = Math.max(1, chunk.height)
  const pixels = new Uint8ClampedArray(width * height * 4)
  const emptyAlpha = options.emptyAlpha ?? 0
  const range = options.range ?? valueRange(chunk.values)
  const ramp = options.mapping === 'category' ? CATEGORY_PALETTE : SCALAR_RAMPS[options.mapping]
  for (let row = 0; row < height; row += 1) {
    for (let column = 0; column < width; column += 1) {
      const index = (row * width + column) * chunk.channels
      const raw = chunk.values[index]
      const pixel = (row * width + column) * 4
      const missing = raw === undefined || !Number.isFinite(raw)
      const color =
        missing || raw === 0
          ? null
          : options.mapping === 'category'
            ? colorForCategory(raw)
            : rampAt(ramp, normalize(raw, range.min, range.max))
      pixels[pixel] = color?.[0] ?? 0
      pixels[pixel + 1] = color?.[1] ?? 0
      pixels[pixel + 2] = color?.[2] ?? 0
      pixels[pixel + 3] = color === null ? Math.round(emptyAlpha * 255) : 255
    }
  }
  return { pixels, width, height }
}

/** One layer's drape over the terrain, one mesh per loaded chunk. */
export class LayerOverlay {
  private readonly scene: Scene
  private readonly meshes = new Map<string, Mesh>()
  private readonly textures = new Map<string, RawTexture>()

  constructor(scene: Scene) {
    this.scene = scene
  }

  /** Number of chunks currently draped. */
  get size(): number {
    return this.meshes.size
  }

  /** Replaces the drape of one chunk. */
  setChunk(key: string, texture: LayerTextureData, mesh: DrapedMeshData): void {
    this.drop(key)
    const raw = RawTexture.CreateRGBATexture(
      texture.pixels,
      texture.width,
      texture.height,
      this.scene,
      false,
      false,
      RawTexture.NEAREST_NEAREST,
    )
    raw.wrapU = RawTexture.CLAMP_ADDRESSMODE
    raw.wrapV = RawTexture.CLAMP_ADDRESSMODE
    const drape = new Mesh(`layer:${key}`, this.scene)
    const vertexData = new VertexData()
    vertexData.positions = mesh.positions
    vertexData.indices = mesh.indices
    vertexData.uvs = mesh.uvs
    vertexData.normals = mesh.normals
    vertexData.applyToMesh(drape, false)
    // One material per chunk: a texture belongs to a material, and sharing one
    // would make every drape show the last chunk's pixels.
    const material = new StandardMaterial(`layerMaterial:${key}`, this.scene)
    material.disableLighting = true
    material.emissiveColor = new Color3(1, 1, 1)
    material.specularColor = new Color3(0, 0, 0)
    material.useAlphaFromDiffuseTexture = true
    material.transparencyMode = StandardMaterial.MATERIAL_ALPHABLEND
    material.zOffset = -2
    material.diffuseTexture = raw
    material.emissiveTexture = raw
    drape.material = material
    drape.isPickable = false
    this.textures.set(key, raw)
    this.meshes.set(key, drape)
  }

  /** Removes one chunk's drape and the material it owned. */
  drop(key: string): void {
    const mesh = this.meshes.get(key)
    mesh?.material?.dispose()
    mesh?.dispose()
    this.textures.get(key)?.dispose()
    this.meshes.delete(key)
    this.textures.delete(key)
  }

  /** Removes every drape; switching layers or levels rebuilds them. */
  clear(): void {
    // A copy is required: `drop` removes from the map being iterated.
    for (const key of Array.from(this.meshes.keys())) {
      this.drop(key)
    }
  }

  /** Disposes the drapes of every chunk. */
  dispose(): void {
    this.clear()
  }
}

/** Geometry of one draped chunk: the terrain's surface with the layer's texture coordinates. */
export interface DrapedMeshData {
  /** Vertex positions, `x, y, z`, taking the terrain's heights. */
  positions: Float32Array
  /** Triangle indices. */
  indices: Uint32Array
  /** Texture coordinates, `u, v` per vertex. */
  uvs: Float32Array
  /** Vertex normals, copied from the terrain so the drape shades like it. */
  normals: Float32Array
}

/** Builds the drape of one chunk from the terrain mesh it covers. */
export function drapedMesh(
  terrain: {
    positions: Float32Array
    indices: Uint32Array
    normals: Float32Array
    grid: { columns: number; rows: number }
  },
  /**
   * Height above the surface, metres.
   *
   * Drapes share the terrain's own vertices, so two of them enabled at once would
   * occupy the same depth and flicker between each other. Each layer is raised by a
   * small multiple of this instead, which is invisible at map scale.
   */
  liftM: number = 0,
): DrapedMeshData {
  const { columns, rows } = terrain.grid
  const uvs = new Float32Array(columns * rows * 2)
  for (let row = 0; row < rows; row += 1) {
    for (let column = 0; column < columns; column += 1) {
      const vertex = row * columns + column
      uvs[vertex * 2] = columns === 1 ? 0 : column / (columns - 1)
      uvs[vertex * 2 + 1] = rows === 1 ? 0 : 1 - row / (rows - 1)
    }
  }
  let positions = terrain.positions
  if (liftM !== 0) {
    positions = new Float32Array(terrain.positions)
    // The terrain is built with y up, so the lift is the second component.
    for (let vertex = 1; vertex < positions.length; vertex += 3) {
      positions[vertex] = (positions[vertex] ?? 0) + liftM
    }
  }
  return {
    positions,
    indices: terrain.indices,
    uvs,
    normals: terrain.normals,
  }
}

/** CSS colour of one value under a mapping, for a legend swatch. */
export function legendColor(
  mapping: ColorMapping,
  value: number,
  range: { min: number; max: number },
): string {
  const color =
    mapping === 'category'
      ? colorForCategory(value)
      : rampAt(SCALAR_RAMPS[mapping], normalize(value, range.min, range.max))
  return `rgb(${color[0]} ${color[1]} ${color[2]})`
}
