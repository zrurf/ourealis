/*
 * The route as something you can grab.
 *
 * A route is a handful of points; making the reader type their coordinates is the
 * reason the studio felt like filling in a form rather than drawing a line on a map.
 * This layer draws those points as marks on the surface and answers which one the
 * pointer is over, so the view can drag it.
 *
 * The two ends are *flags on poles* and the intermediate points are flat discs: a route's ends
 * are what the reader names aloud ("start", "finish") and a 3D mark reads from any camera pitch,
 * while a waypoint is an annotation on the way and should not compete with them. They are the
 * only pickable meshes in the scene: a click that does not land on one is a click on the ground,
 * which is how the "place the next point" gesture works.
 */
import { StandardMaterial } from '@babylonjs/core/Materials/standardMaterial'
import { Color3 } from '@babylonjs/core/Maths/math.color'
import { Matrix, Vector3 } from '@babylonjs/core/Maths/math.vector'
import { CreateDisc } from '@babylonjs/core/Meshes/Builders/discBuilder'
import { CreateBox } from '@babylonjs/core/Meshes/Builders/boxBuilder'
import type { AbstractMesh } from '@babylonjs/core/Meshes/abstractMesh'
import type { Scene } from '@babylonjs/core/scene'
import { HEIGHT_RAMP, SERIES_PALETTE, rgbToHex } from '@/types/colormap'
import { Flag } from './markers'
import type { MapScene } from './scene'

/** Which field of the route a handle edits. */
export type HandleRole = 'start' | 'goal' | 'reference' | 'waypoint' | 'checkpoint'

/** One point of the route as the map draws it. */
export interface HandleSpec {
  /** Field the handle edits. */
  role: HandleRole
  /** Index inside `waypoints` or `checkpoints`; zero for the single-point roles. */
  index: number
  /** Position in the map's metre plane. */
  x: number
  /** Position in the map's metre plane. */
  y: number
  /** Surface elevation, metres, or `null` when it is not known yet. */
  z: number | null
  /** Passing behaviour of a waypoint, which decides its symbol. */
  semantics?: string
  /**
   * Whether the map refuses this point.
   *
   * A point can be dropped on ground a runner cannot stand on — inside a building, or
   * against a wall — and the handle is where the reader finds that out: it is the thing
   * they just placed.
   */
  illegal?: boolean
}

/** Stable key of a handle, also its mesh name. */
export function handleKey(spec: { role: HandleRole; index: number }): string {
  return `${spec.role}:${spec.index}`
}

/** Colour of a handle the map refuses. */
const REFUSED_COLOUR = new Color3(0.72, 0.2, 0.19)

/** Radius of a handle symbol, metres; small enough not to hide the route under it. */
const HANDLE_RADIUS_M = 2.6

/** How far a handle floats above the ground, metres. */
const HANDLE_LIFT_M = 0.6

/**
 * How close the pointer has to be to grab a handle, in screen pixels.
 *
 * A screen-space radius rather than a hit on the mesh: a 2.6 m disc is about six pixels
 * across at a map's default framing, which is far harder to hit than a control should
 * be, and the symbol is a label for a point rather than an object with a shape.
 */
const GRAB_RADIUS_PX = 14

/** Colours of the roles, from the series palette the rest of the interface uses. */
function roleColour(role: HandleRole): Color3 {
  switch (role) {
    case 'start':
      return Color3.FromHexString(SERIES_PALETTE[1] ?? rgbToHex(HEIGHT_RAMP[0] ?? [0, 0, 0]))
    case 'goal':
      return Color3.FromHexString(SERIES_PALETTE[4] ?? rgbToHex(HEIGHT_RAMP[0] ?? [0, 0, 0]))
    case 'reference':
      return Color3.FromHexString(SERIES_PALETTE[2] ?? rgbToHex(HEIGHT_RAMP[0] ?? [0, 0, 0]))
    default:
      return Color3.FromHexString(SERIES_PALETTE[0] ?? rgbToHex(HEIGHT_RAMP[0] ?? [0, 0, 0]))
  }
}

/** Roles drawn as a flag on a pole rather than as a disc. */
const FLAG_ROLES: ReadonlySet<HandleRole> = new Set<HandleRole>(['start', 'goal'])

