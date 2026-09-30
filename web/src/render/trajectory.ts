/*
 * Trajectory: a speed-coloured polyline and a runner marker.
 *
 * The trajectory workstream takes over what this file starts, so the seam is
 * documented rather than implied: {@link TrajectoryPoint} is the only input the
 * line accepts, `sampleTrajectory` is what turns the service's truth samples into
 * it, and {@link TrajectoryLine.setPose} is the one call the playback clock needs.
 * A view that already holds truth samples therefore draws a trajectory without
 * knowing about Babylon, and a later workstream can add channels, markers or an
 * attitude indicator behind the same two calls.
 *
 * Height follows the same rule as the terrain: one world unit is one metre, the
 * map plane is `(x, z)` and `y` is up, and the scene's exaggeration applies to the
 * line together with the surface it sits on.
 */
import { StandardMaterial } from '@babylonjs/core/Materials/standardMaterial'
import { Color3 } from '@babylonjs/core/Maths/math.color'
import { Vector3 } from '@babylonjs/core/Maths/math.vector'
import { CreateCylinder } from '@babylonjs/core/Meshes/Builders/cylinderBuilder'
import { CreateGreasedLine } from '@babylonjs/core/Meshes/Builders/greasedLineBuilder'
import { GreasedLineMeshColorMode } from '@babylonjs/core/Materials/GreasedLine/greasedLineMaterialInterfaces'
import { Mesh } from '@babylonjs/core/Meshes/mesh'
import type { Scene } from '@babylonjs/core/scene'
import { speedColor } from '@/types/colormap'
import type { GreasedLineBaseMesh } from '@babylonjs/core/Meshes/GreasedLine/greasedLineBaseMesh'
import type { SurfaceHeight } from './drape'
import type { TruthSample } from '@/types/result'

/** Channels a trajectory line can be coloured by. */
export type TrajectoryChannel = 'speed' | 'grade' | 'curvature' | 'height'

/** One point of a trajectory, the only input the line draws. */
export interface TrajectoryPoint {
  /** Time since the start of the recording, seconds. */
  time_s: number
  /** Easting and northing in the map plane, metres. */
  x: number
  /** Northing in the map plane, metres. */
  y: number
  /** Absolute altitude including bounce, metres. */
  z: number
  /** Value the point is coloured by, in the channel's own unit. */
  value: number
}

/** The value a channel reads out of a truth sample. */
export function channelValue(sample: TruthSample, channel: TrajectoryChannel): number {
  switch (channel) {
    case 'speed':
      return sample.speed
    case 'grade':
      return sample.grade
    case 'curvature':
      return sample.kappa_eff
    case 'height':
      return sample.z
  }
}

/** Turns truth samples into trajectory points for one channel. */
export function sampleTrajectory(
  samples: readonly TruthSample[],
  channel: TrajectoryChannel = 'speed',
): TrajectoryPoint[] {
  return samples.map((sample) => ({
    time_s: sample.time_s,
    x: sample.position.x,
    y: sample.position.y,
    z: sample.z,
    value: channelValue(sample, channel),
  }))
}

/**
 * Builds the polyline geometry of a trajectory.
 *
 * Each segment carries its own two vertices with their own colours, because a line
 * list has no way to interpolate a colour along a shared vertex; the extra
 * vertices are what buys per-segment colouring. Every channel uses the same fixed
 * scale, so switching from speed to grade compares shapes rather than palettes.
 */
/** How a trajectory is held off the ground when it is drawn. */
export interface TrajectoryGeometryOptions {
  /** Height nothing is drawn below, metres. */
  minZ?: number
  /**
   * Height of the surface the trajectory is drawn over.
   *
   * A run's samples carry the runner's own altitude, which is the truth at the map's full
   * resolution, while the surface on screen is a coarser level interpolated between cell
   * centres. Wherever that surface rides above a sample the line is depth-tested away and
   * the track vanishes in patches — a run that looks like it never happened. Taking the
   * higher of the two keeps the bounce the samples carry and the track visible on the
   * ground the reader is actually looking at.
   */
  surface?: SurfaceHeight
  /** How far above that surface the line is held, metres. */
  lift?: number
}

/** The height a trajectory point is drawn at, never below the surface it crosses. */
function drawHeight(point: TrajectoryPoint, options: TrajectoryGeometryOptions): number {
  const floor = options.minZ ?? 0
  const surface = options.surface
  if (surface === undefined) {
    return Math.max(floor, point.z)
  }
  const ground = surface(point.x, point.y)
  if (ground === null) {
    return Math.max(floor, point.z)
  }
  return Math.max(floor, point.z, ground + (options.lift ?? 0))
}

