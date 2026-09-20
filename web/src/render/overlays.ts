/*
 * Vector overlays: region outlines, connectors, skeleton blocks and roadmap edges.
 *
 * Each family is one line system plus, where a marker helps, a small block at the
 * point that carries meaning — a connector endpoint, an interface node. A family
 * is disposed when its toggle flips rather than kept alive hidden, because a
 * region set of a large map is a few hundred thousand vertices and an invisible
 * mesh still costs memory.
 *
 * Geometry is built as plain `[x, y, z]` triples and converted to Babylon vectors
 * only when a mesh is created, so the shape of an overlay can be reasoned about —
 * and tested — without an engine.
 */
import { StandardMaterial } from '@babylonjs/core/Materials/standardMaterial'
import { Color3 } from '@babylonjs/core/Maths/math.color'
import { Vector3 } from '@babylonjs/core/Maths/math.vector'
import { CreateBox } from '@babylonjs/core/Meshes/Builders/boxBuilder'
import { CreateLineSystem } from '@babylonjs/core/Meshes/Builders/linesBuilder'
import type { AbstractMesh } from '@babylonjs/core/Meshes/abstractMesh'
import type { LinesMesh } from '@babylonjs/core/Meshes/linesMesh'
import type { Scene } from '@babylonjs/core/scene'
import type { Aabb, SkeletonNode } from '@/api/types'
import {
  skeletonBlocks,
  type Connector,
  type PrmEdge,
  type PrmNode,
  type RegionOutline,
} from '@/types/sections'

/** Overlay families a viewer can toggle. */
export type OverlayKind = 'regions' | 'connectors' | 'skeleton' | 'prm'

/** A point in world metres. */
export type WorldPoint = [number, number, number]

/** Colours of the overlay families, drawn as lines over the terrain. */
const OVERLAY_COLORS: Readonly<Record<OverlayKind, readonly [number, number, number]>> = {
  regions: [0.72, 0.47, 0.2],
  connectors: [0.42, 0.35, 0.63],
  skeleton: [0.42, 0.42, 0.45],
  prm: [0.25, 0.5, 0.54],
}

/** Height above the terrain at which the overlays are drawn, metres. */
const OVERLAY_LIFT = 1.5

/** Outlines of the regions, as closed polylines lifted off the ground. */
export function regionPaths(outlines: RegionOutline[]): WorldPoint[][] {
  return outlines.map((outline) =>
    outline.points.map((point): WorldPoint => [point.x, OVERLAY_LIFT, point.y]),
  )
}

/** Segments of the connectors, lifted off the ground. */
export function connectorSegments(connectors: Connector[]): WorldPoint[][] {
  return connectors.map((connector) => [
    [connector.a[0], connector.a[1] + OVERLAY_LIFT, connector.a[2]],
    [connector.b[0], connector.b[1] + OVERLAY_LIFT, connector.b[2]],
  ])
}

/** Segments of the roadmap edges, lifted off the ground. */
export function prmSegments(nodes: PrmNode[], edges: PrmEdge[]): WorldPoint[][] {
  const segments: WorldPoint[][] = []
  for (const edge of edges) {
    const from = nodes[edge.from]
    const to = nodes[edge.to]
    if (from === undefined || to === undefined) {
      continue
    }
    segments.push([
      [from.position[0], from.position[1] + OVERLAY_LIFT, from.position[2]],
      [to.position[0], to.position[1] + OVERLAY_LIFT, to.position[2]],
    ])
  }
  return segments
}

/** Edges of one skeleton block: a bottom and a top rectangle joined at the corners. */
export function boxEdges(bounds: Aabb, height = 2, lift = 0): WorldPoint[][] {
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
      [current[0], lift, current[1]],
      [next[0], lift, next[1]],
    ])
    edges.push([
      [current[0], lift + height, current[1]],
      [next[0], lift + height, next[1]],
    ])
    edges.push([
      [current[0], lift, current[1]],
      [current[0], lift + height, current[1]],
    ])
  }
  return edges
}

/** Edges of every skeleton block a viewer draws. */
export function skeletonEdges(nodes: SkeletonNode[], maxDepth = 6): WorldPoint[][] {
  const edges: WorldPoint[][] = []
  for (const node of skeletonBlocks(nodes, maxDepth)) {
    edges.push(...boxEdges(node.bounds, 2, OVERLAY_LIFT))
  }
  return edges
}

