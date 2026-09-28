/*
 * Vector overlays: the drawing half.
 *
 * Each family is one line mesh plus, where a marker helps, a symbol at the point that
 * carries meaning — a connector endpoint, an interface node. A family is disposed when its
 * toggle flips rather than kept alive hidden, because a region set of a large map is a few
 * hundred thousand vertices and an invisible mesh still costs memory.
 *
 * The lines are *ribbons*, not screen-space lines: `GreasedLine` builds a strip of
 * triangles along each path, so a width in metres means something. The plain `LinesMesh`
 * this replaced ignored the width it was given on most backends, which is why the
 * annotations were one pixel wide — and why making them visible meant making them huge, up
 * to a 1.2 m cube per connector endpoint.
 *
 * Geometry comes from `render/overlayGeometry.ts`; this file owns the colours, the widths
 * and the depth offsets.
 */
import { StandardMaterial } from '@babylonjs/core/Materials/standardMaterial'
import { Color3 } from '@babylonjs/core/Maths/math.color'
import { Vector3 } from '@babylonjs/core/Maths/math.vector'
import { CreateDisc } from '@babylonjs/core/Meshes/Builders/discBuilder'
import { CreateGreasedLine } from '@babylonjs/core/Meshes/Builders/greasedLineBuilder'
import { GreasedLineMeshColorMode } from '@babylonjs/core/Materials/GreasedLine/greasedLineMaterialInterfaces'
import type { AbstractMesh } from '@babylonjs/core/Meshes/abstractMesh'
import type { SkeletonNode } from '@/api/types'
import type { Connector, PrmEdge, PrmNode, RegionOutline } from '@/types/sections'
import type { MapScene } from './scene'
import {
  NO_SURFACE,
  OVERLAY_LIFT,
  connectorEndpoints,
  connectorSegments,
  prmSegments,
  regionPaths,
  skeletonEdges,
  type OverlayKind,
  type SurfaceHeight,
  type WorldPoint,
} from './overlayGeometry'

export {
  NO_SURFACE,
  OVERLAY_LIFT,
  connectorEndpoints,
  connectorSegments,
  prmSegments,
  rectEdges,
  regionPaths,
  skeletonEdges,
  type OverlayKind,
  type SurfaceHeight,
  type WorldPoint,
} from './overlayGeometry'

/** How one family is drawn: a colour, a line weight, an opacity and a marker size. */
export interface OverlayStyle {
  /** Line colour, linear RGB in `[0, 1]` because it becomes a `Color3` unchanged. */
  colour: readonly [number, number, number]
  /** Width of the family's lines, metres. */
  widthM: number
  /** Opacity, `[0, 1]`. */
  alpha: number
  /** Radius of the family's markers, metres; absent when the family draws none. */
  markerM?: number
}

/**
 * The drawing of every family, in one table.
 *
 * One table rather than three parallel records, because the values are only meaningful
 * together: an annotation is legible when its colour, its weight and its opacity agree on
 * how important it is, and separate records let them drift apart.
 *
 * The hierarchy is deliberate. Regions and connectors are *content* — they say where a
 * section is — so they are wide enough to read at a glance. The skeleton and the roadmap
 * are *scaffolding*: they explain how the map was built, so they stay thin and faint, and
 * a reader who has not asked about them is not shown them.
 */
const OVERLAY_STYLE: Readonly<Record<OverlayKind, OverlayStyle>> = {
  regions: { colour: [0.78, 0.51, 0.16], widthM: 1.2, alpha: 0.95 },
  connectors: { colour: [0.48, 0.36, 0.72], widthM: 1.0, alpha: 0.9, markerM: 1.4 },
  skeleton: { colour: [0.38, 0.4, 0.44], widthM: 0.25, alpha: 0.35 },
  prm: { colour: [0.05, 0.45, 0.52], widthM: 0.25, alpha: 0.4, markerM: 0.9 },
  // The direction field is drawn by its own class (`render/directionArrows.ts`); this style
  // is the one a legend entry for the family uses.
  direction: { colour: [0.16, 0.45, 0.72], widthM: 0.6, alpha: 0.85 },
}

/** Size of a connector endpoint disc when a family declares no marker size, metres. */
const DEFAULT_MARKER_M = 1.4

/** The drawing of one family, for a caller that annotates it — a legend, a control. */
export function overlayStyle(kind: OverlayKind): OverlayStyle {
  return OVERLAY_STYLE[kind]
}

/** A family's line colour as CSS, for the 2D legend beside the canvas. */
export function overlayColourCss(kind: OverlayKind): string {
  const [r, g, b] = OVERLAY_STYLE[kind].colour
  return `rgb(${Math.round(r * 255)} ${Math.round(g * 255)} ${Math.round(b * 255)})`
}

/** Every overlay family of one map, keyed by kind. */
export class OverlaySet {
  private readonly mapScene: MapScene
  private readonly scene: MapScene['scene']
  private readonly materials = new Map<OverlayKind, StandardMaterial>()
  private readonly parts = new Map<OverlayKind, AbstractMesh[]>()

  constructor(mapScene: MapScene) {
    this.mapScene = mapScene
    this.scene = mapScene.scene
  }

  /** Draws the region outlines, draped on the ground. */
  setRegions(outlines: RegionOutline[], surface: SurfaceHeight = NO_SURFACE): void {
    this.replace('regions', regionPaths(outlines, surface), [])
  }

