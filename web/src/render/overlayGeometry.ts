/*
 * Vector overlays: region outlines, connectors, skeleton blocks and roadmap edges.
 *
 * Each family is one line mesh plus, where a marker helps, a symbol at the point that
 * carries meaning — a connector endpoint, an interface node. A family is disposed when
 * its toggle flips rather than kept alive hidden, because a region set of a large map
 * is a few hundred thousand vertices and an invisible mesh still costs memory.
 *
 * The lines are *ribbons*, not screen-space lines: `GreasedLine` builds a strip of
 * triangles along each path, so a width in metres means something. The plain
 * `LinesMesh` this replaced ignored the width it was given on most backends, which is
 * why the annotations were one pixel wide — and why making them visible meant making
 * them huge, up to a 1.2 m cube per connector endpoint.
 *
 * Two conventions hold the annotations onto the map, and getting either wrong is visible
 * as "the overlay is floating". First, a stored position is `[x, y, z]` where `z` is the
 * altitude: a connector endpoint and a roadmap node carry the elevation they sit at, so
 * the world triple is `(x, z, y)` — reading the map-plane `y` as the height put every
 * connector in the air. Second, a shape that has no altitude of its own (a region outline,
 * a skeleton block) is *draped*: its height comes from the surface sampler at its own
 * horizontal position, never from a constant, because a constant is below the ground on a
 * hill and above it in a valley.
 *
 * Pure: plain `[x, y, z]` triples, so the shape of an overlay can be reasoned about — and
 * tested — without an engine. `render/overlays.ts` draws them.
 */
import type { Aabb, SkeletonNode } from '@/api/types'
import {
  skeletonBlocks,
  type Connector,
  type PrmEdge,
  type PrmNode,
  type RegionOutline,
} from '@/types/sections'

/**
 * Overlay families a viewer can toggle.
 *
 * The first four come from the map's own sections; `direction` is the direction-constraint
 * field drawn as arrows, which is a *reading* of a cell layer rather than a section, but is
 * toggled and styled like the others.
 */
export type OverlayKind = 'regions' | 'connectors' | 'skeleton' | 'prm' | 'direction'

/** A point in world metres. */
export type WorldPoint = [number, number, number]

/** Ground elevation under a map-plane position, or `null` where it is unknown. */
export type SurfaceHeight = (x: number, y: number) => number | null

/** A plane that samples as no ground at all; the fallback for a caller without terrain. */
export const NO_SURFACE: SurfaceHeight = () => null

/**
 * Height above the ground at which the overlays are drawn, metres.
 *
 * The depth offset in the material keeps an annotation from z-fighting with the
 * surface it follows, so this only has to be large enough to clear the terrain's own
 * vertex spacing — it is not what makes the line visible.
 */
export const OVERLAY_LIFT = 0.2

/** Reads a stored `[x, y, z]` triple, whose `z` is the altitude, as a world position. */
function storedPoint(point: readonly [number, number, number], lift: number): WorldPoint {
  return [point[0], point[2] + lift, point[1]]
}

/** Places a map-plane point on the sampled ground, lifted by {@link OVERLAY_LIFT}. */
function groundPoint(x: number, y: number, surface: SurfaceHeight, lift: number): WorldPoint {
  return [x, (surface(x, y) ?? 0) + lift, y]
}

/** Outlines of the regions, draped on the ground they enclose. */
export function regionPaths(
  outlines: RegionOutline[],
  surface: SurfaceHeight = NO_SURFACE,
  lift = OVERLAY_LIFT,
): WorldPoint[][] {
  return outlines.map((outline) =>
    outline.points.map((point): WorldPoint => groundPoint(point.x, point.y, surface, lift)),
  )
}

/**
 * Segments of the connectors.
 *
 * An endpoint keeps the altitude the file stores for it: a stair or an overpass links
 * two levels, and its ends are at those levels rather than on the ground between them.
 */
export function connectorSegments(connectors: Connector[], lift = OVERLAY_LIFT): WorldPoint[][] {
  return connectors.map((connector) => [
    storedPoint(connector.a, lift),
    storedPoint(connector.b, lift),
  ])
}

/** World positions of the two ends of every connector, for the endpoint markers. */
export function connectorEndpoints(connectors: Connector[], lift = OVERLAY_LIFT): WorldPoint[] {
  const points: WorldPoint[] = []
  for (const connector of connectors) {
    points.push(storedPoint(connector.a, lift), storedPoint(connector.b, lift))
  }
  return points
}

/** Segments of the roadmap edges, at the elevation their nodes carry. */
export function prmSegments(
  nodes: PrmNode[],
  edges: PrmEdge[],
  lift = OVERLAY_LIFT,
): WorldPoint[][] {
  const segments: WorldPoint[][] = []
  for (const edge of edges) {
    const from = nodes[edge.from]
    const to = nodes[edge.to]
    if (from === undefined || to === undefined) {
      continue
    }
    segments.push([storedPoint(from.position, lift), storedPoint(to.position, lift)])
  }
  return segments
}

/**
 * Outline of one skeleton block, lying on the ground.
 *
 * A closed rectangle rather than a box: a block is a footprint, and a 0.6 m prism
 * around every aggregate stood proud of the surface over the whole map, which read as
 * scaffolding rather than as a subdivision.
 */
export function rectEdges(
  bounds: Aabb,
  surface: SurfaceHeight = NO_SURFACE,
  lift = OVERLAY_LIFT,
): WorldPoint[][] {
  const corners: Array<[number, number]> = [
    [bounds.min_x, bounds.min_y],
    [bounds.max_x, bounds.min_y],
    [bounds.max_x, bounds.max_y],
    [bounds.min_x, bounds.max_y],
  ]
  const edges: WorldPoint[][] = []
  for (let index = 0; index < corners.length; index += 1) {
    const current = corners[index]
    const next = corners[(index + 1) % corners.length]
    if (current === undefined || next === undefined) {
      continue
    }
    edges.push([
      groundPoint(current[0], current[1], surface, lift),
      groundPoint(next[0], next[1], surface, lift),
    ])
  }
  return edges
}

/** Outlines of every skeleton block a viewer draws. */
export function skeletonEdges(
  nodes: SkeletonNode[],
  maxDepth = 6,
  surface: SurfaceHeight = NO_SURFACE,
  lift = OVERLAY_LIFT,
): WorldPoint[][] {
  const edges: WorldPoint[][] = []
  for (const node of skeletonBlocks(nodes, maxDepth)) {
    edges.push(...rectEdges(node.bounds, surface, lift))
  }
  return edges
}
