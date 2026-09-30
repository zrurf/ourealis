/*
 * Chevrons travelling along a route.
 *
 * A route is a *direction*, and the notation for direction is something that moves. The
 * route itself is the band drawn by `lines.ts`; this file adds the movement — a train of
 * hollow arrows sliding along the band, every one of them pointing the way the run goes.
 *
 * The arrows are placed from the path's own arc length rather than from a dash pattern.
 * A dash pattern belongs to the shader (`dashOffset`) and stopped partway along a
 * multi-segment path, which read as "the route only reaches the first waypoint".
 *
 * They are geometry rather than sprites: a billboard that faces the camera cannot say
 * which way a route runs, and a chevron drawn flat at a tangent lifts off the ground on a
 * bend, so its own points are interpolated from the route's.
 */
import { Color3 } from '@babylonjs/core/Maths/math.color'
import { CreateGreasedLine } from '@babylonjs/core/Meshes/Builders/greasedLineBuilder'
import { GreasedLineMeshColorMode } from '@babylonjs/core/Materials/GreasedLine/greasedLineMaterialInterfaces'
import type { GreasedLineBaseMesh } from '@babylonjs/core/Meshes/GreasedLine/greasedLineBaseMesh'
import { arcTable, pointAtArc, type ArcTable } from './flow'
import type { WorldPoint } from './overlayGeometry'
import type { MapScene } from './scene'

/**
 * Half-angle of a chevron's wing, radians.
 *
 * The angle a wing makes with the direction of travel: a smaller value is a longer,
 * sharper arrow, a larger one a blunter V.
 */
const WING_ANGLE = 0.62

/** Style of the arrows running along a route. */
export interface PathFlowStyle {
  /** Distance from one chevron to the next, metres. */
  spacingM: number
  /** Length of a chevron's wing, metres. */
  sizeM: number
  /** Width of the stroke a chevron is drawn with, metres. */
  widthM: number
  /** How fast the arrows travel, metres per second. */
  speed: number
  /** Arrow colour, `#rrggbb`. */
  colour: string
  /** Height above the route they run on, metres. */
  liftM: number
}

/**
 * The arrows running along one route.
 *
 * The route's points are world metres that already carry the ground's height, so an
 * arrow's own height is interpolated from them: the train stays on the route whatever the
 * terrain does underneath it.
 */
export class PathFlow {
  private readonly mapScene: MapScene
  private readonly table: ArcTable
  private readonly heights: readonly number[]
  private readonly style: PathFlowStyle
  private mesh: GreasedLineBaseMesh | null = null
  private arc = 0

  constructor(mapScene: MapScene, points: readonly WorldPoint[], style: PathFlowStyle) {
    this.mapScene = mapScene
    this.style = style
    this.table = arcTable(points.map((point) => ({ x: point[0], y: point[2] })))
    this.heights = points.map((point) => point[1])
    this.write()
  }

  /** Moves the arrows along the route by `dt` seconds. */
  advance(dt: number): void {
    this.arc += this.style.speed * dt
    this.write()
  }

  /** Removes the arrows. */
  dispose(): void {
    if (this.mesh !== null) {
      this.mapScene.untrackHeightMesh(this.mesh)
      // The material is the mesh's own, and Babylon leaves it on the scene unless the
      // caller asks for it: without the second argument every frame of a running route
      // leaks one.
      this.mesh.dispose(false, true)
      this.mesh = null
    }
  }

  /**
   * Moves the train of chevrons to the current phase.
   *
   * One mesh holds every chevron, so the whole train is one draw call. The mesh is built
   * once and its points are replaced in place: disposing and rebuilding it every frame
   * cost a fresh `GreasedLineMaterial` per frame, which the engine never released, and
   * threw away the scene's active-mesh and rendering-group caches on the way — which is
   * what made the arrows blink against the band they run on.
   *
   * The chevron count is fixed and the positions wrap, so the vertex buffer keeps its size
   * from frame to frame and the train re-enters at the start as it leaves the end rather
   * than thinning out to nothing.
   */
  private write(): void {
    const spacing = Math.max(1, this.style.spacingM)
    const length = this.table.lengthM
    if (length <= 0) {
      return
    }
    const phase = ((this.arc % length) + length) % length
    const count = Math.max(1, Math.ceil(length / spacing))
    const paths: number[][] = []
    for (let index = 0; index < count; index += 1) {
      const sample = pointAtArc(this.table, phase + index * spacing)
      if (sample === null) {
        continue
      }
      paths.push(this.chevron(sample.at.x, sample.at.y, sample.headingRad, sample.index, sample.t))
    }
    if (paths.length === 0) {
      return
    }
    if (this.mesh !== null) {
      this.mesh.setPoints(paths)
      return
    }
    const mesh = CreateGreasedLine(
      'route-arrows',
      { points: paths, updatable: true },
      {
        width: this.style.widthM,
        color: Color3.FromHexString(this.style.colour),
        sizeAttenuation: false,
        colorMode: GreasedLineMeshColorMode.COLOR_MODE_SET,
      },
      this.mapScene.scene,
    )
    mesh.isPickable = false
    mesh.renderingGroupId = 3
    // Above every band, whose own indices come from their depth offsets and stay below
    // this. Without it the default sort put the band over its own chevrons.
    mesh.alphaIndex = 10
    if (mesh.material !== null) {
      mesh.material.alpha = 0.95
      mesh.material.zOffset = -10
    }
    this.mesh = mesh
    this.mapScene.trackHeightMesh(mesh)
  }

  /** One chevron: two wings swept back from an apex that leads along the route. */
  private chevron(x: number, y: number, heading: number, index: number, t: number): number[] {
    const from = this.heights[index] ?? 0
    const to = this.heights[index + 1] ?? from
    const height = from + (to - from) * t + this.style.liftM
    const forwardX = Math.cos(heading)
    const forwardY = Math.sin(heading)
    const backX = x - forwardX * this.style.sizeM * Math.cos(WING_ANGLE)
    const backY = y - forwardY * this.style.sizeM * Math.cos(WING_ANGLE)
    const sideX = -forwardY * this.style.sizeM * Math.sin(WING_ANGLE)
    const sideY = forwardX * this.style.sizeM * Math.sin(WING_ANGLE)
    return [
      backX + sideX,
      height,
      backY + sideY,
      x,
      height,
      y,
      backX - sideX,
      height,
      backY - sideY,
    ]
  }
}
