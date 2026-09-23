/*
 * Draping flat geometry onto the terrain.
 *
 * A line in the map plane is not a line in the world: the ground under it rises and
 * falls, so a polyline drawn at a constant height is buried on a hill and floating over a
 * valley. Everything drawn *on* the map — a route, a candidate path, a scale grid —
 * therefore takes its height from the same elevation chunks the surface was built from,
 * through a sampler the caller creates once.
 *
 * Pure geometry: paths are plain `[x, y, z]` triples, so the drape can be tested without
 * an engine.
 */
import type { Aabb, Vec2 } from '@/api/types'
import { terrace } from './shading'

/** A point in world metres. */
export type WorldPoint = [number, number, number]

/** Ground elevation under a map-plane position, or `null` where it is unknown. */
export type SurfaceHeight = (x: number, y: number) => number | null

/** How far above the ground a draped line is drawn, metres. */
export const PATH_LIFT_M = 0.35

/**
 * Wraps a sampler so it answers with the height the surface is *drawn* at.
 *
 * The terrain is terraced, so its drawn height is quantised; a line draped on the
 * un-terraced height would sink into a terrace by up to half a step and climb back out —
 * a route that disappears under the ground it is drawn on. `step` of zero wraps nothing.
 */
export function terracedSampler(surface: SurfaceHeight, step: number): SurfaceHeight {
  if (!(step > 0)) {
    return surface
  }
  return (x: number, y: number): number | null => {
    const value = surface(x, y)
    return value === null ? null : terrace(value, step)
  }
}

/**
 * Drapes a polyline on the terrain.
 *
 * A point without a known height falls back to the last known one rather than to zero:
 * a route that enters a chunk which is still loading should continue at the height it
 * was at, not drop to the map's floor and back.
 */
export function drapePath(
  points: readonly Vec2[],
  surface: SurfaceHeight,
  lift = PATH_LIFT_M,
  fallback = 0,
): WorldPoint[] {
  const out: WorldPoint[] = []
  let height = fallback
  for (const point of points) {
    const sampled = surface(point.x, point.y)
    if (sampled !== null) {
      height = sampled
    }
    out.push([point.x, height + lift, point.y])
  }
  return out
}

/** Options of {@link gridPaths}. */
export interface GridPathOptions {
  /** Height above the ground, metres. */
  lift?: number
  /** Samples per line; a line follows the terrain between its two ends. */
  samples?: number
}

/**
 * A square grid over an extent, draped on the terrain.
 *
 * The step is the caller's (they picked it against the map's scale); the lines are
 * sampled rather than straight, because a straight line between two rim points would
 * disappear into every hill it crossed.
 */
export function gridPaths(
  bounds: Aabb,
  stepM: number,
  surface: SurfaceHeight,
  options: GridPathOptions = {},
): WorldPoint[][] {
  const step = Number.isFinite(stepM) && stepM > 0 ? stepM : 0
  if (step <= 0) {
    return []
  }
  const lift = options.lift ?? 0.15
  const width = bounds.max_x - bounds.min_x
  const depth = bounds.max_y - bounds.min_y
  const samples = Math.max(2, options.samples ?? 64)
  const paths: WorldPoint[][] = []
  const along = (from: Vec2, to: Vec2): WorldPoint[] => {
    const line: Vec2[] = []
    for (let index = 0; index <= samples; index += 1) {
      const t = index / samples
      line.push({ x: from.x + (to.x - from.x) * t, y: from.y + (to.y - from.y) * t })
    }
    return drapePath(line, surface, lift)
  }
  for (let x = bounds.min_x; x <= bounds.max_x + 1e-9; x += step) {
    paths.push(along({ x, y: bounds.min_y }, { x, y: bounds.max_y }))
  }
  for (let y = bounds.min_y; y <= bounds.max_y + 1e-9; y += step) {
    paths.push(along({ x: bounds.min_x, y }, { x: bounds.max_x, y }))
  }
  // A grid that stops one step short of the far edge looks broken; the extent's own
  // rim is added when the step does not land on it.
  if (width > 0 && !landsOn(bounds.min_x, bounds.max_x, step)) {
    paths.push(along({ x: bounds.max_x, y: bounds.min_y }, { x: bounds.max_x, y: bounds.max_y }))
  }
  if (depth > 0 && !landsOn(bounds.min_y, bounds.max_y, step)) {
    paths.push(along({ x: bounds.min_x, y: bounds.max_y }, { x: bounds.max_x, y: bounds.max_y }))
  }
  return paths
}

/** Whether a step from `from` lands exactly on `to`. */
function landsOn(from: number, to: number, step: number): boolean {
  const span = to - from
  if (span <= 0) {
    return true
  }
  const steps = Math.round(span / step)
  return Math.abs(steps * step - span) < 1e-6
}
