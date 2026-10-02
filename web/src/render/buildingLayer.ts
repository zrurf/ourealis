/*
 * Building prisms in the scene.
 *
 * The thin part over `render/buildings.ts`: one mesh per chunk's prisms under one white
 * material. The prisms are the campus's real volumes — they cast the shadows the flattened
 * height field used to fake — and they register with the height exaggeration like the
 * terrain does, so a stretched surface carries its blocks with it.
 */
import { StandardMaterial } from '@babylonjs/core/Materials/standardMaterial'
import { Color3 } from '@babylonjs/core/Maths/math.color'
import { Mesh } from '@babylonjs/core/Meshes/mesh'
import { VertexData } from '@babylonjs/core/Meshes/mesh.vertexData'
import type { Scene } from '@babylonjs/core/scene'
import { BUILDING_COLOUR } from './shading'
import type { MapScene } from './scene'
import type { BuildingMeshData } from './buildings'

export type { BuildingMeshData } from './buildings'
export { buildChunkBuildings } from './buildings'

/** The white model's blocks: one mesh per chunk under one untextured material. */
export class BuildingLayer {
  private readonly scene: Scene
  private readonly mapScene: MapScene
  private readonly material: StandardMaterial
  private readonly meshes = new Map<string, Mesh>()

  constructor(scene: Scene, mapScene: MapScene) {
    this.scene = scene
    this.mapScene = mapScene
    const [r, g, b] = BUILDING_COLOUR
    this.material = new StandardMaterial('buildingMaterial', scene)
    this.material.diffuseColor = new Color3(r / 255, g / 255, b / 255)
    // A touch of self-lighting: a wall turned away from the sun takes only the sky half of
    // the hemispheric light, which on a dark page reads as a black gap between blocks. This
    // keeps the white model white while the sun still shapes it.
    this.material.emissiveColor = new Color3(0.16, 0.16, 0.16)
    this.material.specularColor = new Color3(0, 0, 0)
  }

  /** Adds or replaces the prisms of one chunk. */
  setChunk(key: string, data: BuildingMeshData): Mesh {
    this.mapScene.invalidate()
    this.drop(key)
    const mesh = new Mesh(`buildings:${key}`, this.scene)
    const vertexData = new VertexData()
    vertexData.positions = data.positions
    vertexData.indices = data.indices
    vertexData.normals = data.normals
    vertexData.applyToMesh(mesh)
    mesh.material = this.material
    mesh.isPickable = false
    // A block throws the shadow its footprint suggests, and does not receive one itself.
    // Receiving is what painted jagged black patches along every wall's foot: a wall's
    // squared-off normal grazes the sun at the angle where no depth offset in the shadow
    // map can separate the wall from itself, and a white model needs no self-shadow — the
    // sun's own per-face term already shapes it.
    this.mapScene.addShadowCaster(mesh)
    this.mapScene.trackHeightMesh(mesh)
    this.meshes.set(key, mesh)
    return mesh
  }

  /** True when the chunk already has prisms. */
  has(key: string): boolean {
    return this.meshes.has(key)
  }

  /** Removes every prism; a level or style change rebuilds from scratch. */
  clear(): void {
    for (const key of Array.from(this.meshes.keys())) {
      this.drop(key)
    }
  }

  /** Disposes the meshes and the material. */
  dispose(): void {
    this.clear()
    this.material.dispose()
  }

  /** Disposes one chunk's mesh. */
  private drop(key: string): void {
    const mesh = this.meshes.get(key)
    if (mesh === undefined) {
      return
    }
    this.mapScene.invalidate()
    this.mapScene.untrackHeightMesh(mesh)
    this.mapScene.removeShadowCaster(mesh)
    mesh.dispose()
    this.meshes.delete(key)
  }
}