/** Every overlay family of one map, keyed by kind. */
export class OverlaySet {
  private readonly scene: Scene
  private readonly materials = new Map<OverlayKind, StandardMaterial>()
  private readonly parts = new Map<OverlayKind, AbstractMesh[]>()

  constructor(scene: Scene) {
    this.scene = scene
  }

  /** Draws the region outlines. */
  setRegions(outlines: RegionOutline[]): void {
    this.replace('regions', regionPaths(outlines), [])
  }

  /** Draws the connectors as segments with a block at each endpoint. */
  setConnectors(connectors: Connector[]): void {
    const markers: WorldPoint[] = []
    for (const connector of connectors) {
      markers.push(connector.a, connector.b)
    }
    this.replace('connectors', connectorSegments(connectors), markers)
  }

  /** Draws the quadtree blocks that were aggregated, up to `maxDepth`. */
  setSkeleton(nodes: SkeletonNode[], maxDepth = 6): void {
    this.replace('skeleton', skeletonEdges(nodes, maxDepth), [])
  }

  /** Draws the roadmap edges and marks its interface and connector nodes. */
  setPrm(nodes: PrmNode[], edges: PrmEdge[]): void {
    const markers = nodes
      .filter((node) => node.interface || node.connectorEndpoint)
      .map((node): WorldPoint => [
        node.position[0],
        node.position[1] + OVERLAY_LIFT,
        node.position[2],
      ])
    this.replace('prm', prmSegments(nodes, edges), markers)
  }

  /** Shows or hides one family; the geometry stays loaded. */
  setVisible(kind: OverlayKind, visible: boolean): void {
    for (const mesh of this.parts.get(kind) ?? []) {
      mesh.setEnabled(visible)
    }
  }

  /** True when a family currently has geometry. */
  has(kind: OverlayKind): boolean {
    return (this.parts.get(kind)?.length ?? 0) > 0
  }

  /** Removes every family. */
  clear(): void {
    // A copy is required: `replace` rewrites the map being iterated.
    for (const kind of Array.from(this.parts.keys())) {
      this.replace(kind, [], [])
    }
  }

  /** Disposes every mesh and material. */
  dispose(): void {
    this.clear()
    for (const material of this.materials.values()) {
      material.dispose()
    }
    this.materials.clear()
  }

  /** Replaces one family's meshes, disposing what it held. */
  private replace(kind: OverlayKind, lines: WorldPoint[][], markers: WorldPoint[]): void {
    for (const mesh of this.parts.get(kind) ?? []) {
      mesh.dispose()
    }
    const parts: AbstractMesh[] = []
    if (lines.length > 0) {
      parts.push(this.lineMesh(kind, lines))
    }
    for (const marker of markers) {
      parts.push(this.marker(kind, marker))
    }
    this.parts.set(kind, parts)
  }

  /** Builds a line mesh in the family's colour. */
  private lineMesh(kind: OverlayKind, lines: WorldPoint[][]): LinesMesh {
    const mesh = CreateLineSystem(
      `overlay:${kind}`,
      { lines: lines.map(toVectors), updatable: false },
      this.scene,
    )
    mesh.material = this.materialFor(kind)
    mesh.isPickable = false
    mesh.renderingGroupId = 1
    return mesh
  }

  /** Builds a small block at a point, sized in metres. */
  private marker(kind: OverlayKind, point: WorldPoint, size = 1.2): AbstractMesh {
    const box = CreateBox(`overlay:${kind}:marker`, { size }, this.scene)
    box.position = new Vector3(point[0], point[1], point[2])
    box.material = this.materialFor(kind)
    box.isPickable = false
    box.renderingGroupId = 1
    return box
  }

  /** The emissive material of one family, created on first use. */
  private materialFor(kind: OverlayKind): StandardMaterial {
    const existing = this.materials.get(kind)
    if (existing !== undefined) {
      return existing
    }
    const [r, g, b] = OVERLAY_COLORS[kind]
    const material = new StandardMaterial(`overlayMaterial:${kind}`, this.scene)
    const color = new Color3(r, g, b)
    material.emissiveColor = color
    material.diffuseColor = color
    material.specularColor = new Color3(0, 0, 0)
    material.disableLighting = true
    this.materials.set(kind, material)
    return material
  }
}

/** Converts a path of world triples into Babylon vectors. */
function toVectors(path: WorldPoint[]): Vector3[] {
  return path.map(([x, y, z]) => new Vector3(x, y, z))
}
