/*
 * Colour mapping.
 *
 * Two consumers share these functions: the terrain and layer textures, which map
 * a scalar to an RGBA byte, and the charts, whose series palette has to agree with
 * the ramps or a legend would contradict the surface beside it. Everything returns
 * byte triples rather than CSS strings, because a texture upload wants bytes and a
 * hex string would have to be parsed back.
 *
 * The values are literal because a GPU texture and an echarts theme are both
 * built from numbers, not from the CSS custom properties the interface uses:
 * `styles/theme.css` carries the interface's own tokens, and this file carries the
 * data's. A unit test checks that the shared neutrals and the brand green appear
 * in both.
 */

/** A colour as three bytes, red first. */
export type Rgb = readonly [number, number, number]

/** Discrete palette for category-shaped layers; colour-blind safe and ordered so neighbours differ. */
export const CATEGORY_PALETTE: readonly Rgb[] = [
  [63, 125, 78],
  [183, 121, 31],
  [106, 90, 160],
  [42, 128, 138],
  [176, 74, 118],
  [122, 92, 60],
  [90, 96, 104],
  [158, 158, 92],
]

/** Series palette of the charts, the same eight colours the layers use. */
export const SERIES_PALETTE: readonly string[] = CATEGORY_PALETTE.map(rgbToHex)

/** Neutral ink colour of the light appearance, used for text in a chart theme. */
export const INK_LIGHT: Rgb = [28, 28, 30]

/** Neutral ink colour of the dark appearance. */
export const INK_DARK: Rgb = [232, 232, 234]

/** Surface colour of the light appearance. */
export const SURFACE_LIGHT: Rgb = [255, 255, 255]

/** Surface colour of the dark appearance. */
export const SURFACE_DARK: Rgb = [23, 23, 26]

/**
 * Parses a CSS colour into a byte triple.
 *
 * The tokens are read off the document at runtime, and a browser is free to hand
 * back a shorthand — `#fff` for `#ffffff` — so every form the token files use is
 * accepted: three, four, six and eight digit hex, and `rgb()`/`rgba()`. Anything
 * else reads as mid grey, which is visible as a wrong colour rather than as a
 * crash.
 */
