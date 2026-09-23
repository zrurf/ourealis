/*
 * Picking: screen to ground.
 *
 * Two paths, in order. A pick against the terrain meshes gives the exact surface
 * point, because the surface is the mesh the user sees. When no terrain mesh is
 * loaded — before the first chunk arrives, or where a layer drape covers it — the
 * pick falls back to the ray's intersection with the map plane, which is still a
 * usable metre coordinate.
 *
 * The arithmetic of the fallback is a pure function so the geometry can be checked
 * without a scene; the entry point is the small part that needs Babylon.
 */
import { Ray } from '@babylonjs/core/Culling/ray'
import { Vector3 } from '@babylonjs/core/Maths/math.vector'
import type { AbstractMesh } from '@babylonjs/core/Meshes/abstractMesh'
import type { Scene } from '@babylonjs/core/scene'

/** A ground point in the map's own plane, metres. */
export interface GroundPoint {
  /** Easting, metres. */
  x: number
  /** Northing, metres. */
  y: number
  /** Surface elevation at the point, metres. */
  z: number
  /** Whether the elevation came from the terrain rather than from the plane fallback. */
  onTerrain: boolean
}

/** How a pick was resolved, for a caller that shows it. */
export type PickSource = 'terrain' | 'plane' | 'miss'

/** Outcome of a pick. */
export interface PickResult {
  /** Point the pick resolved to, or `null` when the ray left the map. */
  point: GroundPoint | null
  /** Which path produced the point. */
  source: PickSource
}

/**
 * Intersects a ray with the horizontal plane at `planeY`.
 *
 * Returns `null` for a ray parallel to the plane or pointing away from it, which
 * is what a pick outside the viewport above the horizon produces.
 */
export function planeIntersection(
  origin: readonly [number, number, number],
  direction: readonly [number, number, number],
  planeY = 0,
): [number, number, number] | null {
  const dy = direction[1]
  if (Math.abs(dy) < 1e-9) {
    return null
  }
  const t = (planeY - origin[1]) / dy
  if (t < 0) {
    return null
  }
  return [origin[0] + direction[0] * t, planeY, origin[2] + direction[2] * t]
}

/**
 * Picks the ground under a canvas position.
 *
 * `x` and `y` are canvas-relative CSS pixels, the same pair a mouse event carries
 * after the offset of the canvas is subtracted.
 */
export function pickGround(
  scene: Scene,
  x: number,
  y: number,
  options: { targets?: AbstractMesh[]; planeY?: number } = {},
): PickResult {
  const targets = options.targets
  if (targets !== undefined && targets.length > 0) {
    // `scene.pick` rather than a ray built by hand: the scene's pick uses the same
    // coordinate convention its own pointer events established, which is what keeps a
    // hit on a canvas of any size landing where the reader clicked.
    const allowed = new Set(targets)
    const hit = scene.pick(x, y, (mesh) => allowed.has(mesh))
    if (hit?.hit === true && hit.pickedPoint !== null) {
      const point = hit.pickedPoint
      return { point: { x: point.x, y: point.z, z: point.y, onTerrain: true }, source: 'terrain' }
    }
  }
  const ray = scene.createPickingRay(x, y, null, null)
  const fallback = planeIntersection(
    [ray.origin.x, ray.origin.y, ray.origin.z],
    [ray.direction.x, ray.direction.y, ray.direction.z],
    options.planeY ?? 0,
  )
  if (fallback === null) {
    return { point: null, source: 'miss' }
  }
  return {
    point: { x: fallback[0], y: fallback[2], z: fallback[1], onTerrain: false },
    source: 'plane',
  }
}

/** Builds the ray of a canvas position, for a caller that wants to draw it. */
export function pickingRay(scene: Scene, x: number, y: number): Ray {
  return scene.createPickingRay(x, y, null, null)
}

/** World position of a ground point, with the height exaggeration applied. */
export function groundToWorld(point: GroundPoint, exaggeration = 1): Vector3 {
  return new Vector3(point.x, point.z * exaggeration, point.y)
}
