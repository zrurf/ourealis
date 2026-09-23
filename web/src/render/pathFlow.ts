/*
 * Water flowing in a pipe.
 *
 * A route is a *direction*, and the notation for direction is something that moves. The first
 * attempt drew a single white marker, which read as a slider on a rail: a mechanical object
 * travelling a line. Water in a pipe reads the way a route should — the whole line is filled, and
 * the movement comes from a train of *slugs* sliding through it, each of them a slightly different
 * length so the eye reads fluid rather than a conveyor.
 *
 * Why this is geometry rather than a material effect: a dash pattern belongs to the shader
 * (`dashOffset`), and on a multi-segment path it drew its dashes over part of the path and stopped —
 * which read as "the route only goes to the first waypoint". Geometry placed from the path's own arc
 * length covers the whole line, and its speed is a number in metres per second that can be stated.
 *
 * The slugs are rebuilt each frame as short ribbons along the path, because they have to follow the
 * route's *shape*: a straight stub placed at a tangent lifts off the ground on a bend, and a slug
 * that leaves the pipe it belongs to is worse than no slug at all.
 */
import { Color3 } from '@babylonjs/core/Maths/math.color'
import { CreateGreasedLine } from '@babylonjs/core/Meshes/Builders/greasedLineBuilder'
import { GreasedLineMeshColorMode } from '@babylonjs/core/Materials/GreasedLine/greasedLineMaterialInterfaces'
import type { AbstractMesh } from '@babylonjs/core/Meshes/abstractMesh'
import { arcTable, pointAtArc, type ArcTable } from './flow'
import type { WorldPoint } from './overlayGeometry'
import type { MapScene } from './scene'

/** Points a slug's ribbon is built from; enough to follow a curve, few enough to rebuild per frame. */
const SLUG_POINTS = 6

/** How much a slug's length breathes over time, as a fraction. */
const BREATH = 0.22

/** Style of the water running through a route. */
export interface PathFlowStyle {
  /** How many slugs are in the pipe at once. */
  slugs: number
  /** Nominal length of one slug, metres. */
  lengthM: number
  /** How fast the water moves, metres per second. */
  speed: number
  /** Colour of the water, `#rrggbb`. */
  colour: string
  /**
   * Width of a slug, metres.
   *
   * Narrower than the water it runs inside: a slug as wide as its pipe is the pipe, and the water
   * stops reading as something moving *through* something else.
   */
  widthM: number
  /** Height above the path it runs on, metres. */
  liftM: number
}

/**
 * The water in one route.
 *
 * The path's points are in world metres and already carry the ground's height, so a slug's own
 * height is interpolated from them: the water stays inside the pipe whatever the terrain does
 * underneath it.
 */
export class PathFlow {
  private readonly mapScene: MapScene
  private readonly table: ArcTable
  private readonly heights: readonly number[]
  private readonly style: PathFlowStyle
  private mesh: AbstractMesh | null = null
  private arc = 0
  private time = 0

  constructor(mapScene: MapScene, points: readonly WorldPoint[], style: PathFlowStyle) {
    this.mapScene = mapScene
    this.style = style
    this.table = arcTable(points.map((point) => ({ x: point[0], y: point[2] })))
    this.heights = points.map((point) => point[1])
    this.write()
  }

  /** Moves the water along the pipe by `dt` seconds. */
  advance(dt: number): void {
    this.arc += this.style.speed * dt
    this.time += dt
    this.write()
  }

  /** Removes the water. */
  dispose(): void {
    if (this.mesh !== null) {
      this.mapScene.untrackHeightMesh(this.mesh)
      this.mesh.dispose()
      this.mesh = null
    }
  }

  /**
   * Rebuilds the train of slugs.
   *
   * One mesh holding every slug: a family of short ribbons is one draw call, and the length of each
   * slug is geometry rather than a material setting, so a single mesh can hold slugs of different
   * lengths — which is what keeps the flow from looking like a conveyor belt.
   */
  private write(): void {
    const paths: number[][] = []
    const spacing = this.style.lengthM * 2
    for (let index = 0; index < Math.max(1, this.style.slugs); index += 1) {
      // Evenly spaced through the pipe, each breathing at its own phase.
      const breath = 1 + Math.sin(this.time * 1.6 + index * 1.7) * BREATH
      const head = this.arc + index * spacing
      const length = this.style.lengthM * breath
      const path: number[] = []
      for (let step = 0; step <= SLUG_POINTS; step += 1) {
        const sample = pointAtArc(this.table, head - (length * step) / SLUG_POINTS)
        if (sample === null) {
          continue
        }
        const from = this.heights[sample.index] ?? 0
        const to = this.heights[sample.index + 1] ?? from
        path.push(sample.at.x, from + (to - from) * sample.t + this.style.liftM, sample.at.y)
      }
      if (path.length >= 6) {
        paths.push(path)
      }
    }
    if (paths.length === 0) {
      return
    }
    this.mesh?.dispose()
    this.mesh = CreateGreasedLine(
      'route-water',
      { points: paths },
      {
        width: this.style.widthM,
        color: Color3.FromHexString(this.style.colour),
        sizeAttenuation: false,
        colorMode: GreasedLineMeshColorMode.COLOR_MODE_SET,
      },
      this.mapScene.scene,
    )
    this.mesh.isPickable = false
    this.mesh.renderingGroupId = 3
    if (this.mesh.material !== null) {
      this.mesh.material.alpha = 0.95
      this.mesh.material.zOffset = -9
    }
    this.mapScene.trackHeightMesh(this.mesh)
  }
}
