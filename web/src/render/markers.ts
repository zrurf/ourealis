/*
 * The marks a reader places on a map: flags and a pin.
 *
 * A route's ends and an inspected cell are not *data* — they are what the reader is working on —
 * and a data-shaped symbol for them (a disc, a square) has to be read off a legend. A flag on a
 * pole and a pin in the ground are the marks everyone already knows, and a 3D one survives being
 * seen from any angle, which a flat symbol at a glancing camera pitch does not.
 *
 * The meshes are built from primitives, so a mark needs no asset; each is registered with the
 * scene's height exaggeration, so a mark stays on the ground when the vertical scale changes.
 */
import { StandardMaterial } from '@babylonjs/core/Materials/standardMaterial'
import { Color3 } from '@babylonjs/core/Maths/math.color'
import { Vector3 } from '@babylonjs/core/Maths/math.vector'
import { CreateCylinder } from '@babylonjs/core/Meshes/Builders/cylinderBuilder'
import { CreateSphere } from '@babylonjs/core/Meshes/Builders/sphereBuilder'
import { Mesh } from '@babylonjs/core/Meshes/mesh'
import { VertexData } from '@babylonjs/core/Meshes/mesh.vertexData'
import type { MapScene } from './scene'

/** What a flag stands for. */
export type FlagKind = 'start' | 'goal'

/**
 * Height of a flag's pole, metres.
 *
 * Sized to be found at a whole-map view. A route's ends are the two points a reader looks for
 * first, and a flag that is proportional to a person is invisible from the camera that shows the
 * whole campus.
 */
const POLE_HEIGHT_M = 26

/** How far the pennant reaches from the pole, metres. */
const PENNANT_M = 18

/** Half the pennant's height at the pole, metres. */
const PENNANT_HALF_M = 7

/** How tall the pin stands, metres. */
const PIN_HEIGHT_M = 14

/** Radius of a flag's pole, metres. */
const POLE_RADIUS_M = 0.7

/** Colours of the two flags: the route's ends are the two shapes a reader names aloud. */
const FLAG_COLOURS: Readonly<Record<FlagKind, [number, number, number]>> = {
  start: [0.24, 0.55, 0.31],
  goal: [0.72, 0.2, 0.19],
}

/**
 * A flag planted at a point: a pole from the ground to its top, and a pennant on the pole.
 *
 * The two are one mark, so the cloth's edge is built *on* the pole's axis rather than beside it,
 * and both are placed from the same ground point. The pennant is a triangle — a flag is read from
 * its silhouette, and a rectangle on a pole is a sign; the third vertex trails, and the cloth
 * ripples by moving that vertex rather than by rotating the whole shape, which is what a cloth in
 * the wind does.
 */
export class Flag {
  private readonly mapScene: MapScene
  private readonly pole: Mesh
  private readonly cloth: Mesh
  private readonly material: StandardMaterial
  private readonly kind: FlagKind
  private base = new Vector3(0, 0, 0)
  private phase = 0

  constructor(mapScene: MapScene, kind: FlagKind) {
    this.mapScene = mapScene
    this.kind = kind
    const scene = mapScene.scene
    const [red, green, blue] = FLAG_COLOURS[kind]
    const colour = new Color3(red, green, blue)
    const material = new StandardMaterial(`flag:${kind}`, scene)
    material.diffuseColor = colour
    material.emissiveColor = colour.scale(0.4)
    material.specularColor = new Color3(0, 0, 0)
    // The pennant is a sheet seen from either side.
    material.backFaceCulling = false
    this.material = material

    // The pole's origin is its centre, so it stands from the ground to POLE_HEIGHT_M.
    this.pole = CreateCylinder(
      `flag-pole:${kind}`,
      { height: POLE_HEIGHT_M, diameter: POLE_RADIUS_M * 2, tessellation: 10 },
      scene,
    )
    this.pole.material = material
    this.pole.isPickable = true
    this.cloth = this.buildCloth()
    this.cloth.material = material
    this.cloth.isPickable = true
    for (const part of [this.pole, this.cloth]) {
      part.renderingGroupId = 2
      this.mapScene.trackHeightMesh(part)
    }
    this.phase = kind === 'start' ? 0 : Math.PI / 2
  }

  /** Moves the flag onto a map position and the ground under it. */
  place(x: number, mapY: number, groundY: number): void {
    this.base.set(x, groundY, mapY)
    this.pole.position.set(x, groundY + POLE_HEIGHT_M / 2, mapY)
    this.pole.setEnabled(true)
    this.cloth.setEnabled(true)
    this.update(0)
  }

