/*
 * Feature layers as drapes.
 *
 * The thin part: this uploads the pixels `render/layerTexture.ts` produces and builds the
 * mesh that carries them. One material per chunk, because a texture belongs to a material
 * and sharing one would make every drape show the last chunk's pixels.
 *
 * A drape is unlit — it shows the data's own colours — so the light/dark appearance cannot
 * reach it through the scene's lights or background: a map whose cells are all painted is
 * covered by the drape, and a switch that moved only the background moved nothing the
 * reader could see. The appearance is therefore carried on the drape itself, as a vertex
 * colour the layer rewrites in place, the way the terrain carries its ramp.
 */
import { VertexBuffer } from '@babylonjs/core/Buffers/buffer'
import { StandardMaterial } from '@babylonjs/core/Materials/standardMaterial'
import { RawTexture } from '@babylonjs/core/Materials/Textures/rawTexture'
import { Color3 } from '@babylonjs/core/Maths/math.color'
import { Mesh } from '@babylonjs/core/Meshes/mesh'
import { VertexData } from '@babylonjs/core/Meshes/mesh.vertexData'
import type { Scene } from '@babylonjs/core/scene'
import type { DrapedMeshData, LayerTextureData } from './layerTexture'
import type { MapScene } from './scene'

/** Fraction of its natural brightness a painted drape keeps in the dark appearance. */
const DARK_TINT = 0.62

/**
 * Anisotropic samples taken along the view direction.
 *
 * The ground is read at a glancing angle from a raised camera, where an isotropic filter
 * samples too coarse a mip and smears the road and surface edges along the view; the
 * engine clamps this to what the device supports.
 */
const DRAPE_ANISOTROPY = 8

export {
  drapedMesh,
  layerTextureData,
  legendColor,
  sampleColour,
  type DrapedMeshData,
  type LayerTextureData,
  type LayerTextureOptions,
} from './layerTexture'

/** One layer's drape over the terrain, one mesh per loaded chunk. */
export class LayerOverlay {
  private readonly scene: Scene
  private readonly meshes = new Map<string, Mesh>()
  private readonly textures = new Map<string, RawTexture>()
  /** Vertices per chunk, so an appearance change can rewrite one chunk's tint alone. */
  private readonly vertexCounts = new Map<string, number>()
  private tint: number
  private readonly unbindAppearance: () => void

  constructor(scene: Scene, mapScene: MapScene) {
    this.scene = scene
    this.tint = tintFor(mapScene.isDark)
    this.unbindAppearance = mapScene.onAppearance((dark) => {
      this.setTint(tintFor(dark))
    })
  }

  /**
   * Sets the brightness every drape is drawn at; only the colour buffers are rewritten.
   *
   * The chunks already drawn are recoloured here and the ones that arrive later are tinted
   * on arrival, so a chunk that streams in after the switch is not the one bright island
   * left on a dark map.
   */
  setTint(tint: number): void {
    if (tint === this.tint) {
      return
    }
    this.tint = tint
    for (const [key, drape] of this.meshes) {
      const count = this.vertexCounts.get(key) ?? 0
      if (count > 0) {
        drape.updateVerticesData(VertexBuffer.ColorKind, tintColours(count, tint))
      }
    }
  }

  /** Replaces the drape of one chunk. */
  setChunk(key: string, texture: LayerTextureData, mesh: DrapedMeshData): void {
    this.drop(key)
    // Mipmapped and linearly filtered: a pattern sampled per texel aliases badly on a
    // distant drape, and the average of a pattern is its colour.
    const raw = RawTexture.CreateRGBATexture(
      texture.pixels,
      texture.width,
      texture.height,
      this.scene,
      true,
      false,
      RawTexture.TRILINEAR_SAMPLINGMODE,
    )
    raw.wrapU = RawTexture.CLAMP_ADDRESSMODE
    raw.wrapV = RawTexture.CLAMP_ADDRESSMODE
    raw.anisotropicFilteringLevel = DRAPE_ANISOTROPY
    // A raw texture does not say whether it carries transparency; the material has to be
    // told, or a drape whose empty cells are transparent is drawn as an opaque sheet.
    raw.hasAlpha = texture.hasAlpha
    const drape = new Mesh(`layer:${key}`, this.scene)
    const vertexCount = mesh.positions.length / 3
    const vertexData = new VertexData()
    vertexData.positions = mesh.positions
    vertexData.indices = mesh.indices
    vertexData.uvs = mesh.uvs
    vertexData.normals = mesh.normals
    vertexData.colors = tintColours(vertexCount, this.tint)
    // Updatable: an appearance change rewrites the colour buffer in place rather than
    // rebuilding a drape whose geometry and pixels did not change.
    vertexData.applyToMesh(drape, true)
    // One material per chunk: a texture belongs to a material, and sharing one
    // would make every drape show the last chunk's pixels.
    const material = new StandardMaterial(`layerMaterial:${key}`, this.scene)
    material.disableLighting = true
    material.emissiveColor = new Color3(1, 1, 1)
    material.specularColor = new Color3(0, 0, 0)
    material.useAlphaFromDiffuseTexture = texture.hasAlpha
    // Opaque where every cell is painted: blending a solid layer costs fill rate and
    // orders it against the terrain for nothing.
    material.transparencyMode = texture.hasAlpha
      ? StandardMaterial.MATERIAL_ALPHABLEND
      : StandardMaterial.MATERIAL_OPAQUE
    material.zOffset = -2
    material.diffuseTexture = raw
    material.emissiveTexture = raw
    drape.material = material
    drape.isPickable = false
    this.textures.set(key, raw)
    this.meshes.set(key, drape)
    this.vertexCounts.set(key, vertexCount)
  }

  /** Removes one chunk's drape and the material it owned. */
  drop(key: string): void {
    const mesh = this.meshes.get(key)
    mesh?.material?.dispose()
    mesh?.dispose()
    this.textures.get(key)?.dispose()
    this.meshes.delete(key)
    this.textures.delete(key)
    this.vertexCounts.delete(key)
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
    this.unbindAppearance()
    this.clear()
  }
}

/** Brightness of the drapes for one appearance. */
function tintFor(dark: boolean): number {
  return dark ? DARK_TINT : 1
}

/** `count` vertices of one grey, as a vertex colour buffer. */
function tintColours(count: number, tint: number): Float32Array {
  const colours = new Float32Array(count * 4)
  for (let index = 0; index < colours.length; index += 4) {
    colours[index] = tint
    colours[index + 1] = tint
    colours[index + 2] = tint
    colours[index + 3] = 1
  }
  return colours
}
