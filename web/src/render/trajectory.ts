/*
 * The trajectory line in the scene: a speed-coloured polyline and a runner marker.
 *
 * The geometry itself — points, heights and the line-list buffers — lives in
 * `render/trajectoryGeometry.ts`, pure and testable. This is the thin Babylon side: a
 * {@link TrajectoryLine} owns the two line meshes and the marker, and is the one object a
 * view talks to. Its seam is documented rather than implied: {@link TrajectoryPoint} is
 * the only input the line accepts, and {@link TrajectoryLine.setPose} is the one call the
 * playback clock needs.
 *
 * The line is a plain line list — `VertexData` with a `LineListDrawMode` material, no
 * ribbon builder. It is the one drawing path that renders identically on every backend
 * the fallback chain can reach, which is why a track is drawn with it even though its
 * width is a fixed pixel.
 */
import { StandardMaterial } from '@babylonjs/core/Materials/standardMaterial'
import { Color3 } from '@babylonjs/core/Maths/math.color'
import { Vector3 } from '@babylonjs/core/Maths/math.vector'
import { CreateBox } from '@babylonjs/core/Meshes/Builders/boxBuilder'
import { CreateCylinder } from '@babylonjs/core/Meshes/Builders/cylinderBuilder'
import { Material } from '@babylonjs/core/Materials/material'
import { Mesh } from '@babylonjs/core/Meshes/mesh'
import { VertexData } from '@babylonjs/core/Meshes/mesh.vertexData'
import type { Scene } from '@babylonjs/core/scene'
import { speedColor } from '@/types/colormap'
import {
  drawHeight,
  pointAtTime,
  trajectoryGeometry,
  trajectoryRibbon,
  type TrajectoryGeometryOptions,
  type TrajectoryPoint,
} from './trajectoryGeometry'

/**
 * Re-exported so a view keeps importing everything trajectory-shaped from one module,
 * the way the layer drapes re-export their texture builders.
 */
export {
  channelValue,
  drawHeight,
  pointAtTime,
  sampleTrajectory,
  trajectoryGeometry,
  trajectoryRibbon,
  type TrajectoryChannel,
  type TrajectoryGeometry,
  type TrajectoryGeometryOptions,
  type TrajectoryPoint,
} from './trajectoryGeometry'

/**
 * Lift of the runner marker above the height the line is drawn at, metres.
 *
 * Large, because the marker's job is to be findable: at a whole-campus view a marker sized
 * to the runner is a pixel, and a reader looking for "where is it now" cannot find one. It
 * is an annotation on the map rather than a scale model of a person.
 */
const MARKER_LENGTH_M = 22

/** Width of the runner marker across its head, metres. */
const MARKER_HEAD_M = 16

/** Length of the marker's tail behind the head, metres. */
const MARKER_TAIL_M = 12

/** How far the marker rides above the surface it is drawn on, metres. */
const MARKER_LIFT_M = 1.0

/** Width of the ribbon drawn beneath the hairline, metres. */
const RIBBON_WIDTH_M = 8

/** A trajectory drawn over the terrain, with a marker at a chosen time. */
export class TrajectoryLine {
  private readonly path: Mesh
  private readonly pathRibbon: Mesh
  private readonly travelled: Mesh
  private readonly travelledRibbon: Mesh
  private readonly marker: Mesh
  private points: TrajectoryPoint[] = []
  /** How the line is held off the ground; kept so a later band is drawn the same way. */
  private options: TrajectoryGeometryOptions = {}
  /** Index the travelled band currently reaches, so a still playhead rebuilds nothing. */
  private drawnTo = -1

  constructor(scene: Scene) {
    this.pathRibbon = ribbonMesh('trajectoryBand', scene, 0, 0.34)
    this.path = lineMesh('trajectoryPath', scene, 0)
    // The band's material is white because its vertices carry the amber themselves; two
    // colours would multiply into mud.
    this.travelledRibbon = ribbonMesh('trajectoryTravelledBand', scene, 1, 0.5)
    this.travelled = lineMesh('trajectoryTravelled', scene, 1)
    this.marker = buildArrow(scene)
  }

  /** Replaces the drawn trajectory. */
  setTrajectory(points: readonly TrajectoryPoint[], options: TrajectoryGeometryOptions = {}): void {
    this.points = [...points]
    this.options = options
    this.drawnTo = -1
    const colorOf = (point: TrajectoryPoint): readonly [number, number, number] => {
      const [r, g, b] = speedColor(point.value, ...this.valueRange(points))
      return [r / 255, g / 255, b / 255] as const
    }
    this.apply(this.path, points, colorOf)
    this.applyRibbon(this.pathRibbon, points, colorOf)
    const drawn = points.length > 1
    this.path.setEnabled(drawn)
    this.pathRibbon.setEnabled(drawn)
    // The band is rebuilt by the next progress update; clearing it here keeps a stale
    // run's band from surviving a channel switch.
    this.apply(this.travelled, [], undefined)
    this.applyRibbon(this.travelledRibbon, [], undefined)
    this.travelled.setEnabled(false)
    this.travelledRibbon.setEnabled(false)
  }

