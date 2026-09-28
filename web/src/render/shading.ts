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
 * Shading multiplier of a terraced wall, relative to the surface above it.
 *
 * A wall is read as the *side* of the block it carries, so it takes the colour of the
 * surface at its top and is sunk into shadow by this much.
 */
export const SLAB_WALL_SHADE = 0.78

/** Shading multiplier of the floor under a map, darker than the walls around it. */
export const SLAB_FLOOR_SHADE = SLAB_WALL_SHADE * 0.6

/**
 * Colour of a building footprint, in bytes.
 *
 * A navigation display draws a building as a near-white block, untextured and barely off the
 * page's white: a footprint is a *volume* the reader should read as built-up ground, and any
 * tint or pattern spent on it competes with the layers that carry the actual data.
 */
export const BUILDING_COLOUR: readonly [number, number, number] = [242, 242, 242]

/**
 * Vertex colours of a whole mesh, from the ramp inputs the geometry carries.
 *
 * Each pair of floats is one vertex: the elevation normalised against the whole surface's
 * range, and the shading factor. Splitting the colour out of the geometry is what lets an
 * appearance change be an upload of this buffer rather than a rebuild of the map.
 *
 * `building` is the per-vertex mask {@link BUILDING_COLOUR} replaces the ramp for; without it
 * every vertex is coloured by the ramp, which is what a map with no mask channel gets.
 */
export function terrainColours(
  rampInput: Float32Array,
  ramp: readonly Rgb[],
  building?: Float32Array,
): Float32Array {
  const colors = new Float32Array((rampInput.length / 2) * 4)
  for (let vertex = 0; vertex < rampInput.length / 2; vertex += 1) {
    const base =
      building?.[vertex] === 1 ? BUILDING_COLOUR : rampAt(ramp, rampInput[vertex * 2] ?? 0)
    const shade = rampInput[vertex * 2 + 1] ?? 1
    colors[vertex * 4] = Math.min(1, Math.max(0, (base[0] * shade) / 255))
    colors[vertex * 4 + 1] = Math.min(1, Math.max(0, (base[1] * shade) / 255))
    colors[vertex * 4 + 2] = Math.min(1, Math.max(0, (base[2] * shade) / 255))
    colors[vertex * 4 + 3] = 1
  }
  return colors
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
