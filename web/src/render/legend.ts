/*
 * The legend model: what each visible layer is coloured by.
 *
 * A legend has to answer "what does this colour mean" for *every* layer on the ground,
 * not for one of them: the failure this replaces was a legend titled "grey" beside a
 * green surface, describing a mapping the terrain did not use. The model is built from
 * the layers that are actually drawn and from the samples that are actually loaded, so it
 * describes the screen rather than the file — and it never invents a category that no
 * loaded cell carries.
 *
 * Pure data: names are canonical layer names and the numbers are the samples' own values;
 * the component translates and formats.
 */
import { CATEGORY_PALETTE, SCALAR_RAMPS, rampCss, rgbToCss, valueRange } from '@/types/colormap'
import type { Rgb } from '@/types/colormap'
import { flagColour, type ColorMapping } from '@/types/map'
import { sampleColour } from './layerTexture'
import type { SurfacePattern } from './patterns'
import type { SurfaceMaterial } from './surfaces'

/** One colour of a discrete legend, with the value it stands for. */
export interface LegendSwatch {
  /** Value the swatch stands for, as text in the interface locale. */
  label: string
  /** CSS colour. */
  colour: string
  /** Catalog key of a material name, when the swatch stands for one. */
  labelKey?: string
}

/** One layer's entry in the legend. */
export interface LegendEntry {
  /** Layer id, so a test or a control can address the row. */
  layerId: number
  /** Canonical layer name, as the map reports it; the component translates it. */
  name: string
  /** How the layer's samples are coloured. */
  mapping: ColorMapping
  /** Gradient of a ramp mapping, or `null` for a discrete one. */
  ramp: string | null
  /** Discrete colours of a category or flag mapping, empty for a ramp. */
  swatches: LegendSwatch[]
  /** Gradient stops as colours, for a caller that wants them separately. */
  rampStops: string[]
  /** Tick values under a ramp, lowest first; empty for a discrete mapping. */
  ticks: number[]
  /** Unit of the sampled values, when the caller knows one. */
  unit: string | null
}

/** Input of {@link legendEntries}: one layer that is drawn on the map. */
export interface LegendLayerInput {
  /** Layer id. */
  layerId: number
  /** Canonical name. */
  name: string
  /** Mapping in effect. */
  mapping: ColorMapping
  /** Value range of the loaded samples, or `null`/absent when none are loaded. */
  range?: { min: number; max: number } | null
  /** Loaded samples of this layer, for the discrete end of the legend. */
  samples?: ArrayLike<number>
  /** Unit of the values, when the caller knows one. */
  unit?: string | null
  /**
   * Materials a category dimension names, in the format's own value order.
   *
   * When a layer has them, the legend lists the materials rather than the palette: "asphalt,
   * track, grass" is what a reader needs to read the map, and "0, 1, 2" is not.
   */
  materials?: readonly SurfaceMaterial[] | null
  /** Pattern the layer's cells are textured with, drawn on each swatch. */
  pattern?: SurfacePattern
  /**
   * Ramp to draw, when it is not one of the named mappings.
   *
   * The surface is drawn with the neutral terrain ramp rather than with a named scalar
   * mapping, so its legend has to be handed the ramp it is actually painted with — a
   * legend that assumed the layer's own ramp would describe a surface nobody is looking at.
   */
  ramp?: readonly Rgb[] | null
}

/** Options of {@link legendEntries}. */
export interface LegendOptions {
  /** Most entries to describe; the rest are reported as a count. */
  limit?: number
  /** Most swatches per discrete entry. */
  maxSwatches?: number
}

/** The legend: one entry per layer, plus how many were left out. */
export interface LegendModel {
  /** Entries to draw, in the order the layers were given. */
  entries: LegendEntry[]
  /** Number of drawn layers the entries do not cover. */
  hidden: number
}

/**
 * Builds the legend of the layers on screen.
 *
 * A ramp entry shows the range of the *loaded* samples, which is what is on the ground;
 * a discrete entry shows the values that occur in them, so a mask layer with nothing set
 * in view says so by showing no swatch at all.
 */
export function legendEntries(
  layers: readonly LegendLayerInput[],
  options: LegendOptions = {},
): LegendModel {
  const limit = Math.max(1, options.limit ?? 6)
  const maxSwatches = Math.max(1, options.maxSwatches ?? 8)
  const entries = layers.slice(0, limit).map((layer) => entryFor(layer, maxSwatches))
  return { entries, hidden: Math.max(0, layers.length - entries.length) }
}

