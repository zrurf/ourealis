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
  private readonly mapScene: MapScene
  private readonly meshes = new Map<string, Mesh>()
  private readonly textures = new Map<string, RawTexture>()
  /** Vertices per chunk, so an appearance change can rewrite one chunk's tint alone. */
  private readonly vertexCounts = new Map<string, number>()
  private tint: number
  private readonly unbindAppearance: () => void

  constructor(scene: Scene, mapScene: MapScene) {
    this.scene = scene
    this.mapScene = mapScene
    this.tint = tintFor(mapScene.isDark)
    this.unbindAppearance = mapScene.onAppearance((dark) => {
      this.setTint(tintFor(dark))
    })
  }

  /**
   * Sets the brightness every drape is drawn at.
   *
   * On the material rather than in the vertex colours, which is where it used to live. The
   * drape is a lit surface now and it carries no vertex colours at all — tinting those was
   * how a dark page dimmed the map, and leaving the tint there while turning the colours
   * off made the map the same brightness in both appearances: a white sheet under a dark
   * frame. The material's own colour is what a lit surface has instead.
   */
  setTint(tint: number): void {
    this.mapScene.invalidate()
    if (tint === this.tint) {
      return
    }
    this.tint = tint
    for (const drape of this.meshes.values()) {
      const material = drape.material
      if (material instanceof StandardMaterial) {
        material.diffuseColor = new Color3(tint, tint, tint)
      }
    }
  }

  /** Replaces the drape of one chunk. */
  setChunk(key: string, texture: LayerTextureData, mesh: DrapedMeshData): void {
    this.mapScene.invalidate()
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
    // Updatable: an appearance change rewrites the colour buffer in place rather than
    // rebuilding a drape whose geometry and pixels did not change.
    vertexData.applyToMesh(drape, true)
    // One material per chunk: a texture belongs to a material, and sharing one
    // would make every drape show the last chunk's pixels.
    //
    // Lit, not emissive. A drape painted with its own brightness ignores the sun, which
    // made the run workspace — where the ground drape covers the whole map — the one view
    // with no shadows in it at all, and flat because of it: every surface the reader looked
    // at had the same light no matter which way it faced.
    const material = new StandardMaterial(`layerMaterial:${key}`, this.scene)
    material.specularColor = new Color3(0, 0, 0)
    material.useAlphaFromDiffuseTexture = texture.hasAlpha
    // Opaque where every cell is painted: blending a solid layer costs fill rate and
    // orders it against the terrain for nothing.
    material.transparencyMode = texture.hasAlpha
      ? StandardMaterial.MATERIAL_ALPHABLEND
      : StandardMaterial.MATERIAL_OPAQUE
    material.zOffset = -2
    material.diffuseTexture = raw
    // The theme's dimming, which a lit material carries as its own colour.
    material.diffuseColor = new Color3(this.tint, this.tint, this.tint)
    drape.material = material
    drape.isPickable = false
    // The colours the surface mesh carries are the white model's own; a drape paints over
    // them with its material, and multiplying the two would tint every road with the
    // elevation ramp it happens to lie on.
    drape.useVertexColors = false
    this.mapScene.receiveShadows(drape)
    this.textures.set(key, raw)
    this.meshes.set(key, drape)
    this.vertexCounts.set(key, vertexCount)
  }

  /** Removes one chunk's drape and the material it owned. */
  drop(key: string): void {
    this.mapScene.invalidate()
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