  /**
   * Draws the part of the trajectory already run.
   *
   * `index` is a position in the drawn points; `0` hides the band, which is what the start
   * of a playback looks like. The band is rebuilt only when the index actually moves, so a
   * paused playhead — or a frame that advanced less than one drawn point — costs nothing.
   */
  setProgress(index: number): void {
    const target = Math.max(0, Math.min(this.points.length - 1, Math.trunc(index)))
    if (target === this.drawnTo) {
      return
    }
    this.drawnTo = target
    if (target < 1) {
      this.travelled.setEnabled(false)
      this.travelledRibbon.setEnabled(false)
      return
    }
    const run = this.points.slice(0, target + 1)
    this.apply(this.travelled, run, undefined)
    this.applyRibbon(this.travelledRibbon, run, undefined)
    this.travelled.setEnabled(true)
    this.travelledRibbon.setEnabled(true)
  }

  /** Hides the line and the marker without dropping the geometry. */
  setVisible(visible: boolean): void {
    const drawn = visible && this.points.length > 1
    this.path.setEnabled(drawn)
    this.pathRibbon.setEnabled(drawn)
    this.travelled.setEnabled(visible && this.drawnTo >= 1)
    this.travelledRibbon.setEnabled(visible && this.drawnTo >= 1)
    this.marker.setEnabled(visible && this.points.length > 0)
  }

  /** Disposes the line and the marker. */
  dispose(): void {
    for (const mesh of [this.path, this.pathRibbon, this.travelled, this.travelledRibbon]) {
      mesh.material?.dispose()
      mesh.dispose()
    }
    this.marker.material?.dispose()
    this.marker.dispose()
    this.points = []
  }

  /** Moves the marker to the sample nearest `time_s` and returns the point it landed on. */
  setTime(time_s: number): TrajectoryPoint | null {
    const index = pointAtTime(this.points, time_s)
    const point = this.points[index]
    if (point === undefined) {
      this.marker.setEnabled(false)
      return null
    }
    this.marker.position = new Vector3(point.x, point.z, point.y)
    this.marker.setEnabled(true)
    return point
  }

  /**
   * Places the marker at a pose.
   *
   * This is the seam the trajectory workstream extends: an attitude indicator only
   * has to pass the angles it already computes. The marker is a ground arrow — it
   * lies flat on the surface like route guidance does, so only the heading turns
   * it; pitch and roll belong to a body-shaped model and would tip the arrow into
   * the terrain.
   */
  setPose(point: TrajectoryPoint, pose: { heading_rad: number }): void {
    // The arrow rides the height the line is drawn at rather than the raw sample, for the
    // same reason the line does: over a flattened footprint the drawn surface is the roof,
    // and an arrow placed at the sample's own altitude disappears underneath it.
    this.marker.position = new Vector3(
      point.x,
      drawHeight(point, this.options) + MARKER_LIFT_M,
      point.y,
    )
    // The arrow is modelled pointing along +Z, and a heading is measured anticlockwise from
    // the map's +x axis. Babylon's yaw maps a local +Z to `(sin y, 0, cos y)`, so pointing
    // that at `(cos h, sin h)` needs `pi/2 - h`.
    this.marker.rotation = new Vector3(0, Math.PI / 2 - pose.heading_rad, 0)
    this.marker.setEnabled(true)
  }

  /** Uploads one stretch of points into a line mesh, in the line's own style. */
  private apply(
    mesh: Mesh,
    points: readonly TrajectoryPoint[],
    colorOf?: (point: TrajectoryPoint) => readonly [number, number, number],
  ): void {
    const geometry = trajectoryGeometry(points, this.options, colorOf)
    const vertexData = new VertexData()
    vertexData.positions = geometry.positions
    vertexData.indices = geometry.indices
    vertexData.colors = geometry.colors
    vertexData.applyToMesh(mesh, true)
  }

  /** Uploads one stretch of points into the ribbon that underlays a line mesh. */
  private applyRibbon(
    mesh: Mesh,
    points: readonly TrajectoryPoint[],
    colorOf?: (point: TrajectoryPoint) => readonly [number, number, number],
  ): void {
    const geometry = trajectoryRibbon(points, this.options, RIBBON_WIDTH_M, colorOf)
    const vertexData = new VertexData()
    vertexData.positions = geometry.positions
    vertexData.indices = geometry.indices
    vertexData.colors = geometry.colors
    if (geometry.normals !== undefined) {
      vertexData.normals = geometry.normals
    }
    vertexData.applyToMesh(mesh, true)
  }

