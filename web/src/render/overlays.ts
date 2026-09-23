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

/** Colours of the overlay families, drawn over the terrain in the series palette. */
const OVERLAY_COLORS: Readonly<Record<OverlayKind, readonly [number, number, number]>> = {
  // Saturated annotation colours: the surface itself is neutral, so a family is
  // identified by its colour alone rather than by being darker than a green ramp.
  regions: [0.87, 0.42, 0.12],
  connectors: [0.55, 0.28, 0.75],
  skeleton: [0.38, 0.4, 0.44],
  prm: [0.05, 0.48, 0.55],
  // The direction field is drawn by its own class (`render/directionArrows.ts`); this colour
  // is the one a marker or a legend entry for the family uses.
  direction: [0.16, 0.45, 0.72],
}

/** Width of each family's lines, metres. */
const OVERLAY_WIDTH_M: Readonly<Record<OverlayKind, number>> = {
  regions: 0.8,
  connectors: 1.0,
  skeleton: 0.3,
  prm: 0.28,
  direction: 0.6,
}

/** Opacity of each family: annotations, not obstacles. */
const OVERLAY_ALPHA: Readonly<Record<OverlayKind, number>> = {
  regions: 0.95,
  connectors: 0.9,
  skeleton: 0.45,
  prm: 0.5,
  direction: 0.85,
}

/** Size of a connector endpoint disc, metres. */
const CONNECTOR_DISC_M = 1.4

/** Radius of a roadmap node dot, metres. */
const PRM_NODE_M = 0.9

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
    this.replace('prm', prmSegments(nodes, edges), markers, PRM_NODE_M)
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
    markerRadius = CONNECTOR_DISC_M,
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
        width: OVERLAY_WIDTH_M[kind],
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
      mesh.material.alpha = OVERLAY_ALPHA[kind]
    }
    // Registered with the height exaggeration so the annotation scales with the ground
    // it describes; a family drawn at the map's own metres would sink into a stretched
    // terrain and float over a flattened one.
    this.mapScene.trackHeightMesh(mesh)
    return mesh
  }

  /** Colour of one family, from the series palette. */
  private colourOf(kind: OverlayKind): Color3 {
    const [r, g, b] = OVERLAY_COLORS[kind]
    return new Color3(r, g, b)
  }

  /**
   * Builds a flat disc at a point, sized in metres.
   *
   * A disc lies on the ground the way a map symbol does; the cube it replaces stood a
   * metre and a half proud of the surface and, at a campus scale, read as a bigger
   * object than the buildings it was marking.
   */
  private marker(kind: OverlayKind, point: WorldPoint, radius = CONNECTOR_DISC_M): AbstractMesh {
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
    const [r, g, b] = OVERLAY_COLORS[kind]
    const material = new StandardMaterial(`overlayMaterial:${kind}`, this.scene)
    const color = new Color3(r, g, b)
    material.emissiveColor = color
    material.diffuseColor = color
    material.specularColor = new Color3(0, 0, 0)
    material.disableLighting = true
    material.alpha = OVERLAY_ALPHA[kind]
    material.zOffset = -1
    this.materials.set(kind, material)
    return material
  }
}
