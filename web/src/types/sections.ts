/*
 * Readers for the optional map sections.
 *
 * The service passes these sections through as JSON with map-format's own field
 * names, so the shapes arrive untyped. These functions narrow them once, in one
 * place, and hand the renderer an array it can trust: a payload that is missing a
 * field yields a default rather than a crash, because a viewer that refuses to
 * draw a map over one malformed outline hides everything else too.
 */
import type { Aabb, SkeletonNode } from '@/api/types'
import type { ConnectorSection, PrmGraphSection, RegionSection } from '@/api/maps'

/** One region outline in the metre plane. */
export interface RegionOutline {
  /** Index of the outline inside the section; features reference it by `geom_ref`. */
  index: number
  /** Vertices of the outline, metres. */
  points: Array<{ x: number; y: number }>
}

/** One Z-axis connector. */
export interface Connector {
  /** Kind: `stair`, `elevator`, `overpass`, `underpass` or `loop`. */
  type: string | null
  /** Traversal direction: `both`, `a_to_b` or `b_to_a`. */
  direction: string
  /** Endpoint A, metres, including altitude. */
  a: [number, number, number]
  /** Endpoint B, metres, including altitude. */
  b: [number, number, number]
  /** Geometric length along the connector, metres. */
  length_3d_m: number
}

/** One roadmap waypoint. */
export interface PrmNode {
  /** Position in local metres; `z` is the terrain elevation at that point. */
  position: [number, number, number]
  /** Whether the node stitches the roadmap to the fine grid. */
  interface: boolean
  /** Whether the node coincides with a connector endpoint. */
  connectorEndpoint: boolean
  /** Whether the node lies in a direction-constrained area. */
  directionConstrained: boolean
}

/** One roadmap edge, with its source node spelled out. */
export interface PrmEdge {
  /** Source node index. */
  from: number
  /** Target node index. */
  to: number
  /** Geometric length, metres. */
  len_m: number
  /** Preference-weighted cost in equivalent metres. */
  cost_equiv_m: number
}

/** A region feature, as far as a viewer reads it. */
export interface RegionFeature {
  /** Semantic tag id. */
  tag_id: number
  /** Tag name when the service recognises the id. */
  tag: string | null
  /** Outline this feature refers to. */
  geom_ref: number
}

/** Narrows every outline of a region section. */
export function regionOutlines(section: RegionSection | null): RegionOutline[] {
  const outlines = section?.outlines
  if (!Array.isArray(outlines)) {
    return []
  }
  const out: RegionOutline[] = []
  for (const entry of outlines) {
    const record = asRecord(entry)
    const index = asNumber(record?.index, out.length)
    const points = Array.isArray(record?.points) ? record.points : []
    out.push({ index, points: points.map(asPoint).filter(isPoint) })
  }
  return out
}

/** Narrows every feature of a region section. */
export function regionFeatures(section: RegionSection | null): RegionFeature[] {
  const features = section?.features
  if (!Array.isArray(features)) {
    return []
  }
  return features.map((entry) => {
    const record = asRecord(entry)
    return {
      tag_id: asNumber(record?.tag_id, 0),
      tag: typeof record?.tag === 'string' ? record.tag : null,
      geom_ref: asNumber(record?.geom_ref, 0),
    }
  })
}

/** Narrows the connector table. */
export function connectors(section: ConnectorSection | null): Connector[] {
  const entries = section?.connectors
  if (!Array.isArray(entries)) {
    return []
  }
  return entries.map((entry) => {
    const record = asRecord(entry)
    return {
      type: typeof record?.type === 'string' ? record.type : null,
      direction: typeof record?.direction === 'string' ? record.direction : 'both',
      a: asTriple(record?.a),
      b: asTriple(record?.b),
      length_3d_m: asNumber(record?.length_3d_m, 0),
    }
  })
}

/** Narrows the roadmap nodes of a PRM section. */
export function prmNodes(section: PrmGraphSection | null): PrmNode[] {
  const nodes = section?.nodes
  if (!Array.isArray(nodes)) {
    return []
  }
  return nodes.map((entry) => {
    const record = asRecord(entry)
    return {
      position: asTriple(record?.position),
      interface: record?.interface === true,
      connectorEndpoint: record?.connector_endpoint === true,
      directionConstrained: record?.direction_constrained === true,
    }
  })
}

/** Narrows the roadmap edges of a PRM section. */
export function prmEdges(section: PrmGraphSection | null): PrmEdge[] {
  const edges = section?.edges
  if (!Array.isArray(edges)) {
    return []
  }
  const out: PrmEdge[] = []
  for (const entry of edges) {
    const record = asRecord(entry)
    const from = asNumber(record?.from, -1)
    const to = asNumber(record?.to, -1)
    if (from < 0 || to < 0) {
      continue
    }
    out.push({
      from,
      to,
      len_m: asNumber(record?.len_m, 0),
      cost_equiv_m: asNumber(record?.cost_equiv_m, 0),
    })
  }
  return out
}

/**
 * Skeleton blocks worth drawing.
 *
 * A block is drawn when it aggregates something or carries a constraint — an
 * interior node, an aggregate maximum, a drill hint, a forbidden or
 * direction-constrained area. A plain leaf is the fine grid itself, and drawing one
 * box per leaf would cover the surface with wireframe instead of summarising it.
 */
export function skeletonBlocks(nodes: SkeletonNode[], maxDepth = 6): SkeletonNode[] {
  return nodes.filter(
    (node) =>
      node.depth <= maxDepth &&
      (!node.leaf ||
        node.has_aggregate_max ||
        node.drill_hint ||
        node.suspect_forbidden ||
        node.direction_constrained),
  )
}

/** Extent of every block of a skeleton, for a camera fit. */
export function boundsOfNodes(nodes: SkeletonNode[]): Aabb | null {
  let bounds: Aabb | null = null
  for (const node of nodes) {
    bounds =
      bounds === null
        ? { ...node.bounds }
        : {
            min_x: Math.min(bounds.min_x, node.bounds.min_x),
            min_y: Math.min(bounds.min_y, node.bounds.min_y),
            max_x: Math.max(bounds.max_x, node.bounds.max_x),
            max_y: Math.max(bounds.max_y, node.bounds.max_y),
          }
  }
  return bounds
}

/** Reads a value as an object, or `undefined` for anything else. */
function asRecord(value: unknown): Record<string, unknown> | undefined {
  return typeof value === 'object' && value !== null
    ? (value as Record<string, unknown>)
    : undefined
}

/** Reads a number, or the fallback when the value is not a finite one. */
function asNumber(value: unknown, fallback: number): number {
  return typeof value === 'number' && Number.isFinite(value) ? value : fallback
}

/** Reads a three-component position, defaulting each missing component to zero. */
function asTriple(value: unknown): [number, number, number] {
  if (!Array.isArray(value)) {
    return [0, 0, 0]
  }
  return [asNumber(value[0], 0), asNumber(value[1], 0), asNumber(value[2], 0)]
}

/** Reads one outline vertex. */
function asPoint(value: unknown): { x: number; y: number } | undefined {
  if (Array.isArray(value)) {
    return { x: asNumber(value[0], 0), y: asNumber(value[1], 0) }
  }
  const record = asRecord(value)
  if (record === undefined) {
    return undefined
  }
  return { x: asNumber(record.x, 0), y: asNumber(record.y, 0) }
}

/** Type guard for {@link asPoint}'s result. */
function isPoint(value: { x: number; y: number } | undefined): value is { x: number; y: number } {
  return value !== undefined
}
