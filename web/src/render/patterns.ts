/*
 * Procedural surface patterns.
 *
 * Colour on a draped layer carries the *meaning* of a cell; a pattern carries its
 * *material* — asphalt, grass, water, a façade — so a map reads as a place rather than as a
 * coloured grid. The patterns are generated here rather than shipped as images: a tiling
 * texture that is a pure function of world position needs no asset, no loading state and no
 * new dependency, and it can be tested as arithmetic.
 *
 * Every pattern samples the map plane in *metres*, so two cells of the same material abut
 * seamlessly and a pattern does not restart at a cell boundary. Each returns a brightness
 * factor around 1, which multiplies the cell's semantic colour: meaning stays in the
 * palette, texture only modulates it.
 */
import { hash2 } from './noise'

/** How a cell is textured. */
export type SurfacePattern =
  | 'flat'
  | 'asphalt'
  | 'grass'
  | 'water'
  | 'building'
  | 'gravel'
  | 'hatch'
  | 'contour'

/** How strongly a pattern is allowed to modulate its colour. */
export const PATTERN_STRENGTH = 0.14

/** World-space period of the fine grain, metres; smaller than one cell so it reads as detail. */
const GRAIN_M = 0.9

/** Returns the brightness factor of a pattern at a map position, around `1`. */
export function patternAt(
  pattern: SurfacePattern,
  x: number,
  y: number,
  /** Extra argument some patterns use: a normalised value, e.g. cost for contours. */
  value = 0,
): number {
  switch (pattern) {
    case 'flat':
      return 1
    case 'asphalt':
      return speckle(x, y, 0.55)
    case 'grass':
      return tufts(x, y)
    case 'water':
      return waves(x, y)
    case 'building':
      return facade(x, y)
    case 'gravel':
      return speckle(x, y, 0.8)
    case 'hatch':
      return hatch(x, y, 2.4)
    case 'contour':
      return contour(value, 12)
    default:
      return 1
  }
}

/** Applies a pattern to a colour, keeping the hue and modulating the brightness. */
export function applyPattern(
  colour: readonly [number, number, number],
  pattern: SurfacePattern,
  x: number,
  y: number,
  value = 0,
): [number, number, number] {
  const factor = 1 + (patternAt(pattern, x, y, value) - 1) * PATTERN_STRENGTH
  return [
    clampByte(colour[0] * factor),
    clampByte(colour[1] * factor),
    clampByte(colour[2] * factor),
  ]
}

/** Round to a byte, inside the byte range. */
function clampByte(value: number): number {
  return Math.max(0, Math.min(255, Math.round(value)))
}

/**
 * Fine grain: two octaves of value noise on a sub-metre grid.
 *
 * `contrast` picks how visible the grain is — asphalt is speckled, gravel is coarse.
 */
function speckle(x: number, y: number, contrast: number): number {
  const coarse = noiseAt(x / GRAIN_M, y / GRAIN_M)
  const fine = noiseAt(x / (GRAIN_M * 0.45), y / (GRAIN_M * 0.45))
  return 1 + (coarse * 0.6 + fine * 0.4 - 0.5) * contrast
}

/** Grass: coarse clumps with fine tufts between them. */
function tufts(x: number, y: number): number {
  const clump = noiseAt(x / 2.4, y / 2.4)
  const blade = noiseAt(x / 0.5, y / 0.5)
  return 1 + (clump - 0.5) * 0.5 + (blade - 0.5) * 0.25
}

/** Water: long, soft waves, so the pattern reads as a surface at rest. */
function waves(x: number, y: number): number {
  const wave = Math.sin((x * 0.6 + y * 0.35) / 3.1) * 0.5 + 0.5
  const ripple = noiseAt(x / 1.6, y / 1.6)
  return 1 + (wave * 0.6 + ripple * 0.4 - 0.5) * 0.22
}

/** A building: a façade grid with a light band every floor and a pier every bay. */
function facade(x: number, y: number): number {
  const bay = Math.abs((((x % 4) + 4) % 4) - 2) / 2
  const floor = Math.abs((((y % 3.2) + 3.2) % 3.2) - 1.6) / 1.6
  return 1 + (0.5 - Math.min(bay, floor)) * 0.3
}

/** Diagonal stripes: the convention for "restricted" or "no data" on a plan. */
function hatch(x: number, y: number, period: number): number {
  const diagonal = (x + y) / period
  const fraction = diagonal - Math.floor(diagonal)
  return 1 + (fraction < 0.5 ? 1 : -1) * 0.5
}

/** Bands of a scalar value: how a cost or a slope is read as a set of levels. */
function contour(value: number, levels: number): number {
  const scaled = value * levels
  const fraction = scaled - Math.floor(scaled)
  return 1 + (fraction < 0.15 ? -1 : 0) * 0.5
}

/** Whether a pattern needs the cell's value (only the contour pattern does). */
export function patternUsesValue(pattern: SurfacePattern): boolean {
  return pattern === 'contour'
}

/** Whether a pattern draws any texture at all; a flat pattern is drawn as one colour. */
export function patternHasTexture(pattern: SurfacePattern): boolean {
  return pattern !== 'flat'
}

/**
 * Value noise on the unit grid.
 *
 * Deterministic and cheap: an integer hash at the four surrounding lattice points, blended
 * smoothly, so the same world position always yields the same grain on every machine and
 * every rebuild.
 */
function noiseAt(x: number, y: number): number {
  const x0 = Math.floor(x)
  const y0 = Math.floor(y)
  const fx = smooth(x - x0)
  const fy = smooth(y - y0)
  const top = mix(hash2(x0, y0), hash2(x0 + 1, y0), fx)
  const bottom = mix(hash2(x0, y0 + 1), hash2(x0 + 1, y0 + 1), fx)
  return mix(top, bottom, fy)
}

/** Smoothstep of a lattice fraction. */
function smooth(t: number): number {
  return t * t * (3 - 2 * t)
}

/** Linear blend. */
function mix(a: number, b: number, t: number): number {
  return a + (b - a) * t
}
