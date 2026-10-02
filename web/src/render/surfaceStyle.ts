/*
 * The surface style: everything a view bakes into a terrain mesh.
 *
 * Two views draw the same terrain — the preview and the run workspace — and a parameter
 * set in one has to produce the same surface in the other, so the derivation lives here
 * rather than in each view. It also owns the *automatic* choices: a terrace step that fits
 * the map's own relief and a slab thickness that fits its span, which is what an
 * unconfigured viewport should show.
 *
 * The light/dark appearance is deliberately absent: the geometry carries a ramp input
 * rather than a colour, so the appearance is applied to the colour buffer afterwards and
 * switching it rebuilds nothing. Only the values that move vertices belong here.
 *
 * Pure: the inputs are values, not stores, so the mapping from a viewport parameter to a
 * mesh option can be tested without a canvas.
 */
import { autoTerraceStep, slabThickness } from './shading'
import type { ChunkMeshOptions } from './terrainMesh'

/** Inputs of {@link surfaceStyle}, as the viewport holds them. */
export interface SurfaceStyleInput {
  /** Elevation range of the whole map, or `null` before the metadata is read. */
  range: { min: number; max: number } | null
  /** Terrace step the reader chose, or `null` for the step the relief implies. */
  terraceM: number | null
  /** Azimuth of the sun in the map plane, degrees. */
  sunAzimuthDeg: number
  /** Height of the sun above the horizon, degrees. */
  sunElevationDeg?: number
}

/** Terrace step in effect for a style input, metres; zero means "no terracing". */
export function effectiveTerraceStep(input: SurfaceStyleInput): number {
  if (input.terraceM !== null) {
    return Math.max(0, input.terraceM)
  }
  return input.range === null ? 0 : autoTerraceStep(input.range)
}

/** World height of the slab's underside for a style input. */
export function slabFloorOf(input: SurfaceStyleInput): number {
  return input.range === null ? 0 : input.range.min - slabThickness(input.range)
}

/** The mesh options one style input produces. */
export function surfaceStyle(input: SurfaceStyleInput): ChunkMeshOptions {
  return {
    range: input.range ?? undefined,
    terraceM: effectiveTerraceStep(input),
    slabFloorY: slabFloorOf(input),
  }
}

/**
 * Identity of a style, so a view can rebuild only when a baked value changed.
 *
 * The sun is deliberately absent: it no longer bakes into the surface — the scene's light
 * shades it per fragment — so turning the sun round is a light change, not a rebuild.
 */
export function surfaceKey(input: SurfaceStyleInput): string {
  return JSON.stringify([
    input.range?.min ?? null,
    input.range?.max ?? null,
    effectiveTerraceStep(input),
  ])
}
