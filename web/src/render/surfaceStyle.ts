/*
 * The surface style: everything a view bakes into a terrain mesh.
 *
 * Two views draw the same terrain — the preview and the run workspace — and a parameter
 * set in one has to produce the same surface in the other, so the derivation lives here
 * rather than in each view. It also owns the *automatic* choices: a terrace step that fits
 * the map's own relief and a slab thickness that fits its span, which is what an
 * unconfigured viewport should show.
 *
 * Pure: the inputs are values, not stores, so the mapping from a viewport parameter to a
 * mesh option can be tested without a canvas.
 */
import { TERRAIN_RAMP, TERRAIN_RAMP_DARK } from '@/types/colormap'
import { autoTerraceStep, slabThickness, sunDirection } from './shading'
import type { ChunkMeshOptions } from './terrainMesh'

/** Inputs of {@link surfaceStyle}, as the viewport holds them. */
export interface SurfaceStyleInput {
  /** Elevation range of the whole map, or `null` before the metadata is read. */
  range: { min: number; max: number } | null
  /** Whether the dark appearance is in use. */
  dark: boolean
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
    ramp: input.dark ? TERRAIN_RAMP_DARK : TERRAIN_RAMP,
    terraceM: effectiveTerraceStep(input),
    sun: sunDirection(input.sunAzimuthDeg, input.sunElevationDeg),
    slabFloorY: slabFloorOf(input),
  }
}

/** Identity of a style, so a view can rebuild only when a baked value changed. */
export function surfaceKey(input: SurfaceStyleInput): string {
  return JSON.stringify([
    input.range?.min ?? null,
    input.range?.max ?? null,
    input.dark,
    effectiveTerraceStep(input),
    input.sunAzimuthDeg,
    input.sunElevationDeg ?? null,
  ])
}