export function parseCssColor(value: string): Rgb {
  const text = value.trim()
  const hex = /^#?([0-9a-f]{3,8})$/i.exec(text)
  const digits = hex?.[1]
  if (digits !== undefined && [3, 4, 6, 8].includes(digits.length)) {
    const expanded =
      digits.length <= 4
        ? digits
            .slice(0, 3)
            .split('')
            .map((digit) => digit + digit)
            .join('')
        : digits.slice(0, 6)
    const packed = Number.parseInt(expanded, 16)
    return [(packed >> 16) & 0xff, (packed >> 8) & 0xff, packed & 0xff]
  }
  const rgb = /rgba?\(\s*([\d.]+)[\s,]+([\d.]+)[\s,]+([\d.]+)/i.exec(text)
  if (rgb !== null) {
    const channel = (index: number): number =>
      Math.max(0, Math.min(255, Math.round(Number(rgb[index]))))
    return [channel(1), channel(2), channel(3)]
  }
  return [128, 128, 128]
}

/** Formats a byte triple as `#rrggbb`. */
export function rgbToHex(rgb: Rgb): string {
  return `#${rgb.map((channel) => channel.toString(16).padStart(2, '0')).join('')}`
}

/** Formats a byte triple as a CSS `rgb()` colour. */
export function rgbToCss(rgb: Rgb): string {
  return `rgb(${rgb[0]} ${rgb[1]} ${rgb[2]})`
}

/** Clamps a value into `[0, 1]`; a non-finite value becomes zero. */
export function clamp01(value: number): number {
  if (!Number.isFinite(value)) {
    return 0
  }
  return Math.min(1, Math.max(0, value))
}

/** Linear interpolation between two bytes. */
function mix(from: number, to: number, t: number): number {
  return Math.round(from + (to - from) * t)
}

/** Interpolates a colour ramp at `t` in `[0, 1]`. */
export function rampAt(stops: readonly Rgb[], t: number): Rgb {
  if (stops.length === 0) {
    return [0, 0, 0]
  }
  const first = stops[0] ?? [0, 0, 0]
  const last = stops[stops.length - 1] ?? first
  if (stops.length === 1) {
    return first
  }
  const scaled = clamp01(t) * (stops.length - 1)
  const index = Math.min(stops.length - 2, Math.floor(scaled))
  const local = scaled - index
  const from = stops[index] ?? first
  const to = stops[index + 1] ?? last
  return [mix(from[0], to[0], local), mix(from[1], to[1], local), mix(from[2], to[2], local)]
}

/** Grey ramp of the scalar layer display: low values dark, high values light. */
export const GREY_RAMP: readonly Rgb[] = [
  [24, 24, 26],
  [128, 128, 132],
  [246, 246, 247],
]

/** Speed ramp: the palette greens through amber to the danger red, in that order. */
export const SPEED_RAMP: readonly Rgb[] = [
  [42, 128, 138],
  [63, 125, 78],
  [158, 158, 92],
  [183, 121, 31],
  [180, 52, 47],
]

/**
 * Elevation ramp of the *layer* view: low ground to ridge line.
 *
 * A hypsometric ramp — the convention a topographic map uses — so a height read as a
 * layer is a height: deep green for the lowest ground, then grass, then dry grass, then
 * rock, then snow. The surface itself is drawn in {@link TERRAIN_RAMP} instead: a
 * thematic layer may be terrain-shaped, the terrain is not a thematic layer.
 */
export const HEIGHT_RAMP: readonly Rgb[] = [
  [46, 84, 66],
  [86, 122, 74],
  [140, 156, 92],
  [196, 182, 121],
  [172, 147, 116],
  [150, 146, 146],
  [238, 240, 242],
]

/**
 * Surface ramp of the model: a neutral clay.
 *
 * Almost no colour, so the shape is told by light and terracing and the *layers* keep
 * their meaning: a saturated ground ramp competes with every drape laid on it, and a
 * green one reads as vegetation the data does not contain. The ramp still runs from
 * darker to lighter with height, which is what keeps relief readable at a glance.
 */
export const TERRAIN_RAMP: readonly Rgb[] = [
  [156, 165, 176],
  [186, 193, 202],
  [214, 219, 225],
  [236, 239, 242],
  [248, 249, 251],
]

/** The same ramp for the dark appearance: a dark clay rather than a white slab. */
export const TERRAIN_RAMP_DARK: readonly Rgb[] = [
  [58, 63, 70],
  [86, 93, 102],
  [118, 126, 136],
  [152, 160, 170],
  [188, 195, 203],
]

/** Cost ramp: a sequential cool-to-warm ramp for derived cost and distance fields. */
export const COST_RAMP: readonly Rgb[] = [
  [32, 82, 116],
  [42, 128, 138],
  [142, 160, 96],
  [183, 121, 31],
  [146, 62, 92],
]

/** The ramps the layer panel offers for scalar data. */
export const SCALAR_RAMPS: Readonly<Record<'grey' | 'speed' | 'height' | 'cost', readonly Rgb[]>> =
  {
    grey: GREY_RAMP,
    speed: SPEED_RAMP,
    height: HEIGHT_RAMP,
    cost: COST_RAMP,
  }

/** Picks the colour of one sample under a named mapping. */
export function colorFor(value: number, mapping: 'grey' | 'speed' | 'height' | 'cost'): Rgb {
  return rampAt(SCALAR_RAMPS[mapping], value)
}

/** Picks a palette colour by integer index; the palette wraps. */
export function colorForCategory(index: number): Rgb {
  const count = CATEGORY_PALETTE.length
  const slot = ((Math.trunc(index) % count) + count) % count
  return CATEGORY_PALETTE[slot] ?? CATEGORY_PALETTE[0] ?? [0, 0, 0]
}

/** Normalises a value into `[0, 1]` against a range, saturating at both ends. */
export function normalize(value: number, min: number, max: number): number {
  const span = max - min
  if (!Number.isFinite(span) || Math.abs(span) < Number.EPSILON) {
    return 0.5
  }
  return clamp01((value - min) / span)
}

/** Smallest and largest finite value of a sample array; both zero when it holds none. */
export function valueRange(values: ArrayLike<number>): { min: number; max: number } {
  let min = Number.POSITIVE_INFINITY
  let max = Number.NEGATIVE_INFINITY
  for (let index = 0; index < values.length; index += 1) {
    const value = values[index]
    if (typeof value !== 'number' || !Number.isFinite(value)) {
      continue
    }
    min = Math.min(min, value)
    max = Math.max(max, value)
  }
  if (min === Number.POSITIVE_INFINITY) {
    return { min: 0, max: 0 }
  }
  return { min, max }
}

/**
 * Perceptual ramp of the trajectory line and the speed legend.
 *
 * Kept separate from the layer ramps: a speed-coloured trajectory sits on top of a
 * grey terrain, so it needs the full chromatic span rather than a subtle one.
 */
export function speedColor(speed: number, min: number, max: number): Rgb {
  return rampAt(SPEED_RAMP, normalize(speed, min, max))
}

/** A CSS gradient of a ramp, for the legend under a layer control. */
export function rampCss(stops: readonly Rgb[]): string {
  const first = stops[0]
  if (first === undefined) {
    return 'none'
  }
  if (stops.length === 1) {
    return rgbToCss(first)
  }
  const parts = stops.map(
    (stop, index) => `${rgbToCss(stop)} ${(index / (stops.length - 1)) * 100}%`,
  )
  return `linear-gradient(90deg, ${parts.join(', ')})`
}
