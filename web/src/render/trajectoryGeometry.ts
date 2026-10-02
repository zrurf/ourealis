/*
 * Trajectory geometry: points, heights and the line-list buffers.
 *
 * Pure: a `TrajectoryPoint` in, typed arrays and heights out, nothing from Babylon, so
 * the drawing rules can be tested without an engine. `render/trajectory.ts` is the thin
 * Babylon side that owns the meshes and the marker.
 *
 * Each segment carries its own two vertices with their own colours, because a line list
 * has no way to interpolate a colour along a shared vertex; the extra vertices are what
 * buys per-segment colouring. Every channel uses the same fixed scale, so switching from
 * speed to grade compares shapes rather than palettes.
 *
 * Height follows the same rule as the terrain: one world unit is one metre, the map plane
 * is `(x, z)` and `y` is up, and a point is never drawn below the surface it crosses.
 */
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

/** How a trajectory is held off the ground when it is drawn. */
export interface TrajectoryGeometryOptions {
  /** Height nothing is drawn below, metres. */
  minZ?: number
  /**
   * Height of the surface the trajectory is drawn over.
   *
   * A run's samples carry the runner's own altitude, which is the truth at the map's full
   * resolution, while the surface on screen is a coarser level interpolated between cell
   * centres — and a *building footprint* on that surface is flattened to its roof. Wherever
   * the drawn surface rides above a sample, the line is depth-tested away and the track
   * vanishes in patches — a run that looks like it never happened. Taking the higher of the
   * two keeps the bounce the samples carry and the track visible on the ground the reader
   * is actually looking at.
   */
  surface?: SurfaceHeight
  /** How far above that surface the line is held, metres. */
  lift?: number
}

/** The height a trajectory point is drawn at, never below the surface it crosses. */
export function drawHeight(point: TrajectoryPoint, options: TrajectoryGeometryOptions): number {
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

/** Shared colour of the travelled band, as 0–1 channels: the one amber in the scene. */
const DEFAULT_TRAVELLED_COLOUR: readonly [number, number, number] = [0.95, 0.69, 0.2]

/**
 * Line-list geometry of a trajectory.
 *
 * Each segment carries its own two vertices with their own colours, because a line
 * list has no way to interpolate a colour along a shared vertex; the extra
 * vertices are what buys per-segment colouring. Every channel uses the same fixed
 * scale, so switching from speed to grade compares shapes rather than palettes.
 */
export interface TrajectoryGeometry {
  /** Positions, `x, y, z` per vertex; each segment contributes two vertices. */
  positions: Float32Array
  /** Line-list indices. */
  indices: Uint32Array
  /** Vertex colours, `r, g, b, a` per vertex. */
  colors: Float32Array
  /** Vertex normals, present only when the geometry is faces rather than a line list. */
  normals?: Float32Array
}

/** Builds the line-list geometry of a stretch of trajectory points. */
export function trajectoryGeometry(
  points: readonly TrajectoryPoint[],
  options: TrajectoryGeometryOptions = {},
  colorOf?: (point: TrajectoryPoint) => readonly [number, number, number],
): TrajectoryGeometry {
  const segmentCount = Math.max(0, points.length - 1)
  const positions = new Float32Array(segmentCount * 6)
  const colors = new Float32Array(segmentCount * 8)
  const indices = new Uint32Array(segmentCount * 2)
  for (let index = 0; index < segmentCount; index += 1) {
    const from = points[index]
    const to = points[index + 1]
    if (from === undefined || to === undefined) {
      continue
    }
    positions.set(
      [from.x, drawHeight(from, options), from.y, to.x, drawHeight(to, options), to.y],
      index * 6,
    )
    const fromColor = colorOf?.(from) ?? DEFAULT_TRAVELLED_COLOUR
    const toColor = colorOf?.(to) ?? DEFAULT_TRAVELLED_COLOUR
    colors.set(
      [
        fromColor[0],
        fromColor[1],
        fromColor[2],
        1,
        toColor[0],
        toColor[1],
        toColor[2],
        1,
      ],
      index * 8,
    )
    indices[index * 2] = index * 2
    indices[index * 2 + 1] = index * 2 + 1
  }
  return { positions, indices, colors }
}

/**
 * Ribbon geometry of a trajectory: one flat quad per segment, `widthM` metres across.
 *
 * The line list above is a hairline — a pixel, whatever the zoom — which at a whole-map
 * view is a track the reader has to hunt for. The ribbon is the same polyline as real
 * triangles, so it exists on every backend and reads as a route at any distance; the
 * hairline rides on top of it, and the two together are what the replay draws.
 */
export function trajectoryRibbon(
  points: readonly TrajectoryPoint[],
  options: TrajectoryGeometryOptions = {},
  widthM = 8,
  colorOf?: (point: TrajectoryPoint) => readonly [number, number, number],
): TrajectoryGeometry {
  const segmentCount = Math.max(0, points.length - 1)
  const positions = new Float32Array(segmentCount * 12)
  const colors = new Float32Array(segmentCount * 16)
  const indices = new Uint32Array(segmentCount * 6)
  // Flat on the ground, so every vertex faces straight up; the camera never reads a map
  // from below it.
  const normals = new Float32Array(segmentCount * 12)
  const half = widthM / 2
  for (let index = 0; index < segmentCount; index += 1) {
    const from = points[index]
    const to = points[index + 1]
    if (from === undefined || to === undefined) {
      continue
    }
    const dx = to.x - from.x
    const dz = to.y - from.y
    const length = Math.hypot(dx, dz)
    if (!(length > 0)) {
      continue
    }
    // The quad's cross axis is the segment direction turned a quarter in the map plane.
    const px = (-dz / length) * half
    const pz = (dx / length) * half
    const fromY = drawHeight(from, options)
    const toY = drawHeight(to, options)
    positions.set(
      [
        from.x + px, fromY, from.y + pz,
        from.x - px, fromY, from.y - pz,
        to.x - px, toY, to.y - pz,
        to.x + px, toY, to.y + pz,
      ],
      index * 12,
    )
    const fromColor = colorOf?.(from) ?? DEFAULT_TRAVELLED_COLOUR
    const toColor = colorOf?.(to) ?? DEFAULT_TRAVELLED_COLOUR
    const corners = [fromColor, fromColor, toColor, toColor]
    for (let corner = 0; corner < 4; corner += 1) {
      const colour = corners[corner] ?? DEFAULT_TRAVELLED_COLOUR
      colors.set([colour[0], colour[1], colour[2], 1], (index * 4 + corner) * 4)
    }
    for (let corner = 0; corner < 4; corner += 1) {
      normals[(index * 4 + corner) * 3 + 1] = 1
    }
    const base = index * 4
    indices.set([base, base + 1, base + 2, base, base + 2, base + 3], index * 6)
  }
  return { positions, indices, colors, normals }
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

