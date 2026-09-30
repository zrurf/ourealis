/*
 * Elevation chunks as meshes.
 *
 * The thin part: this hands the vertex data `render/terrainMesh.ts` builds to Babylon, and
 * keeps one mesh per `(level, chunk)` pair plus the slab that goes under it. A chunk that
 * arrives late is added beside the ones already drawn rather than triggering a rebuild of
 * the surface, which is what lets a coarse level show first and finer levels fill in.
 *
 * Both meshes of a chunk register with the scene's height exaggeration, so a change to the
 * vertical scale moves the surface and its slab together.
 *
 * The layer also owns the *ramp*: the geometry carries a normalised elevation and a shading
 * factor, and the colour buffer is derived from them here. A light/dark switch therefore
 * re-uploads one colour buffer per mesh instead of rebuilding the map, and it reaches every
 * chunk already drawn, including the ones that arrived after the switch.
 */
import { VertexBuffer } from '@babylonjs/core/Buffers/buffer'
import { StandardMaterial } from '@babylonjs/core/Materials/standardMaterial'
import { Color3 } from '@babylonjs/core/Maths/math.color'
import { Mesh } from '@babylonjs/core/Meshes/mesh'
import { VertexData } from '@babylonjs/core/Meshes/mesh.vertexData'
import type { Scene } from '@babylonjs/core/scene'
import { TERRAIN_RAMP, TERRAIN_RAMP_DARK, type Rgb } from '@/types/colormap'
import { GRAIN_COMPENSATION, grainTexture } from './grain'
import type { ChunkCuller } from './chunkCuller'
import type { MapScene } from './scene'
import { terrainColours } from './shading'
import type { ChunkMeshData, MeshData } from './terrainMesh'

export type { ChunkMeshData, ChunkMeshOptions, MeshData } from './terrainMesh'
export { buildChunkMesh, chunkEdges, elevationRange, heightFieldNormals } from './terrainMesh'

/** The ramp an appearance draws the surface with. */
export function rampFor(dark: boolean): readonly Rgb[] {
  return dark ? TERRAIN_RAMP_DARK : TERRAIN_RAMP
}

/** The elevation surface of one map, one mesh per loaded chunk plus its slab. */
export class TerrainLayer {
  private readonly scene: Scene
  private readonly mapScene: MapScene
  private readonly material: StandardMaterial
  private readonly slabMaterial: StandardMaterial
  private readonly meshes = new Map<string, Mesh>()
  private readonly slabs = new Map<string, Mesh>()
  /** Vertex data of every mesh drawn, so a ramp change can recolour without the geometry. */
  private readonly meshData = new Map<Mesh, MeshData>()
  private ramp: readonly Rgb[]
  /** The grain both materials share, so it is built once and disposed once. */
  private readonly grain: ReturnType<typeof grainTexture>
  private readonly unbindAppearance: () => void
  /**
   * Decides which of these meshes are out of view.
   *
   * Optional: a surface built without one is drawn exactly as it always was, which is
   * what the headless test lanes and the studio want.
   */
  private culler: ChunkCuller | null = null

  constructor(scene: Scene, mapScene: MapScene) {
    this.scene = scene
    this.mapScene = mapScene
    this.ramp = rampFor(mapScene.isDark)
    this.unbindAppearance = mapScene.onAppearance((dark) => {
      this.setRamp(rampFor(dark))
    })
    const grain = grainTexture(scene)
    this.grain = grain
    this.material = new StandardMaterial('terrainMaterial', scene)
    this.material.specularColor = new Color3(0, 0, 0)
    // The grain is a near-white noise whose mean is below white, so the albedo is scaled up
    // by the reciprocal: the surface keeps the colour the ramp gave it, and gains detail.
    this.material.diffuseColor = new Color3(
      GRAIN_COMPENSATION,
      GRAIN_COMPENSATION,
      GRAIN_COMPENSATION,
    )
    this.material.diffuseTexture = grain
    this.material.backFaceCulling = true
    // The slab is seen from outside the model, and its winding follows the map's own
    // rim; not culling it keeps a wall readable from an angle where its front face is
    // turned away.
    this.slabMaterial = new StandardMaterial('terrainSlabMaterial', scene)
    this.slabMaterial.specularColor = new Color3(0, 0, 0)
    this.slabMaterial.diffuseColor = new Color3(
      GRAIN_COMPENSATION,
      GRAIN_COMPENSATION,
      GRAIN_COMPENSATION,
    )
    this.slabMaterial.diffuseTexture = grain
    this.slabMaterial.backFaceCulling = false
  }