  /** Hides the flag, for a route end that is not set. */
  hide(): void {
    this.pole.setEnabled(false)
    this.cloth.setEnabled(false)
  }

  /** Ripples the pennant: the trailing corner moves, the pole edge stays put. */
  update(timeSeconds: number): void {
    const wave = Math.sin(timeSeconds * 2 + this.phase) * 0.5 + Math.sin(timeSeconds * 3.4) * 0.2
    const top = this.base.y + POLE_HEIGHT_M
    // The pennant hangs from just below the pole's top, pointing away from the pole.
    const vertices = new Float32Array([
      0,
      PENNANT_HALF_M,
      0,
      0,
      -PENNANT_HALF_M,
      0,
      PENNANT_M * (0.9 + wave * 0.1),
      wave * PENNANT_HALF_M * 0.35,
      wave * 0.2,
    ])
    const data = new VertexData()
    data.positions = vertices
    data.indices = new Uint32Array([0, 1, 2])
    data.normals = new Float32Array([0, 0, 1, 0, 0, 1, 0, 0, 1])
    data.applyToMesh(this.cloth, true)
    this.cloth.position.set(this.base.x + POLE_RADIUS_M, top - PENNANT_HALF_M - 1.5, this.base.z)
  }

  /** Where the flag stands, for the handle picking. */
  get position(): Vector3 {
    return this.base
  }

  /** Disposes the flag. */
  dispose(): void {
    for (const part of [this.pole, this.cloth]) {
      this.mapScene.untrackHeightMesh(part)
      part.dispose()
    }
    this.material.dispose()
  }

  /** An empty pennant mesh; `update` writes its shape every frame. */
  private buildCloth(): Mesh {
    const mesh = new Mesh(`flag-cloth:${this.kind}`, this.mapScene.scene)
    const data = new VertexData()
    data.positions = new Float32Array([
      0,
      PENNANT_HALF_M,
      0,
      0,
      -PENNANT_HALF_M,
      0,
      PENNANT_M,
      0,
      0,
    ])
    data.indices = new Uint32Array([0, 1, 2])
    data.normals = new Float32Array([0, 0, 1, 0, 0, 1, 0, 0, 1])
    data.applyToMesh(mesh, true)
    return mesh
  }
}

/** A pin planted where the reader is inspecting: a stem from the ground to a round head. */
export class MapPin {
  private readonly mapScene: MapScene
  private readonly stem: Mesh
  private readonly head: Mesh
  private readonly material: StandardMaterial

  constructor(mapScene: MapScene) {
    this.mapScene = mapScene
    const scene = mapScene.scene
    const material = new StandardMaterial('map-pin', scene)
    material.diffuseColor = new Color3(0.13, 0.42, 0.75)
    material.emissiveColor = new Color3(0.12, 0.3, 0.55)
    material.specularColor = new Color3(0, 0, 0)
    this.material = material
    // One stem, one head, and the head sits exactly on the stem's top: the three parts were
    // placed from three different heights before, which left the mark in pieces.
    this.stem = CreateCylinder(
      'map-pin-stem',
      { height: PIN_HEIGHT_M, diameter: 1, tessellation: 10 },
      scene,
    )
    this.head = CreateSphere('map-pin-head', { diameter: 5, segments: 12 }, scene)
    for (const part of [this.stem, this.head]) {
      part.material = material
      part.isPickable = false
      part.renderingGroupId = 2
      this.mapScene.trackHeightMesh(part)
    }
  }

  /** Moves the pin onto a map position and the ground under it. */
  place(x: number, mapY: number, groundY: number): void {
    // The stem's origin is its centre, so it spans `groundY … groundY + PIN_HEIGHT_M`; the head
    // is centred on the stem's top, which is where they meet.
    this.stem.position.set(x, groundY + PIN_HEIGHT_M / 2, mapY)
    this.head.position.set(x, groundY + PIN_HEIGHT_M, mapY)
    this.stem.setEnabled(true)
    this.head.setEnabled(true)
  }

  /** Hides the pin. */
  hide(): void {
    this.stem.setEnabled(false)
    this.head.setEnabled(false)
  }

  /** Disposes the pin. */
  dispose(): void {
    for (const part of [this.stem, this.head]) {
      this.mapScene.untrackHeightMesh(part)
      part.dispose()
    }
    this.material.dispose()
  }
}