  /** Draws the connectors as segments with a disc at each endpoint. */
  setConnectors(connectors: Connector[]): void {
    this.replace('connectors', connectorSegments(connectors), connectorEndpoints(connectors))
  }

  /** Draws the quadtree blocks that were aggregated, up to `maxDepth`. */
  setSkeleton(nodes: SkeletonNode[], maxDepth = 6, surface: SurfaceHeight = NO_SURFACE): void {
    this.replace('skeleton', skeletonEdges(nodes, maxDepth, surface), [])
  }

  /** Draws the roadmap edges and marks its interface and connector nodes. */
  setPrm(nodes: PrmNode[], edges: PrmEdge[]): void {
    // Only the nodes that carry meaning get a symbol: a dot on every roadmap sample
    // turns the graph into a field of dots, which hides the edges it exists to show.
    const markers = nodes
      .filter((node) => node.interface || node.connectorEndpoint)
      .map((node): WorldPoint => [
        node.position[0],
        node.position[2] + OVERLAY_LIFT,
        node.position[1],
      ])
    this.replace('prm', prmSegments(nodes, edges), markers, OVERLAY_STYLE.prm.markerM)
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
  private replace(
    kind: OverlayKind,
    lines: WorldPoint[][],
    markers: WorldPoint[],
    markerRadius?: number,
  ): void {
    for (const mesh of this.parts.get(kind) ?? []) {
      // The mesh carried its own colour, so its material goes with it; the shared disc
      // material below is disposed with the set instead.
      this.mapScene.untrackHeightMesh(mesh)
      mesh.dispose()
    }
    const parts: AbstractMesh[] = []
    if (lines.length > 0) {
      parts.push(this.lineMesh(kind, lines))
    }
    for (const marker of markers) {
      parts.push(this.marker(kind, marker, markerRadius))
    }
    this.parts.set(kind, parts)
  }

  /**
   * Builds one mesh holding every line of a family.
   *
   * A family is many disconnected segments — thousands of roadmap edges, dozens of
   * region outlines — and the builder takes them as one list of paths, so a family
   * stays one draw call instead of one per segment.
   */
  private lineMesh(kind: OverlayKind, lines: WorldPoint[][]): AbstractMesh {
    const mesh = CreateGreasedLine(
      `overlay:${kind}`,
      { points: lines.map((line) => line.flat()) },
      {
        width: OVERLAY_STYLE[kind].widthM,
        color: this.colourOf(kind),
        // Scene units, not pixels: the annotation is a metre-wide band on the ground,
        // which is the whole point of drawing it as a ribbon rather than as a line.
        sizeAttenuation: false,
        // The ribbon carries the colour itself, so the mesh needs no shared material;
        // `Mesh.dispose()` releases it with the mesh.
        colorMode: GreasedLineMeshColorMode.COLOR_MODE_SET,
      },
      this.scene,
    )
    mesh.isPickable = false
    mesh.renderingGroupId = 1
    if (mesh.material !== null) {
      // A depth offset rather than a large lift: the annotation hugs the ground it
      // describes while still winning the depth test against it. Small, because an
      // offset large enough to draw a *buried* line through the surface was what made
      // every annotation look painted on the underside of the terrain.
      mesh.material.zOffset = -1
      mesh.material.alpha = OVERLAY_STYLE[kind].alpha
    }
    // Registered with the height exaggeration so the annotation scales with the ground
    // it describes; a family drawn at the map's own metres would sink into a stretched
    // terrain and float over a flattened one.
    this.mapScene.trackHeightMesh(mesh)
    return mesh
  }

  /** Colour of one family. */
  private colourOf(kind: OverlayKind): Color3 {
    const [r, g, b] = OVERLAY_STYLE[kind].colour
    return new Color3(r, g, b)
  }

  /**
   * Builds a flat disc at a point, sized in metres.
   *
   * A disc lies on the ground the way a map symbol does; the cube it replaces stood a
   * metre and a half proud of the surface and, at a campus scale, read as a bigger
   * object than the buildings it was marking.
   */
  private marker(kind: OverlayKind, point: WorldPoint, radius = DEFAULT_MARKER_M): AbstractMesh {
    const disc = CreateDisc(`overlay:${kind}:marker`, { radius, tessellation: 16 }, this.scene)
    disc.rotation.x = Math.PI / 2
    disc.position = new Vector3(point[0], point[1], point[2])
    disc.material = this.materialFor(kind)
    disc.isPickable = false
    disc.renderingGroupId = 1
    this.mapScene.trackHeightMesh(disc)
    return disc
  }

  /** The emissive material a family's discs use, created on first use. */
  private materialFor(kind: OverlayKind): StandardMaterial {
    const existing = this.materials.get(kind)
    if (existing !== undefined) {
      return existing
    }
    const [r, g, b] = OVERLAY_STYLE[kind].colour
    const material = new StandardMaterial(`overlayMaterial:${kind}`, this.scene)
    const color = new Color3(r, g, b)
    material.emissiveColor = color
    material.diffuseColor = color
    material.specularColor = new Color3(0, 0, 0)
    material.disableLighting = true
    material.alpha = OVERLAY_STYLE[kind].alpha
    material.zOffset = -1
    this.materials.set(kind, material)
    return material
  }
}
