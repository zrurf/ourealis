/*
 * Travelling along a polyline, as arithmetic.
 *
 * A route is a *direction*, and a static line cannot say one. The notation for direction is
 * something that moves, and what moves has to be placed from the path's own length: a parameter
 * that advances at a constant speed in metres per second, resolving to a position and a heading at
 * every instant.
 *
 * Pure: a table of cumulative lengths and a lookup, so "where is the marker after two seconds"
 * can be tested without a scene — and so the marker cannot drift away from the line it travels.
 */
import type { Vec2 } from '@/api/types'

/** A point on a polyline, with the direction of travel there. */
export interface PathPoint {
  /** Position in the map plane, metres. */
  at: Vec2
  /** Heading of travel at that point, radians clockwise from east. */
  headingRad: number
  /** Arc length travelled to reach it, metres. */
  arcM: number
  /** Index of the segment's first point, so a caller can interpolate its own per-point data. */
  index: number
  /** Position inside that segment, `0` at its first point and `1` at its second. */
  t: number
}

/** Cumulative arc lengths of a polyline, one entry per point. */
export interface ArcTable {
  /** Points the table was built from. */
  points: readonly Vec2[]
  /** Cumulative length at each point; the last entry is the whole length. */
  arcs: readonly number[]
  /** Total length, metres. */
  lengthM: number
}

/**
 * Builds the arc-length table of a polyline.
 *
 * A marker that stepped proportionally through the *points* would speed up on long segments and
 * crawl on short ones, because a planner's points are not evenly spaced.
 */
export function arcTable(points: readonly Vec2[]): ArcTable {
  const arcs: number[] = [0]
  let total = 0
  for (let index = 1; index < points.length; index += 1) {
    const from = points[index - 1]
    const to = points[index]
    if (from === undefined || to === undefined) {
      continue
    }
    total += Math.hypot(to.x - from.x, to.y - from.y)
    arcs.push(total)
  }
  return { points, arcs, lengthM: total }
}

/**
 * Position and heading at an arc length, wrapped into the path.
 *
 * Wrapping is what makes the marker loop: a route has no end to stop at, and a marker that
 * vanished at the goal would leave the reader wondering whether it had finished or broken.
 */
export function pointAtArc(table: ArcTable, arcM: number): PathPoint | null {
  const { points, arcs, lengthM } = table
  if (points.length === 0) {
    return null
  }
  const first = points[0]
  if (first === undefined) {
    return null
  }
  if (points.length === 1 || !(lengthM > 0)) {
    return { at: { x: first.x, y: first.y }, headingRad: 0, arcM: 0, index: 0, t: 0 }
  }
  let target = arcM % lengthM
  if (target < 0) {
    target += lengthM
  }
  // Linear scan: a route is hundreds of points and this runs once per frame, so a table lookup
  // would cost more to maintain than it saves.
  let index = 1
  while (index < arcs.length - 1 && (arcs[index] ?? 0) < target) {
    index += 1
  }
  const from = points[index - 1] ?? first
  const to = points[index] ?? from
  const startArc = arcs[index - 1] ?? 0
  const span = Math.max(1e-9, (arcs[index] ?? startArc) - startArc)
  const t = Math.min(1, Math.max(0, (target - startArc) / span))
  return {
    at: { x: from.x + (to.x - from.x) * t, y: from.y + (to.y - from.y) * t },
    headingRad: Math.atan2(to.y - from.y, to.x - from.x),
    arcM: target,
    index: index - 1,
    t,
  }
}