/** Index of the point nearest a time, assuming ascending sample times. */
export function pointAtTime(points: readonly TrajectoryPoint[], time_s: number): number {
  if (points.length === 0) {
    return -1
  }
  let best = 0
  let bestDistance = Number.POSITIVE_INFINITY
  for (let index = 0; index < points.length; index += 1) {
    const point = points[index]
    if (point === undefined) {
      continue
    }
    const distance = Math.abs(point.time_s - time_s)
    if (distance < bestDistance) {
      bestDistance = distance
      best = index
    }
  }
  return best
}

/**
 * Width of the whole trajectory, in screen pixels.
 *
 * Screen rather than world metres, and deliberately: a track read from a whole-map view and
 * the same track read from a hundred metres up are the same line to the reader, and a line
 * whose width is fixed in metres disappears at one and swamps the map at the other.
 */
const PATH_WIDTH_PX = 2

/**
 * Width of the part already run, in screen pixels.
 *
 * Half again as wide as the rest, because the travelled half is the answer to "where did it
 * go" and the remaining half is only the question. It is drawn over the full path rather
 * than replacing it, so the two reads as one line that thickens behind the runner.
 */
const TRAVELLED_WIDTH_PX = 5

/** Colour of the part already run, `#rrggbb`. */
const TRAVELLED_COLOUR = '#f2b134'

/**
 * Length of the runner's marker, metres.
 *
 * Large, because the marker's job is to be findable: at a whole-campus view a marker sized
 * to the runner is a pixel, and a reader looking for "where is it now" cannot find one.
 * It is an annotation on the map rather than a scale model of a person.
 */
const MARKER_LENGTH_M = 14

/** Width of the marker's shaft, metres. */
const MARKER_SHAFT_M = 2.4

/** Width of the marker's head, metres. */
const MARKER_HEAD_M = 6

/** Fraction of the marker's length taken by its head. */
const MARKER_HEAD_FRACTION = 0.45

/** A trajectory drawn over the terrain, with a marker at a chosen time. */
export class TrajectoryLine {
  private readonly path: GreasedLineBaseMesh
  private readonly travelled: GreasedLineBaseMesh
  private readonly marker: Mesh
  private points: TrajectoryPoint[] = []
  /** How the line is held off the ground; kept so a later band is drawn the same way. */
  private options: TrajectoryGeometryOptions = {}
  /** Index the travelled band currently reaches, so a still playhead rebuilds nothing. */
  private drawnTo = -1

  constructor(scene: Scene) {
    this.path = CreateGreasedLine(
      'trajectoryPath',
      { points: [] },
      {
        width: PATH_WIDTH_PX,
        sizeAttenuation: false,
        colorMode: GreasedLineMeshColorMode.COLOR_MODE_SET,
        useColors: true,
      },
      scene,
    )
    this.path.isPickable = false
    this.path.renderingGroupId = 2

    this.travelled = CreateGreasedLine(
      'trajectoryTravelled',
      { points: [] },
      {
        width: TRAVELLED_WIDTH_PX,
        sizeAttenuation: false,
        color: Color3.FromHexString(TRAVELLED_COLOUR),
        colorMode: GreasedLineMeshColorMode.COLOR_MODE_SET,
      },
      scene,
    )
    this.travelled.isPickable = false
    // The same group as the path and a higher index, so the travelled band lands on top of
    // the line it is part of rather than being sorted against it by distance.
    this.travelled.renderingGroupId = 2
    this.travelled.alphaIndex = 1

    this.marker = buildArrow(scene)
  }

  /** Replaces the drawn trajectory. */
  setTrajectory(points: readonly TrajectoryPoint[], options: TrajectoryGeometryOptions = {}): void {
    this.points = [...points]
    this.options = options
    this.drawnTo = -1
    const line = this.drawnLine(points)
    this.path.setPoints(line.length > 1 ? [line] : [])
    const plugin = greasedLinePlugin(this.path)
    if (plugin !== null && points.length > 0) {
      plugin.setColors(pointColours(points), false)
    }
    this.path.setEnabled(points.length > 1)
    this.travelled.setEnabled(false)
  }

