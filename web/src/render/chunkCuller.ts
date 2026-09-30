/*
 * Frustum culling for streamed chunks.
 *
 * A map of a few square kilometres is a thousand meshes at the finest level, and Babylon
 * walks all of them on every frame to decide which to draw. That walk is unavoidable per
 * mesh, but the *culling* of one is not the point: the point is that a chunk which has
 * streamed out of the camera's footprint is still submitted every frame until the map is
 * torn down. This turns those off, and leaves everything it was not given alone.
 *
 * The design is shaped by what a wrong answer costs. A chunk that is wrongly kept costs a
 * draw call; a chunk that is wrongly dropped takes a piece of the map away from a reader
 * who is looking at it, and there is no visual cue that says "that part was culled". So:
 *
 * - the planes come from Babylon's own `Frustum.GetPlanes`, the same call its per-mesh
 *   culling uses, so the two cannot disagree about where the view volume is;
 * - the box test is the one-sided predicate in `culling.ts`, which may only answer "no"
 *   when the box is provably outside;
 * - the camera's own position and orbit target are protected, so the ground a reader is
 *   standing on survives even when its bounds do not reach the view;
 * - a small map is never culled at all, because there is nothing to save and only risk;
 * - anything that goes wrong disables culling and puts every mesh back, rather than
 *   leaving the surface with a hole in it.
 */
import type { AbstractMesh } from '@babylonjs/core/Meshes/abstractMesh'
import { Frustum } from '@babylonjs/core/Maths/math.frustum'
import { Vector3 } from '@babylonjs/core/Maths/math.vector'
import type { Plane } from '@babylonjs/core/Maths/math.plane'
import type { Scene } from '@babylonjs/core/scene'
import {
  cullCandidates,
  CULL_MARGIN,
  frustumUsable,
  boxInsideFrustum,
  type Aabb,
  type CullCandidate,
  type FrustumPlane,
} from './culling'
import type { MapScene, OurealisDebug } from './scene'

/**
 * Fewest meshes worth culling.
 *
 * The cost of a decision is eight corner transforms per mesh, and it only runs when the
 * camera has actually moved, so even a small map pays almost nothing for the check. The
 * threshold is therefore low: it is there to leave a handful of meshes drawn exactly as
 * before, where a decision could not save anything worth the risk, and not to keep culling
 * off a map that happens to be one level coarser than it was a moment ago.
 */
const MIN_CANDIDATES = 8

/** One mesh under the culler, with the world box that decides it. */
interface Tracked {
  mesh: AbstractMesh
  candidate: CullCandidate<AbstractMesh>
}

/** Converts Babylon's plane into the plain shape the predicate takes. */
function toPlane(plane: Plane): FrustumPlane {
  const normal = plane.normal
  return { a: normal.x, b: normal.y, c: normal.z, d: plane.d }
}

/**
 * Keeps the streamed chunks that are out of view switched off.
 *
 * Owned by a view rather than by the scene, because what is worth culling is a property
 * of the map being drawn: the route, the handles and the grid are few and are never
 * registered.
 */
export class ChunkCuller {
  private readonly scene: Scene
  private readonly mapScene: MapScene
  private tracked = new Map<AbstractMesh, Tracked>()
  private observer: { remove(): void } | null = null
  private enabled = true
  /** The camera as of the last cull, so a still camera costs nothing. */
  private lastCamera: readonly number[] = []
  private passes = 0
  private rescues = 0

  constructor(scene: Scene, mapScene: MapScene) {
    this.scene = scene
    this.mapScene = mapScene
  }

  /** Whether culling is running; `false` puts every tracked mesh back and keeps it there. */
  get active(): boolean {
    return this.enabled && this.tracked.size >= MIN_CANDIDATES
  }

  /** Starts deciding after each frame. */
  start(): void {
    this.observer ??= this.scene.onAfterRenderObservable.add(() => this.tick())
  }

  /** Stops deciding and puts everything back. */
  stop(): void {
    this.observer?.remove()
    this.observer = null
    this.showAll()
  }

  /**
   * Starts or stops culling.
   *
   * The escape hatch a reader or a test reaches for when the surface looks wrong: with
   * culling off the map is drawn exactly as it was before any of this existed.
   */
  setEnabled(on: boolean): void {
    this.enabled = on
    if (!on) {
      this.showAll()
    } else {
      this.invalidate()
    }
  }

  /** Adds a mesh, and switches it on until the first decision says otherwise. */
  add(mesh: AbstractMesh): void {
    if (this.tracked.has(mesh)) {
      return
    }
    this.tracked.set(mesh, {
      mesh,
      candidate: { box: { min: [0, 0, 0], max: [0, 0, 0] }, value: mesh },
    })
    this.invalidate()
  }

  /** Stops tracking a mesh, which is what a chunk's own disposal does. */
  remove(mesh: AbstractMesh): void {
    this.tracked.delete(mesh)
  }

  /** What the browser lane reads; the shape `OurealisDebug.culling` expects. */
  get report(): OurealisDebug['culling'] {
    return {
      active: this.active,
      culled: this.culledCount,
      tracked: this.trackedCount,
      passes: this.passes,
      rescues: this.rescues,
      // Triangles the tracked meshes hold, whether or not they are on screen: the budget
      // a frame is measured against, and the number a decimation has to move.
      triangles: this.triangles,
    }
  }

  /** Triangles every tracked mesh holds, on screen or not. */
  get triangles(): number {
    let total = 0
    for (const { mesh } of this.tracked.values()) {
      total += mesh.getTotalIndices() / 3
    }
    return total
  }

  /** Number of meshes currently switched off, for the test surface. */
  get culledCount(): number {
    let count = 0
    for (const { mesh } of this.tracked.values()) {
      if (!mesh.isEnabled()) {
        count += 1
      }
    }
    return count
  }