/** The route's handles: a flag for each end, a disc for every other point. */
export class RouteHandles {
  private readonly scene: Scene
  private readonly mapScene: MapScene
  private readonly materials = new Map<string, StandardMaterial>()
  private readonly meshes = new Map<string, AbstractMesh>()
  private readonly flags = new Map<string, Flag>()
  private readonly specs = new Map<string, HandleSpec>()
  private selected: string | null = null
  private observer: { remove(): void } | null = null

  constructor(scene: Scene, mapScene: MapScene) {
    this.scene = scene
    this.mapScene = mapScene
  }

  /** Replaces every handle with the given set. */
  update(specs: readonly HandleSpec[]): void {
    const wanted = new Set(specs.map(handleKey))
    // A handle whose point was deleted goes with it; the rest are moved, which keeps
    // the mesh count stable while a point is being dragged.
    for (const [key, mesh] of Array.from(this.meshes)) {
      if (!wanted.has(key)) {
        mesh.dispose()
        this.meshes.delete(key)
        this.specs.delete(key)
      }
    }
    for (const [key, flag] of Array.from(this.flags)) {
      if (!wanted.has(key)) {
        flag.dispose()
        this.flags.delete(key)
        this.specs.delete(key)
      }
    }
    for (const spec of specs) {
      const key = handleKey(spec)
      const previous = this.specs.get(key)
      this.specs.set(key, spec)
      if (FLAG_ROLES.has(spec.role)) {
        const flag = this.flags.get(key) ?? this.createFlag(spec)
        this.flags.set(key, flag)
        flag.place(spec.x, spec.y, spec.z ?? 0)
        continue
      }
      const existing = this.meshes.get(key)
      const position = this.positionOf(spec)
      if (existing !== undefined) {
        existing.position = position
        if (previous?.illegal !== spec.illegal) {
          existing.material = this.materialFor(spec.role, spec.illegal === true)
        }
        continue
      }
      const mesh = this.create(spec, position)
      this.meshes.set(key, mesh)
    }
    this.applySelection()
  }

  /** Marks one handle as the selected one, or clears the mark. */
  setSelected(key: string | null): void {
    this.selected = key
    this.applySelection()
  }

  /** The key of the selected handle, if any. */
  get selectedKey(): string | null {
    return this.selected
  }

  /**
   * The handle under a canvas position, or `null` for the ground.
   *
   * The test is in screen space, not against the meshes: the pointer position is
   * projected through the camera for each handle and the nearest one within
   * {@link GRAB_RADIUS_PX} wins. Picking the mesh itself was tried and never hit — a
   * six-pixel disc is a smaller target than the browser's own pointer slop, so a reader
   * aiming at a marker would place a new point instead of moving the one they meant.
   */
  pickAt(x: number, y: number): HandleSpec | null {
    const engine = this.scene.getEngine()
    const viewport = this.scene.getEngine().getRenderingCanvasClientRect()
    const renderWidth = engine.getRenderWidth()
    const renderHeight = engine.getRenderHeight()
    if (viewport === null || renderWidth === 0 || renderHeight === 0) {
      return null
    }
    // Projection lands in render pixels; the pointer arrives in CSS pixels.
    const scaleX = viewport.width / renderWidth
    const scaleY = viewport.height / renderHeight
    const transform = this.scene.getTransformMatrix()
    const globalViewport = this.scene.activeCamera?.viewport.toGlobal(renderWidth, renderHeight)
    if (globalViewport === undefined) {
      return null
    }
    let best: HandleSpec | null = null
    let bestDistance = GRAB_RADIUS_PX
    // Both kinds of mark are grabbed the same way: by where they stand on the ground, which is
    // also where the pointer is aiming when the reader reaches for one.
    const marks: Array<[string, { x: number; y: number; z: number }]> = []
    for (const [key, mesh] of this.meshes) {
      if (mesh.isEnabled()) {
        marks.push([key, mesh.position])
      }
    }
    for (const [key, flag] of this.flags) {
      marks.push([key, flag.position])
    }
    for (const [key, position] of marks) {
      const projected = Vector3.Project(
        new Vector3(position.x, position.y, position.z),
        Matrix.Identity(),
        transform,
        globalViewport,
      )
      const distance = Math.hypot(projected.x * scaleX - x, projected.y * scaleY - y)
      if (distance <= bestDistance) {
        bestDistance = distance
        best = this.specs.get(key) ?? null
      }
    }
    return best
  }