/** One layer's entry. */
function entryFor(layer: LegendLayerInput, maxSwatches: number): LegendEntry {
  const range = layer.range ?? rangeOf(layer.samples)
  const base: LegendEntry = {
    layerId: layer.layerId,
    name: layer.name,
    mapping: layer.mapping,
    ramp: null,
    swatches: [],
    rampStops: [],
    ticks: [],
    unit: layer.unit ?? null,
  }
  if (layer.mapping === 'flag') {
    const colour = rgbToCss(flagColour(layer.layerId))
    const set = hasSetValue(layer.samples)
    return { ...base, swatches: set ? [{ label: '1', colour }] : [] }
  }
  if (layer.mapping === 'category' || (layer.mapping === 'material' && materialsPresent(layer))) {
    // The values that are actually on screen, in ascending order. No fallback swatch: a
    // legend that always shows "0" claims the layer is there when the view holds none of
    // it, which is the sort of quiet lie the panel must not tell.
    const present = distinctCategories(layer.samples, maxSwatches)
    const materials = layer.materials ?? null
    return {
      ...base,
      swatches:
        materials === null
          ? present.map((value) => {
              const colour = sampleColour(value, 'category', range) ?? [0, 0, 0]
              return { label: String(value), colour: rgbToCss(colour) }
            })
          : // With a material table the *value* is still the key, but the name is what a reader
            // reads: the label carries the material's catalog key and the value stays beside it.
            present.flatMap((value): LegendSwatch[] => {
              const material = materials[value]
              return material === undefined
                ? []
                : [
                    {
                      label: String(value),
                      labelKey: material.labelKey,
                      colour: rgbToCss(material.colour),
                    },
                  ]
            }),
    }
  }
  // A material mapping with nothing to name its values falls through to the palette, which is
  // what the panel shows for a layer whose map does not declare one.
  const stops =
    layer.ramp ?? (layer.mapping === 'material' ? CATEGORY_PALETTE : SCALAR_RAMPS[layer.mapping])
  return {
    ...base,
    ramp: rampCss(stops),
    rampStops: stops.map(rgbToCss),
    ticks: ticksOf(range),
  }
}

/** Whether a layer brought a material table to name its values with. */
function materialsPresent(layer: LegendLayerInput): boolean {
  return (layer.materials?.length ?? 0) > 0
}

/** Range of a sample array, or a neutral one when there is nothing to measure. */
function rangeOf(samples: ArrayLike<number> | undefined): { min: number; max: number } {
  return samples === undefined ? { min: 0, max: 1 } : valueRange(samples)
}

/** Whether any loaded sample of a mask is set. */
function hasSetValue(samples: ArrayLike<number> | undefined): boolean {
  if (samples === undefined) {
    return false
  }
  for (let index = 0; index < samples.length; index += 1) {
    const value = samples[index]
    if (typeof value === 'number' && Number.isFinite(value) && value !== 0) {
      return true
    }
  }
  return false
}

/**
 * Distinct integer values of a categorical layer, in ascending order.
 *
 * Only values that *are* integers count: a continuous field drawn with the discrete palette
 * has no categories to name, and listing its trunkated values would put a swatch in the
 * legend for a colour no cell is painted with.
 */
function distinctCategories(samples: ArrayLike<number> | undefined, limit: number): number[] {
  if (samples === undefined) {
    return []
  }
  const seen = new Set<number>()
  for (let index = 0; index < samples.length; index += 1) {
    const value = samples[index]
    if (typeof value !== 'number' || !Number.isFinite(value) || value === 0) {
      continue
    }
    const rounded = Math.round(value)
    if (Math.abs(value - rounded) > 0.05) {
      return []
    }
    seen.add(rounded)
    if (seen.size > limit) {
      break
    }
  }
  return [...seen].toSorted((a, b) => a - b).slice(0, limit)
}

/** Tick values under a ramp: the ends and three steps between them. */
function ticksOf(range: { min: number; max: number }): number[] {
  const steps = 4
  return Array.from({ length: steps + 1 }, (_, index) => {
    const value = range.min + ((range.max - range.min) * index) / steps
    return Math.round(value * 1000) / 1000
  })
}
