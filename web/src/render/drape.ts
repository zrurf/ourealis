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
import type { PlaneOrigin } from '@/types/map'
import type { CellSampler } from './cellSampler'
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
 * The height the terrain mesh is *drawn* at, from the cells it was built from.
 *
 * `buildChunkMesh` puts one vertex at each cell's centre and lets the rasteriser fill the
 * quads between them, so the visible surface between two cells is a ramp, not a step.
 * Reading back the single cell that contains a point instead gives a staircase, and a line
 * draped on a staircase disappears under the ramp on the way up and hangs in the air on
 * the way down — by up to half the height difference between neighbouring cells, which on
 * a terraced campus map is metres. Quantisation belongs to each cell centre, before the
 * interpolation, exactly as the mesh builder applies it; re-quantising the interpolated
 * value would reintroduce the very staircase this removes.
 *
 * `null` means one of the four surrounding cells is not loaded, which is different from a
 * cell whose height is zero.
 */
export function meshSurfaceSampler(
  cell: CellSampler,
  cellSize: number,
  origin: PlaneOrigin,
  step = 0,
): SurfaceHeight {
  // Cell centres sit at `(index + 0.5) * cellSize` from the plane origin, so a map position
  // lands in the cell-centre lattice half a cell further along than its own cell index.
  const inLattice = (value: number, along: number): number => (value - along) / cellSize - 0.5
  const height = (i: number, j: number): number | null => {
    const value = cell(i, j)
    if (value === null || !Number.isFinite(value)) {
      return null
    }
    return step > 0 ? terrace(value, step) : value
  }
  return (x: number, y: number): number | null => {
    if (!(cellSize > 0)) {
      return null
    }
    const fx = inLattice(x, origin.x)
    const fy = inLattice(y, origin.y)
    const i0 = Math.floor(fx)
    const j0 = Math.floor(fy)
    const tx = fx - i0
    const ty = fy - j0
    const near = height(i0, j0)
    const nearUp = height(i0, j0 + 1)
    if (near === null || nearUp === null) {
      return null
    }
    const low = near + (nearUp - near) * ty
    const far = height(i0 + 1, j0)
    const farUp = height(i0 + 1, j0 + 1)
    // A column that has not loaded degrades to the one that has: the line keeps following
    // the ground it can see instead of dropping the segment.
    if (far === null || farUp === null) {
      return low
    }
    return low + (far + (farUp - far) * ty - low) * tx
  }
}

/** Options of {@link drapePath}. */
export interface DrapeOptions {
  /** How far above the ground the line is drawn, metres. */
  lift?: number
  /** Height used where the ground is unknown, metres. */
  fallback?: number
  /**
   * Straight-line spacing between samples, metres.
   *
   * A planner's vertices can be tens of metres apart, and a straight chord between two of
   * them cuts through every rise between: sampling only the vertices leaves a line that
   * disappears under each hill it crosses and reappears on the far side. Half a cell is
   * the finest spacing that still catches every rise the surface can express. Zero or less
   * samples the vertices only, which is what a path that is already dense wants.
   */
  spacingM?: number
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
  options: DrapeOptions = {},
): WorldPoint[] {
  const lift = options.lift ?? PATH_LIFT_M
  const fallback = options.fallback ?? 0
  const spacing = options.spacingM ?? 0
  const out: WorldPoint[] = []
  let height = fallback
  for (const point of resample(points, spacing)) {
    const sampled = surface(point.x, point.y)
    if (sampled !== null) {
      height = sampled
    }
    out.push([point.x, height + lift, point.y])
  }
  return out
}

/**
 * Inserts intermediate points so no segment is longer than `spacingM`.
 *
 * A spacing of zero or less returns the input untouched, and a segment shorter than the
 * spacing is not subdivided at all — the point count stays bounded by the path's own
 * length rather than by a fixed multiplier.
 */
export function resample(points: readonly Vec2[], spacingM: number): Vec2[] {
  if (!(spacingM > 0) || points.length < 2) {
    return [...points]
  }
  const out: Vec2[] = []
  for (let index = 0; index + 1 < points.length; index += 1) {
    const from = points[index]
    const to = points[index + 1]
    if (from === undefined || to === undefined) {
      continue
    }
    out.push(from)
    const span = Math.hypot(to.x - from.x, to.y - from.y)
    const steps = Math.floor(span / spacingM)
    for (let step = 1; step < steps; step += 1) {
      const t = step / steps
      out.push({ x: from.x + (to.x - from.x) * t, y: from.y + (to.y - from.y) * t })
    }
  }
  const last = points[points.length - 1]
  if (last !== undefined) {
    out.push(last)
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
    return drapePath(line, surface, { lift })
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