  /** Fixed colour scale of the current points, shared by every channel. */
  private valueRange(points: readonly TrajectoryPoint[]): [number, number] {
    let min = Number.POSITIVE_INFINITY
    let max = Number.NEGATIVE_INFINITY
    for (const point of points) {
      if (!Number.isFinite(point.value)) {
        continue
      }
      min = Math.min(min, point.value)
      max = Math.max(max, point.value)
    }
    if (min === Number.POSITIVE_INFINITY) {
      return [0, 0]
    }
    return [min, max]
  }
}

/** One line-list mesh with a self-lit material; `stacking` lifts bands a fraction apart. */
function lineMesh(name: string, scene: Scene, stacking: number): Mesh {
  const mesh = new Mesh(name, scene)
  const material = new StandardMaterial(`${name}Material`, scene)
  // Emissive carries the colour and the diffuse term only answers the lights; the material
  // stays *lit* rather than unlit on purpose. `disableLighting` is the obvious way to draw
  // a constant colour and it is the one setting whose effect never compiled on the WebGPU
  // backend this app falls back to — a track that renders nowhere.
  material.emissiveColor = new Color3(1, 1, 1)
  material.diffuseColor = new Color3(0.35, 0.35, 0.35)
  material.specularColor = new Color3(0, 0, 0)
  // A line list has no faces, so the material must not try to shade them.
  material.fillMode = Material.LineListDrawMode
  mesh.material = material
  mesh.isPickable = false
  mesh.renderingGroupId = 2
  // Two coincident line lists would flicker against each other for depth; the travelled
  // band sits this much higher, which at map scale is invisible and in depth is decisive.
  mesh.position.y = stacking * 0.3
  mesh.setEnabled(false)
  return mesh
}

/**
 * One translucent ribbon mesh under a line: the route as a band a reader can find at any
 * zoom, in the line's own colours. Alpha-blended, so it is drawn after the opaque pass and
 * can never hide the hairline that rides on top of it; its own depth test still loses to
 * the terrain it drapes on.
 */
function ribbonMesh(name: string, scene: Scene, stacking: number, alpha: number): Mesh {
  const mesh = new Mesh(name, scene)
  const material = new StandardMaterial(`${name}Material`, scene)
  material.emissiveColor = new Color3(1, 1, 1)
  material.diffuseColor = new Color3(0.35, 0.35, 0.35)
  material.specularColor = new Color3(0, 0, 0)
  material.alpha = alpha
  mesh.material = material
  mesh.isPickable = false
  mesh.renderingGroupId = 2
  // The band lies below the hairline so the hairline's depth wins where they overlap.
  mesh.position.y = -0.1 - stacking * 0.05
  mesh.setEnabled(false)
  return mesh
}

/**
 * The runner's marker: a flat navigation arrow lying on the ground.
 *
 * An arrow rather than a dot, because a trajectory is a *direction* and the one thing a
 * reader wants from the live position is which way it is going. The head is a three-sided
 * pyramid laid flat — from above, the chevron route guidance draws at a playhead — with a
 * thin tail behind it, and the whole arrow is one merged mesh that turns about its heading
 * as one piece. Emissive and unlit: it must read against a white model, day or night.
 */
function buildArrow(scene: Scene): Mesh {
  const head = CreateCylinder(
    'trajectoryMarkerHead',
    {
      height: MARKER_LENGTH_M,
      diameterTop: 0,
      diameterBottom: MARKER_HEAD_M,
      tessellation: 3,
    },
    scene,
  )
  const tail = CreateBox(
    'trajectoryMarkerTail',
    { width: MARKER_HEAD_M * 0.22, height: MARKER_HEAD_M * 0.1, depth: MARKER_TAIL_M },
    scene,
  )
  // Babylon's primitives stand along +Y; the arrow points along +Z so a yaw aims it.
  head.rotation.x = Math.PI / 2
  head.position.z = MARKER_LENGTH_M / 2
  tail.position.z = -MARKER_TAIL_M / 2
  const merged = Mesh.MergeMeshes([head, tail], true, true, undefined, false, false)
  const marker = merged ?? head
  marker.name = 'trajectoryMarker'
  // The offset is in the parts' own local space, which the merge folds into the vertices.
  marker.bakeCurrentTransformIntoVertices()
  // The arrow is drawn flat, the way route guidance reads: squashed onto the ground plane
  // rather than standing out of it.
  marker.scaling.y = 0.18
  const material = new StandardMaterial('trajectoryMarkerMaterial', scene)
  material.backFaceCulling = false
  // Bright against a white model, and the one colour in the scene nothing else uses. Lit
  // rather than unlit for the same reason the line is — see {@link lineMesh}.
  material.emissiveColor = new Color3(0.95, 0.35, 0.1)
  material.diffuseColor = new Color3(0.45, 0.16, 0.05)
  material.specularColor = new Color3(0, 0, 0)
  marker.material = material
  marker.isPickable = false
  marker.setEnabled(false)
  return marker
}
