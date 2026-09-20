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
import { CreateSphere } from '@babylonjs/core/Meshes/Builders/sphereBuilder'
import { Mesh } from '@babylonjs/core/Meshes/mesh'
import { VertexData } from '@babylonjs/core/Meshes/mesh.vertexData'
import { Material } from '@babylonjs/core/Materials/material'
import type { Scene } from '@babylonjs/core/scene'
import { speedColor } from '@/types/colormap'
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

/** Geometry of a trajectory polyline: line-list positions with one colour per vertex. */
export interface TrajectoryGeometry {
  /** Positions, `x, y, z` per vertex; each segment contributes two vertices. */
  positions: Float32Array
  /** Line-list indices. */
  indices: Uint32Array
  /** Vertex colours, `r, g, b, a` per vertex. */
  colors: Float32Array
  /** Lowest and highest value of the coloured channel. */
  range: { min: number; max: number }
  /** Time span of the trajectory, seconds. */
  time: { start: number; end: number }
}

/**
 * Builds the polyline geometry of a trajectory.
 *
 * Each segment carries its own two vertices with their own colours, because a line
 * list has no way to interpolate a colour along a shared vertex; the extra
 * vertices are what buys per-segment colouring. Every channel uses the same fixed
 * scale, so switching from speed to grade compares shapes rather than palettes.
 */
export function trajectoryGeometry(
  points: readonly TrajectoryPoint[],
  options: { minZ?: number } = {},
): TrajectoryGeometry {
  const segmentCount = Math.max(0, points.length - 1)
  const positions = new Float32Array(segmentCount * 6)
  const colors = new Float32Array(segmentCount * 8)
  const indices = new Uint32Array(segmentCount * 2)
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
  const floor = options.minZ ?? 0
  for (let index = 0; index < segmentCount; index += 1) {
    const from = points[index]
    const to = points[index + 1]
    if (from === undefined || to === undefined) {
      continue
    }
    // A point below the terrain floor would be hidden by the surface; clamping keeps
    // the line visible where the samples dip under a coarse elevation chunk.
    positions.set(
      [from.x, Math.max(floor, from.z), from.y, to.x, Math.max(floor, to.z), to.y],
      index * 6,
    )
    const fromColor = speedColor(from.value, min, max)
    const toColor = speedColor(to.value, min, max)
    colors.set(
      [
        fromColor[0] / 255,
        fromColor[1] / 255,
        fromColor[2] / 255,
        1,
        toColor[0] / 255,
        toColor[1] / 255,
        toColor[2] / 255,
        1,
      ],
      index * 8,
    )
    indices[index * 2] = index * 2
    indices[index * 2 + 1] = index * 2 + 1
  }
  const first = points[0]
  const last = points[points.length - 1]
  return {
    positions,
    indices,
    colors,
    range: { min, max },
    time: { start: first?.time_s ?? 0, end: last?.time_s ?? 0 },
  }
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

/** A trajectory drawn over the terrain, with a marker at a chosen time. */
export class TrajectoryLine {
  private readonly mesh: Mesh
  private readonly marker: Mesh
  private readonly material: StandardMaterial
  private points: TrajectoryPoint[] = []

  constructor(scene: Scene) {
    this.material = new StandardMaterial('trajectoryMaterial', scene)
    this.material.disableLighting = true
    this.material.emissiveColor = new Color3(1, 1, 1)
    this.material.specularColor = new Color3(0, 0, 0)
    this.mesh = new Mesh('trajectory', scene)
    this.mesh.material = this.material
    this.mesh.isPickable = false
    // A line list has no faces, so the material must not try to shade them.
    this.material.fillMode = Material.LineListDrawMode
    this.marker = CreateSphere('trajectoryMarker', { diameter: 1.2 }, scene)
    const markerMaterial = new StandardMaterial('trajectoryMarkerMaterial', scene)
    markerMaterial.emissiveColor = new Color3(0.9, 0.9, 0.2)
    this.marker.material = markerMaterial
    this.marker.isPickable = false
    this.marker.setEnabled(false)
  }

  /** Replaces the drawn trajectory. */
  setTrajectory(points: readonly TrajectoryPoint[], options: { minZ?: number } = {}): void {
    this.points = [...points]
    const geometry = trajectoryGeometry(points, options)
    const vertexData = new VertexData()
    vertexData.positions = geometry.positions
    vertexData.indices = geometry.indices
    vertexData.colors = geometry.colors
    vertexData.applyToMesh(this.mesh, true)
    this.mesh.setEnabled(points.length > 1)
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
    this.marker.rotation = new Vector3(pitch, pose.heading_rad, roll)
    this.marker.setEnabled(true)
  }

  /** Hides the line and the marker without dropping the geometry. */
  setVisible(visible: boolean): void {
    this.mesh.setEnabled(visible && this.points.length > 1)
    this.marker.setEnabled(visible && this.points.length > 0)
  }

  /** Disposes the line and the marker. */
  dispose(): void {
    this.mesh.dispose()
    this.marker.dispose()
    this.material.dispose()
  }
}
