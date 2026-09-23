/*
 * What a layer's cells mean, and how each meaning is drawn.
 *
 * A cell's colour says *what it is*; its pattern says *what it is made of*. Both come from
 * the map's own feature schema — `feature_schema` names every dimension (`surface_type`,
 * `traffic`, `direction`, …) and declares whether it is a category or a scalar — so a
 * viewer that reads it can draw a road as asphalt and a lawn as grass instead of painting
 * both with the same ramp. The tables below are the display half of that schema: the file
 * carries the semantics, this module carries the colours and textures for them.
 *
 * Pure data and pure functions: no Vue, no Babylon, so the mapping is testable and the same
 * numbers feed the legend.
 */
import type { Rgb } from '@/types/colormap'
import {
  FEATURE_BASE,
  LAYER_DIRECTION,
  LAYER_EDT,
  LAYER_HARD_FORBIDDEN,
  LAYER_SLOPE,
} from '@/types/map'
import type { SurfacePattern } from './patterns'

/** One material as it is drawn: a colour, a texture and a name for the legend. */
export interface SurfaceMaterial {
  /** Name shown in the legend and the inspector; a catalog key. */
  labelKey: string
  /** Base colour of the material, before its pattern modulates it. */
  colour: Rgb
  /** Texture the material is drawn with. */
  pattern: SurfacePattern
}

/**
 * Surface classes of a `surface_type` dimension, in the order the format numbers them.
 *
 * The order is the map format's own (`map-format`'s `surface` module: road, track, grass,
 * sidewalk, dirt), and the colours are the ones a reader already associates with them —
 * asphalt grey, track brown, lawn green, paving, bare earth.
 */
export const SURFACE_MATERIALS: readonly SurfaceMaterial[] = [
  { labelKey: 'map.surface.road', colour: [74, 76, 80], pattern: 'asphalt' },
  { labelKey: 'map.surface.track', colour: [124, 92, 60], pattern: 'gravel' },
  { labelKey: 'map.surface.grass', colour: [108, 142, 78], pattern: 'grass' },
  { labelKey: 'map.surface.sidewalk', colour: [168, 166, 158], pattern: 'gravel' },
  { labelKey: 'map.surface.dirt', colour: [146, 122, 88], pattern: 'gravel' },
]

/** One dimension of the map's feature schema, as far as the viewer reads it. */
export interface FeatureDimension {
  /** Dimension name, e.g. `surface_type`. */
  name: string
  /** Layer the dimension lives in. */
  layerId: number
  /** `category`, `scalar`, `direction` or `bitmap`, as the file declares it. */
  kind: string
  /** Categories the dimension declares, when it is a category one. */
  materials?: readonly SurfaceMaterial[]
}

/** Reads `feature_schema` from the metadata JSON into the dimensions a viewer draws. */
export function featureDimensions(schema: unknown): FeatureDimension[] {
  const dims = readArray(readProperty(schema, 'dims'))
  const out: FeatureDimension[] = []
  for (const entry of dims) {
    const name = readProperty(entry, 'name')
    const layerId = readProperty(entry, 'layer_id')
    if (typeof name !== 'string' || typeof layerId !== 'number') {
      continue
    }
    const kind = readProperty(entry, 'kind')
    out.push({
      name,
      layerId,
      kind: typeof kind === 'string' ? kind : 'scalar',
      // Only `surface_type` has a material table; another category dimension would need its
      // own, and until it has one the palette is the honest answer.
      materials: name === 'surface_type' ? SURFACE_MATERIALS : undefined,
    })
  }
  return out
}

/** How one layer is drawn: a pattern for its cells and, when it has materials, a table. */
export interface LayerSurfacePlan {
  /** Pattern every cell of the layer is textured with. */
  pattern: SurfacePattern
  /** Materials of a category dimension, in the format's own order. */
  materials: readonly SurfaceMaterial[] | null
  /** The dimension the layer belongs to, when the schema names one. */
  dimension: FeatureDimension | null
}

/**
 * How a layer's cells are textured.
 *
 * Semantics first: a restriction mask is hatched (the plan convention for "no"), a cost or
 * a slope is contoured (it is a level, not a material), and a named category dimension uses
 * its own material table. Everything else is flat, which is the honest default for a field
 * whose meaning the file does not state.
 */
export function layerSurfacePlan(
  layerId: number,
  kind: string,
  dimensions: readonly FeatureDimension[],
): LayerSurfacePlan {
  const dimension = dimensions.find((entry) => entry.layerId === layerId) ?? null
  if (layerId === LAYER_HARD_FORBIDDEN) {
    return { pattern: 'hatch', materials: null, dimension }
  }
  if (layerId === LAYER_EDT || layerId === LAYER_SLOPE) {
    return { pattern: 'contour', materials: null, dimension }
  }
  if (dimension?.materials !== undefined) {
    return { pattern: 'flat', materials: dimension.materials, dimension }
  }
  if (dimension?.kind === 'direction' || layerId === LAYER_DIRECTION) {
    return { pattern: 'flat', materials: null, dimension }
  }
  if (kind === 'bitmap') {
    return { pattern: 'hatch', materials: null, dimension }
  }
  return { pattern: 'flat', materials: null, dimension }
}

/** Material a cell value names, or `null` when the layer has no material table. */
export function materialForValue(plan: LayerSurfacePlan, value: number): SurfaceMaterial | null {
  const table = plan.materials
  if (table === null || table.length === 0) {
    return null
  }
  const index = Math.trunc(value)
  if (index < 0 || index >= table.length) {
    return null
  }
  return table[index] ?? null
}

/** True when a layer id names a resistance feature dimension. */
export function isFeatureLayer(layerId: number): boolean {
  return layerId >= FEATURE_BASE && layerId < FEATURE_BASE + 0x100
}

/** Reads a property off an unknown JSON value. */
function readProperty(value: unknown, key: string): unknown {
  return typeof value === 'object' && value !== null
    ? (value as Record<string, unknown>)[key]
    : undefined
}

/** Reads a value as an array, or an empty one. */
function readArray(value: unknown): unknown[] {
  return Array.isArray(value) ? value : []
}