  /** Disposes every mark and material. */
  dispose(): void {
    for (const mesh of this.meshes.values()) {
      mesh.dispose()
    }
    for (const flag of this.flags.values()) {
      flag.dispose()
    }
    this.meshes.clear()
    this.flags.clear()
    this.specs.clear()
    for (const material of this.materials.values()) {
      material.dispose()
    }
    this.materials.clear()
    this.observer?.remove()
    this.observer = null
  }

  /** Builds a flag for a route end, and starts the ripple that runs in the render loop. */
  private createFlag(spec: HandleSpec): Flag {
    const flag = new Flag(this.mapScene, spec.role === 'goal' ? 'goal' : 'start')
    this.startRipple()
    return flag
  }

  /** Ripples every flag's cloth once per frame. */
  private startRipple(): void {
    if (this.observer !== null) {
      return
    }
    const start = Date.now()
    const observable = this.scene.onBeforeRenderObservable.add(() => {
      const seconds = (Date.now() - start) / 1000
      for (const flag of this.flags.values()) {
        flag.update(seconds)
      }
    })
    this.observer = {
      remove: () => {
        this.scene.onBeforeRenderObservable.remove(observable)
      },
    }
  }

  /** Where a handle sits: on the surface when its height is known, on the plane otherwise. */
  private positionOf(spec: HandleSpec): Vector3 {
    const y = (spec.z ?? 0) + HANDLE_LIFT_M
    return new Vector3(spec.x, y * this.mapScene.exaggerationFactor, spec.y)
  }

  /** Builds the symbol of one handle. */
  private create(spec: HandleSpec, position: Vector3): AbstractMesh {
    const material = this.materialFor(spec.role, spec.illegal === true)
    // The goal is the one square: shape is what keeps the ends apart at a glance when
    // both are the same size, and colour alone is not enough for a red-green reader.
    const mesh =
      spec.role === 'goal'
        ? CreateBox(
            `h:${handleKey(spec)}`,
            { size: HANDLE_RADIUS_M * 1.6, height: 0.4 },
            this.scene,
          )
        : CreateDisc(
            `h:${handleKey(spec)}`,
            { radius: HANDLE_RADIUS_M, tessellation: 20 },
            this.scene,
          )
    if (spec.role !== 'goal') {
      mesh.rotation.x = Math.PI / 2
    }
    mesh.position = position
    mesh.material = material
    mesh.isPickable = true
    mesh.renderingGroupId = 2
    this.mapScene.trackHeightMesh(mesh)
    return mesh
  }

  /**
   * Emphasises the selected handle and the one whose semantics make it a stop.
   *
   * The pointer is not over a handle most of the time, so the selected one has to say
   * so on its own; a dwell waypoint is the one whose behaviour changes the trajectory,
   * which its symbol reports.
   */
  private applySelection(): void {
    for (const [key, mesh] of this.meshes) {
      const spec = this.specs.get(key)
      const base = spec?.semantics === 'dwell' ? 1.35 : 1
      const scale = key === this.selected ? base * 1.5 : base
      mesh.scaling.x = scale
      mesh.scaling.z = scale
    }
  }

  /** The emissive material of one role and verdict, created on first use. */
  private materialFor(role: HandleRole, illegal: boolean): StandardMaterial {
    const key = `${role}:${illegal ? 'refused' : 'usable'}`
    const existing = this.materials.get(key)
    if (existing !== undefined) {
      return existing
    }
    const material = new StandardMaterial(`handleMaterial:${key}`, this.scene)
    const colour = illegal ? REFUSED_COLOUR : roleColour(role)
    material.emissiveColor = colour
    material.diffuseColor = colour
    material.specularColor = new Color3(0, 0, 0)
    material.disableLighting = true
    material.zOffset = -6
    this.materials.set(key, material)
    return material
  }
}
