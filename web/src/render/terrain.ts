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
 */
import { StandardMaterial } from '@babylonjs/core/Materials/standardMaterial'
import { Color3 } from '@babylonjs/core/Maths/math.color'
import { Mesh } from '@babylonjs/core/Meshes/mesh'
import { VertexData } from '@babylonjs/core/Meshes/mesh.vertexData'
import type { Scene } from '@babylonjs/core/scene'
import { GRAIN_COMPENSATION, grainTexture } from './grain'
import type { MapScene } from './scene'
import type { ChunkMeshData, MeshData } from './terrainMesh'

export type { ChunkMeshData, ChunkMeshOptions, MeshData } from './terrainMesh'
export { buildChunkMesh, chunkEdges, elevationRange, heightFieldNormals } from './terrainMesh'

/** The elevation surface of one map, one mesh per loaded chunk plus its slab. */
export class TerrainLayer {
  private readonly scene: Scene
  private readonly mapScene: MapScene
  private readonly material: StandardMaterial
  private readonly slabMaterial: StandardMaterial
  private readonly meshes = new Map<string, Mesh>()
  private readonly slabs = new Map<string, Mesh>()

  constructor(scene: Scene, mapScene: MapScene) {
    this.scene = scene
    this.mapScene = mapScene
    this.material = new StandardMaterial('terrainMaterial', scene)
    this.material.specularColor = new Color3(0, 0, 0)
    // The grain is a near-white noise whose mean is below white, so the albedo is scaled up
    // by the reciprocal: the surface keeps the colour the ramp gave it, and gains detail.
    this.material.diffuseColor = new Color3(
      GRAIN_COMPENSATION,
      GRAIN_COMPENSATION,
      GRAIN_COMPENSATION,
    )
    this.material.diffuseTexture = grainTexture(scene)
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
    this.slabMaterial.diffuseTexture = grainTexture(scene)
    this.slabMaterial.backFaceCulling = false
  }

  /** Adds or replaces the mesh of one chunk and the slab it carries. */
  setChunk(key: string, data: ChunkMeshData): Mesh {
    this.drop(key)
    const mesh = this.create(`terrain:${key}`, data, this.material)
    mesh.isPickable = true
    const slab = this.create(`terrain-slab:${key}`, data.slab, this.slabMaterial)
    slab.isPickable = false
    this.meshes.set(key, mesh)
    this.slabs.set(key, slab)
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

  /** Disposes the meshes and the materials. */
  dispose(): void {
    this.clear()
    this.material.dispose()
    this.slabMaterial.dispose()
  }

  /** Builds one mesh of vertex data and registers it with the height exaggeration. */
  private create(name: string, data: MeshData, material: StandardMaterial): Mesh {
    const mesh = new Mesh(name, this.scene)
    const vertexData = new VertexData()
    vertexData.positions = data.positions
    vertexData.indices = data.indices
    vertexData.normals = data.normals
    vertexData.colors = data.colors
    vertexData.uvs = data.uvs
    vertexData.applyToMesh(mesh, false)
    mesh.material = material
    this.mapScene.trackHeightMesh(mesh)
    return mesh
  }

  /** Disposes one chunk's meshes. */
  private drop(key: string): void {
    for (const mesh of [this.meshes.get(key), this.slabs.get(key)]) {
      if (mesh === undefined) {
        continue
      }
      this.mapScene.untrackHeightMesh(mesh)
      mesh.dispose()
    }
    this.meshes.delete(key)
    this.slabs.delete(key)
  }
}
