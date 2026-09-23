/*
 * Terrain shading, as pure functions.
 *
 * The surface is coloured per vertex and lit by the scene's own lights, so everything
 * that makes a hill read as a hill has to happen here rather than in a shader: one
 * shader would need a WGSL and a GLSL variant to survive the engine fallback chain,
 * while a vertex colour travels with the mesh on every backend and can be tested
 * without a GPU.
 *
 * The style is a *model* rather than a map: a neutral surface whose form is told by
 * light, terracing and occlusion instead of by colour. Four effects, in this order —
 * a mild height ramp for the neutral base, a relief term for the sun, a darkening of
 * steep faces, and an occlusion term for hollows. Colour is what the *layers* carry;
 * a green hypsometric ramp on the ground competes with them and reads as vegetation
 * that the data does not contain.
 */
import { clamp01, normalize, rampAt, type Rgb } from '@/types/colormap'

/** How far the shaded side of a slope is allowed to fall, in luminance terms. */
const AMBIENT = 0.7

/** How much of the diffuse term a surface facing the sun receives. */
const DIFFUSE = 0.42

/** Slope at which the steep-face darkening starts, as the normal's y component. */
const STEEP_FROM = 0.92

/** Slope at which the darkening is at its strongest. */
const STEEP_TO = 0.55

/** Strongest darkening applied to a near-vertical face. */
const STEEP_STRENGTH = 0.22

/** How quickly a hollow darkens with its depth relative to its surroundings. */
const CAVITY_GAIN = 0.35

/** Strongest occlusion applied to a hollow one cell deep. */
const CAVITY_DEPTH = 0.22

/** Range a shading factor may take, so no vertex goes black or blows out. */
export const SHADE_RANGE: readonly [number, number] = [0.42, 1.12]

/** Default sun azimuth in the map plane, degrees clockwise from east. */
export const DEFAULT_SUN_AZIMUTH_DEG = 315

/** Default sun elevation above the horizon, degrees. */
export const DEFAULT_SUN_ELEVATION_DEG = 52

/**
 * Unit vector the light comes *from*, from an azimuth and an elevation.
 *
 * The same function feeds the vertex shading and the scene's directional light, so
 * "what the reader sees lit" and "what the colours call lit" cannot drift apart. The
 * azimuth is measured in the map plane clockwise from +x, which is how a compass
 * reading of the light is given.
 */
export function sunDirection(
  azimuthDeg = DEFAULT_SUN_AZIMUTH_DEG,
  elevationDeg = DEFAULT_SUN_ELEVATION_DEG,
): [number, number, number] {
  const azimuth = (azimuthDeg * Math.PI) / 180
  const elevation = (elevationDeg * Math.PI) / 180
  const horizontal = Math.cos(elevation)
  return [horizontal * Math.cos(azimuth), Math.sin(elevation), horizontal * Math.sin(azimuth)]
}

/**
 * Quantises a height into terraces of `step` metres.
 *
 * A height field read as a model is read in *steps*: stacking the surface into levels
 * gives it plateaux and cliff faces, which is what makes its shape legible at a glancing
 * angle. A step of zero leaves the surface as surveyed.
 */
export function terrace(height: number, step: number): number {
  if (!(step > 0)) {
    return height
  }
  return Math.round(height / step) * step
}

/**
 * Diffuse-plus-ambient shading of a surface normal.
 *
 * `AMBIENT + DIFFUSE * max(0, n · sun)`: a surface facing away from the sun keeps the
 * ambient term instead of going black, which is what makes a valley floor readable
 * rather than a silhouette.
 */
export function reliefShade(
  normal: readonly [number, number, number],
  sun: readonly [number, number, number] = sunDirection(),
): number {
  const dot = normal[0] * sun[0] + normal[1] * sun[1] + normal[2] * sun[2]
  const lit = AMBIENT + DIFFUSE * Math.max(0, dot)
  return Math.min(SHADE_RANGE[1], Math.max(SHADE_RANGE[0], lit))
}

/**
 * Darkening of a steep face, from the normal's vertical component.
 *
 * A flat surface has `y = 1` and is untouched; a cliff approaches `y = 0` and is
 * darkened. This is what separates two slopes of the same height apart from a contour
 * map.
 */
export function slopeAccent(normalY: number): number {
  const steepness = 1 - normalize(normalY, STEEP_TO, STEEP_FROM)
  return 1 - steepness * STEEP_STRENGTH
}

/**
 * Occlusion of a hollow, from its height relative to its surroundings.
 *
 * `relative` is how much lower the cell is than its neighbours, in cells of ground:
 * zero on a flat or convex cell, one when the cell stands a full cell below its
 * surroundings. Terracing makes this term do most of the work — every riser casts a
 * band of shade onto the terrace below it, which is what reads as a stack of blocks.
 */
export function cavityShade(relative: number): number {
  return 1 - clamp01(relative * CAVITY_GAIN) * CAVITY_DEPTH
}

/**
 * Colour of one terrain vertex.
 *
 * `range` is the elevation range of the whole surface being drawn, so the ramp is
 * comparable across chunks rather than rescaled per chunk — a per-chunk range would
 * make each one a different colour for the same height. `shade` carries every lighting
 * term (relief, steepness, occlusion), already multiplied together.
 */
export function terrainColour(
  height: number,
  shade: number,
  ramp: readonly Rgb[],
  range: { min: number; max: number },
): Rgb {
  const base = rampAt(ramp, normalize(height, range.min, range.max))
  return [
    Math.min(255, Math.max(0, Math.round(base[0] * shade))),
    Math.min(255, Math.max(0, Math.round(base[1] * shade))),
    Math.min(255, Math.max(0, Math.round(base[2] * shade))),
  ]
}

/** Colour of a slab face: the surface colour, sunk into shadow. */
export function slabColour(surface: Rgb, factor = 0.78): Rgb {
  return [
    Math.max(0, Math.round(surface[0] * factor)),
    Math.max(0, Math.round(surface[1] * factor)),
    Math.max(0, Math.round(surface[2] * factor)),
  ]
}

/**
 * Terrace step to use when the reader has not chosen one, metres.
 *
 * Derived from the map's own relief so a 5 m hill is not terraced into nothing and a 400 m
 * valley is not terraced into one step: the step is one, two or five times a power of ten,
 * chosen to give roughly `levels` steps across the range. Fewer levels than one might
 * expect, because a gentle slope terraced finely produces a riser every few cells and the
 * surface reads as a hatch rather than as land.
 */
export function autoTerraceStep(range: { min: number; max: number }, levels = 12): number {
  const span = range.max - range.min
  if (!(span > 0) || !Number.isFinite(span)) {
    return 0
  }
  const raw = span / Math.max(1, levels)
  const magnitude = 10 ** Math.floor(Math.log10(raw))
  const normalized = raw / magnitude
  // Rounded *up* to the next one, two or five: rounding down would leave the map with more
  // terraces than the reader asked for, and the point of a step is that one terrace is a
  // legible unit rather than a fine quantisation.
  const step = normalized > 5 ? 10 : normalized > 2 ? 5 : normalized > 1 ? 2 : 1
  return step * magnitude
}

/**
 * Thickness of the slab under a map, metres.
 *
 * A fraction of the map's own relief, so a flat map is not given a tower of a base and a
 * mountain is not given a sheet: the model reads as one piece.
 */
export function slabThickness(range: { min: number; max: number }, fraction = 0.6): number {
  const span = range.max - range.min
  if (!Number.isFinite(span) || span <= 0) {
    return 4
  }
  return Math.min(80, Math.max(2, span * fraction))
}