  /** Sets the ramp the surface is coloured with; only the colour buffers are rewritten. */
  setRamp(ramp: readonly Rgb[]): void {
    this.mapScene.invalidate()
    if (ramp === this.ramp) {
      return
    }
    this.ramp = ramp
    for (const [mesh, data] of this.meshData) {
      mesh.updateVerticesData(
        VertexBuffer.ColorKind,
        terrainColours(data.rampInput, ramp, data.building),
      )
    }
  }

  /**
   * Hands the surface's meshes to a culler, and takes them back when it is dropped.
   *
   * A surface is the thing worth culling: a large map is a thousand of these meshes and
   * most of them are behind the reader at any moment. The route, the handles and the
   * grid are a handful and are never registered.
   */
  useCuller(culler: ChunkCuller | null): void {
    if (this.culler === culler) {
      return
    }
    for (const mesh of this.meshes.values()) {
      this.culler?.remove(mesh)
    }
    for (const slab of this.slabs.values()) {
      this.culler?.remove(slab)
    }
    this.culler = culler
    if (culler !== null) {
      for (const mesh of this.meshes.values()) {
        culler.add(mesh)
      }
      for (const slab of this.slabs.values()) {
        culler.add(slab)
      }
    }
  }

  /** Adds or replaces the mesh of one chunk and the slab it carries. */
  setChunk(key: string, data: ChunkMeshData): Mesh {
    this.mapScene.invalidate()
    this.drop(key)
    const mesh = this.create(`terrain:${key}`, data, this.material)
    mesh.isPickable = true
    const slab = this.create(`terrain-slab:${key}`, data.slab, this.slabMaterial)
    slab.isPickable = false
    this.meshes.set(key, mesh)
    this.slabs.set(key, slab)
    // A new mesh is on screen wherever its chunk is, which is inside the view for any
    // camera that framed it; registering it enabled avoids a frame of missing ground.
    this.culler?.add(mesh)
    this.culler?.add(slab)
    return mesh
  }

  /** Shows or hides the whole surface without dropping its meshes. */
  setVisible(visible: boolean): void {
    for (const mesh of this.meshes.values()) {
      mesh.setEnabled(visible)
    }
    for (const slab of this.slabs.values()) {
      slab.setEnabled(visible)
    }
    // The surface is on or off as a whole, so any decision the culler made is stale.
    this.culler?.invalidate()
  }

  /** True when the chunk already has a mesh. */
  has(key: string): boolean {
    return this.meshes.has(key)
  }

  /** Surface meshes currently drawn, for picking and for a bounds fit. */
  get list(): Mesh[] {
    return [...this.meshes.values()]
  }

  /** Removes every mesh; a level change or a map change rebuilds from scratch. */
  clear(): void {
    for (const key of Array.from(this.meshes.keys())) {
      this.drop(key)
    }
  }

  /** Disposes the meshes, the materials and the grain they share. */
  dispose(): void {
    this.unbindAppearance()
    this.useCuller(null)
    this.clear()
    this.material.dispose()
    this.slabMaterial.dispose()
    // The grain is a raw texture of its own, and disposing the materials that used it
    // does not dispose it: without this it stayed on the engine for the session.
    this.grain.dispose()
  }

  /** Builds one mesh of vertex data and registers it with the height exaggeration. */
  private create(name: string, data: MeshData, material: StandardMaterial): Mesh {
    const mesh = new Mesh(name, this.scene)
    const vertexData = new VertexData()
    vertexData.positions = data.positions
    vertexData.indices = data.indices
    vertexData.normals = data.normals
    vertexData.colors = terrainColours(data.rampInput, this.ramp, data.building)
    vertexData.uvs = data.uvs
    // Updatable: a light/dark switch rewrites the colour buffer in place rather than
    // rebuilding a mesh whose geometry did not change.
    vertexData.applyToMesh(mesh, true)
    mesh.material = material
    this.meshData.set(mesh, data)
    this.mapScene.trackHeightMesh(mesh)
    // The surface is both: a raised footprint throws the shadow, and the ground beside it
    // receives one. Nothing else in the scene can do either, because a building is not
    // separate geometry here — it is part of this mesh.
    this.mapScene.addShadowCaster(mesh)
    this.mapScene.receiveShadows(mesh)
    return mesh
  }

  /** Disposes one chunk's meshes. */
  private drop(key: string): void {
    this.mapScene.invalidate()
    for (const mesh of [this.meshes.get(key), this.slabs.get(key)]) {
      if (mesh === undefined) {
        continue
      }
      this.mapScene.untrackHeightMesh(mesh)
      this.mapScene.removeShadowCaster(mesh)
      this.culler?.remove(mesh)
      this.meshData.delete(mesh)
      mesh.dispose()
    }
    this.meshes.delete(key)
    this.slabs.delete(key)
  }
}