  /**
   * One flat `x, y, z` run for a stretch of the trajectory, at the height it is drawn at.
   *
   * The height is the sample's own unless the ground has risen above it, which is the same
   * rule the terrain is drawn by — a line placed at the raw sample altitude sinks under a
   * surface interpolated between cell centres, which is what made a run disappear in
   * patches on a hilly map.
   */
  private drawnLine(points: readonly TrajectoryPoint[]): number[] {
    const out: number[] = []
    for (const point of points) {
      out.push(point.x, drawHeight(point, this.options), point.y)
    }
    return out
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
      return
    }
    this.travelled.setPoints([this.drawnLine(this.points.slice(0, target + 1))])
    this.travelled.setEnabled(true)
  }

  /** Hides the line and the marker without dropping the geometry. */
  setVisible(visible: boolean): void {
    this.path.setEnabled(visible && this.points.length > 1)
    this.travelled.setEnabled(visible && this.drawnTo >= 1)
    this.marker.setEnabled(visible && this.points.length > 0)
  }

  /** Disposes the line and the marker. */
  dispose(): void {
    // The second argument releases each mesh's own GreasedLine material, which Babylon
    // otherwise leaves on the scene for the life of the engine.
    this.path.dispose(false, true)
    this.travelled.dispose(false, true)
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
   * has to pass the angles it already computes, and the matrix keeps the marker's
   * forward axis along the runner's heading with roll about it.
   */
  setPose(
    point: TrajectoryPoint,
    pose: { heading_rad: number; pitch_rad?: number; roll_rad?: number },
  ): void {
    const pitch = pose.pitch_rad ?? 0
    const roll = pose.roll_rad ?? 0
    this.marker.position = new Vector3(point.x, point.z, point.y)
    // The arrow is modelled along +Z, and a heading is measured anticlockwise from the map's
    // +x axis. Babylon's yaw maps a local +Z to `(sin y, 0, cos y)`, so pointing that at
    // `(cos h, sin h)` needs `pi/2 - h`. Using `h` directly — which is what a sphere, with no
    // direction to show, could get away with — points the arrow mirrored about the diagonal.
    this.marker.rotation = new Vector3(pitch, Math.PI / 2 - pose.heading_rad, roll)
    this.marker.setEnabled(true)
  }
}

/**
 * The GreasedLine plugin a mesh created by the builder carries.
 *
 * The colours and the widths of a GreasedLine live on its material's plugin rather than on
 * the mesh, and the builder is the only place that knows how the two are wired together.
 */
function greasedLinePlugin(
  mesh: GreasedLineBaseMesh,
): { setColors: (colours: Color3[], lazy: boolean) => void } | null {
  const material = mesh.material
  if (material === null) {
    return null
  }
  const plugin = (
    material as unknown as { pluginManager?: { getPlugin: (name: string) => unknown } }
  ).pluginManager?.getPlugin('GreasedLine')
  if (plugin === undefined || plugin === null) {
    return null
  }
  return plugin as { setColors: (colours: Color3[], lazy: boolean) => void }
}

/** One colour per drawn point, from the channel the trajectory is coloured by. */
function pointColours(points: readonly TrajectoryPoint[]): Color3[] {
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
    min = 0
    max = 0
  }
  return points.map((point) => {
    const [r, g, b] = speedColor(point.value, min, max)
    return new Color3(r / 255, g / 255, b / 255)
  })
}

/**
 * The runner's marker: a navigation arrow lying on the ground.
 *
 * An arrow rather than a dot, because a trajectory is a *direction* and the one thing a
 * reader wants from the live position is which way it is going. Assembled from a shaft and
 * a cone and baked into one mesh, so the whole marker is one draw call and rotates as one
 * piece; the long axis is local +Z, which is the axis {@link TrajectoryLine.setPose} turns.
 */
function buildArrow(scene: Scene): Mesh {
  const headLength = MARKER_LENGTH_M * MARKER_HEAD_FRACTION
  const shaft = CreateCylinder(
    'trajectoryMarkerShaft',
    { height: MARKER_LENGTH_M - headLength, diameter: MARKER_SHAFT_M, tessellation: 12 },
    scene,
  )
  const head = CreateCylinder(
    'trajectoryMarkerHead',
    {
      height: headLength,
      diameterTop: 0,
      diameterBottom: MARKER_HEAD_M,
      tessellation: 12,
    },
    scene,
  )
  // Babylon's cylinders stand along +Y; the marker has to lie along +Z so a yaw aims it.
  for (const part of [shaft, head]) {
    part.rotation.x = Math.PI / 2
  }
  shaft.position.z = (MARKER_LENGTH_M - headLength) / 2
  head.position.z = MARKER_LENGTH_M - headLength / 2
  const arrow = Mesh.MergeMeshes([shaft, head], true, true, undefined, false, false)
  const marker = arrow ?? shaft
  // The offset is in the parts' own local space, which the merge folds into the vertices.
  marker.bakeCurrentTransformIntoVertices()
  const material = new StandardMaterial('trajectoryMarkerMaterial', scene)
  material.disableLighting = true
  // Bright against a white model, and the one colour in the scene nothing else uses.
  material.emissiveColor = new Color3(0.95, 0.35, 0.1)
  material.specularColor = new Color3(0, 0, 0)
  marker.material = material
  marker.isPickable = false
  marker.setEnabled(false)
  return marker
}