  /** Number of meshes under the culler, for the test surface. */
  get trackedCount(): number {
    return this.tracked.size
  }

  /** Decides again on the next frame, whatever the camera did. */
  invalidate(): void {
    this.lastCamera = []
  }

  /** The decision, run directly. Exposed so the browser lane can assert on it. */
  apply(): void {
    if (!this.enabled) {
      this.showAll()
      return
    }
    if (this.tracked.size < MIN_CANDIDATES) {
      this.showAll()
      return
    }
    let planes: FrustumPlane[]
    try {
      planes = Frustum.GetPlanes(this.scene.getTransformMatrix()).map(toPlane)
    } catch {
      // A matrix the engine could not produce is a reason to draw everything.
      this.setEnabled(false)
      this.showAll()
      return
    }
    if (!frustumUsable(planes)) {
      this.showAll()
      return
    }
    const protectedPoints = this.protectedPoints()
    // The box is read here rather than held, so a mesh the exaggeration has rescaled
    // since the last decision is tested against the scale it is actually drawn at.
    const candidates = [...this.tracked.values()].map((entry) => ({
      box: worldBoxOf(entry.mesh),
      value: entry.mesh,
    }))
    let result
    try {
      result = cullCandidates(planes, candidates, protectedPoints)
    } catch {
      this.setEnabled(false)
      this.showAll()
      return
    }
    this.passes += 1
    this.rescues = result.visible.length - countInside(planes, candidates, CULL_MARGIN)
    const wanted = new Set(result.visible.map((candidate) => candidate.value))
    for (const { mesh } of this.tracked.values()) {
      const on = wanted.has(mesh)
      if (mesh.isEnabled() !== on) {
        mesh.setEnabled(on)
      }
    }
  }

  /**
   * Runs the decision only when the camera has moved.
   *
   * The camera is a handful of numbers; comparing them is far cheaper than rebuilding six
   * planes and testing a thousand boxes, and a preview sits still most of the time.
   */
  private tick(): void {
    if (!this.enabled || this.tracked.size < MIN_CANDIDATES) {
      return
    }
    const camera = this.mapScene.camera
    const now = [
      camera.alpha,
      camera.beta,
      camera.radius,
      camera.target.x,
      camera.target.y,
      camera.target.z,
      camera.fov,
    ]
    if (
      now.length === this.lastCamera.length &&
      now.every((value, index) => value === this.lastCamera[index])
    ) {
      return
    }
    this.lastCamera = now
    this.apply()
  }

  /**
   * The points a chunk is never dropped for: the camera itself and what it orbits.
   *
   * Both can be outside a chunk's own box on a map that is still streaming, and a reader
   * standing on a hole the surface no longer covers is the one failure mode no amount of
   * margin protects against.
   */
  private protectedPoints(): Array<[number, number, number]> {
    const camera = this.mapScene.camera
    const position = camera.globalPosition
    const target = camera.target
    return [
      [position.x, position.y, position.z],
      [target.x, target.y, target.z],
    ]
  }

  private showAll(): void {
    for (const { mesh } of this.tracked.values()) {
      if (!mesh.isEnabled()) {
        mesh.setEnabled(true)
      }
    }
  }
}

/**
 * The mesh's box in world metres.
 *
 * Read from the mesh's own immutable local bounds and its current world matrix, rather
 * than cached. The distinction matters: `boundingBox.minimumWorld` is only refreshed while
 * the engine is evaluating the mesh, so a mesh this culler has switched off stops having
 * a live world box — and a stale box is the one way a culler confidently drops something
 * that is on screen. Transforming the eight corners is exact for any transform, so it
 * needs no assumption about a rotation this project happens not to use, and it stays
 * correct when the vertical exaggeration rescales the mesh.
 */
function worldBoxOf(mesh: AbstractMesh): Aabb {
  const local = mesh.getBoundingInfo().boundingBox
  const world = mesh.getWorldMatrix()
  let minX = Number.POSITIVE_INFINITY
  let minY = Number.POSITIVE_INFINITY
  let minZ = Number.POSITIVE_INFINITY
  let maxX = Number.NEGATIVE_INFINITY
  let maxY = Number.NEGATIVE_INFINITY
  let maxZ = Number.NEGATIVE_INFINITY
  for (let corner = 0; corner < 8; corner += 1) {
    const point = Vector3.TransformCoordinates(
      new Vector3(
        corner & 1 ? local.maximum.x : local.minimum.x,
        corner & 2 ? local.maximum.y : local.minimum.y,
        corner & 4 ? local.maximum.z : local.minimum.z,
      ),
      world,
    )
    if (!Number.isFinite(point.x) || !Number.isFinite(point.y) || !Number.isFinite(point.z)) {
      // A mesh with no geometry, or a matrix that is not a transform. Keeping it is free.
      return {
        min: [Number.NaN, Number.NaN, Number.NaN],
        max: [Number.NaN, Number.NaN, Number.NaN],
      }
    }
    minX = Math.min(minX, point.x)
    minY = Math.min(minY, point.y)
    minZ = Math.min(minZ, point.z)
    maxX = Math.max(maxX, point.x)
    maxY = Math.max(maxY, point.y)
    maxZ = Math.max(maxZ, point.z)
  }
  return { min: [minX, minY, minZ], max: [maxX, maxY, maxZ] }
}

/** How many of the candidates the frustum accepts on their own, protected points aside. */
function countInside(
  planes: readonly FrustumPlane[],
  candidates: readonly CullCandidate<AbstractMesh>[],
  margin: number,
): number {
  let count = 0
  for (const candidate of candidates) {
    if (boxInsideFrustum(planes, candidate.box, margin)) {
      count += 1
    }
  }
  return count
}
