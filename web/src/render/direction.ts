/*
 * The direction constraint as arrows.
 *
 * The direction layer stores a heading and a strength per cell — the map's "you may only
 * walk this way" field — and a coloured grid cannot say a direction: a reader has to decode
 * a hue into an angle. Arrows are the notation for a vector field on a plan, so this module
 * turns the decoded samples into arrow paths on the ground: a shaft plus two barbs, drawn at
 * a spacing the caller picks so the field stays readable at a whole-map view.
 *
 * Pure geometry: plain `[x, y, z]` paths, so the arrows can be checked without an engine.
 */
import type { Vec2 } from '@/api/types'

/** A point in world metres. */
export type WorldPoint = [number, number, number]

/** Ground elevation under a map-plane position, or `null` where it is unknown. */
export type SurfaceHeight = (x: number, y: number) => number | null

/** One cell's decoded direction constraint. */
export interface DirectionSample {
  /** Position in the map plane, metres. */
  at: Vec2
  /** Heading the cell allows, degrees clockwise from east. */
  angleDeg: number
  /** How strong the constraint is, 0..255; a weak cell is drawn faintly. */
  strength: number
}

/** Options of {@link directionArrows}. */
export interface DirectionArrowOptions {
  /** Length of an arrow, metres. */
  lengthM?: number
  /** Height above the ground, metres. */
  liftM?: number
  /** Opening angle of the two barbs, degrees. */
  barbDeg?: number
  /** Length of a barb as a fraction of the shaft. */
  barbRatio?: number
}

/** The paths of one arrow: index 0 is the shaft, 1 and 2 are its barbs. */
export interface DirectionArrow {
  /** Arrow length in metres, so a caller can scale its width with it. */
  lengthM: number
  /** Heading in degrees, for a legend or a tooltip. */
  angleDeg: number
  /** Strength in `0..1`. */
  strength: number
  /** The three paths, in world metres. */
  paths: WorldPoint[][]
}

/**
 * Builds an arrow per sample.
 *
 * The arrow is drawn *from* its sample position along the heading, so the head marks where
 * the field leads; a zero-strength sample is skipped, because an arrow that means "no
 * constraint" is worse than no arrow.
 */
export function directionArrows(
  samples: readonly DirectionSample[],
  surface: SurfaceHeight,
  options: DirectionArrowOptions = {},
): DirectionArrow[] {
  const length = Math.max(0.5, options.lengthM ?? 12)
  const lift = options.liftM ?? 0.3
  const barb = ((options.barbDeg ?? 28) * Math.PI) / 180
  const barbLength = length * (options.barbRatio ?? 0.38)
  const arrows: DirectionArrow[] = []
  for (const sample of samples) {
    if (!(sample.strength > 0)) {
      continue
    }
    const heading = (sample.angleDeg * Math.PI) / 180
    const tail = sample.at
    const head: Vec2 = {
      x: tail.x + Math.cos(heading) * length,
      y: tail.y + Math.sin(heading) * length,
    }
    const left = barbEnd(head, heading + Math.PI - barb, barbLength)
    const right = barbEnd(head, heading + Math.PI + barb, barbLength)
    arrows.push({
      lengthM: length,
      angleDeg: sample.angleDeg,
      strength: Math.min(1, sample.strength / 255),
      paths: [
        [draped(tail, surface, lift, 0), draped(head, surface, lift, 0)],
        [draped(head, surface, lift, 0), draped(left, surface, lift, 0)],
        [draped(head, surface, lift, 0), draped(right, surface, lift, 0)],
      ],
    })
  }
  return arrows
}

/** The end of one barb, given the heading it points along. */
function barbEnd(head: Vec2, heading: number, length: number): Vec2 {
  return { x: head.x + Math.cos(heading) * length, y: head.y + Math.sin(heading) * length }
}

/** A point on the ground, `fallback` metres up where the ground is unknown. */
function draped(point: Vec2, surface: SurfaceHeight, lift: number, fallback: number): WorldPoint {
  return [point.x, (surface(point.x, point.y) ?? fallback) + lift, point.y]
}

/**
 * Samples the field on a coarse grid.
 *
 * A direction per cell is thousands of arrows on a campus map, which is a texture rather
 * than a reading; every `stepM` metres is what a printed plan does, and the caller picks the
 * step against the zoom.
 */
export function directionGrid(
  origin: { x: number; y: number },
  dimensions: { width: number; height: number },
  stepM: number,
  read: (x: number, y: number) => DirectionSample | null,
): DirectionSample[] {
  const step = Math.max(1, stepM)
  const out: DirectionSample[] = []
  for (let x = origin.x; x <= origin.x + dimensions.width; x += step) {
    for (let y = origin.y; y <= origin.y + dimensions.height; y += step) {
      const sample = read(x, y)
      if (sample !== null) {
        out.push({ ...sample, at: { x, y } })
      }
    }
  }
  return out
}
