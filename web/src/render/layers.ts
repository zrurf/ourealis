/*
 * Feature layers as drapes.
 *
 * The thin part: this uploads the pixels `render/layerTexture.ts` produces and builds the
 * mesh that carries them. One material per chunk, because a texture belongs to a material
 * and sharing one would make every drape show the last chunk's pixels.
 */
import { StandardMaterial } from '@babylonjs/core/Materials/standardMaterial'
import { RawTexture } from '@babylonjs/core/Materials/Textures/rawTexture'
import { Color3 } from '@babylonjs/core/Maths/math.color'
import { Mesh } from '@babylonjs/core/Meshes/mesh'
import { VertexData } from '@babylonjs/core/Meshes/mesh.vertexData'
import type { Scene } from '@babylonjs/core/scene'
import type { DrapedMeshData, LayerTextureData } from './layerTexture'

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
    // A raw texture does not say whether it carries transparency; the material has to be
    // told, or a drape whose empty cells are transparent is drawn as an opaque sheet.
    raw.hasAlpha = texture.hasAlpha
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
