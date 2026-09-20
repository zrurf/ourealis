/*
 * The echarts theme, in both appearances.
 *
 * echarts paints into a canvas and cannot read the interface's CSS custom
 * properties, so the chart palette is registered as literal colours and the theme
 * is rebuilt — not patched — when the appearance changes. The values are the data
 * palette of `src/types/colormap.ts` plus the interface's neutrals; the unit lane
 * checks that the shared ones still match `styles/theme.css`.
 */
import {
  INK_DARK,
  INK_LIGHT,
  SERIES_PALETTE,
  SURFACE_DARK,
  SURFACE_LIGHT,
  rgbToHex,
} from '@/types/colormap'

/** Name the light theme is registered under. */
export const CHART_THEME_LIGHT = 'ourealis-light'

/** Name the dark theme is registered under. */
export const CHART_THEME_DARK = 'ourealis-dark'

/** The echarts theme object, as `echarts.registerTheme` takes it. */
export interface ChartTheme {
  /** Series palette, in the order echarts hands colours to series. */
  color: string[]
  /** Global font settings. */
  textStyle: Record<string, unknown>
  /** Canvas background; transparent so the page's own surface shows through. */
  backgroundColor: string
  /** Axis styling that applies to every axis unless a builder overrides it. */
  categoryAxis: Record<string, unknown>
  /** Value axis styling. */
  valueAxis: Record<string, unknown>
  /** Tooltip styling. */
  tooltip: Record<string, unknown>
  /** Legend styling. */
  legend: Record<string, unknown>
  /** Title styling. */
  title: Record<string, unknown>
}

/** Line colour of the neutral grid, per appearance. */
const GRID_LIGHT = 'rgba(28, 28, 30, 0.10)'
const GRID_DARK = 'rgba(232, 232, 234, 0.12)'

/** Muted label colour, per appearance. */
const MUTED_LIGHT = rgbToHex([110, 110, 115])
const MUTED_DARK = rgbToHex([154, 154, 160])

/** Border colour of a floating surface, per appearance. */
const LINE_LIGHT = 'rgba(28, 28, 30, 0.12)'
const LINE_DARK = 'rgba(232, 232, 234, 0.16)'

/**
 * Builds the theme of one appearance.
 *
 * The doc's rules are applied here rather than in every builder: no grid
 * background noise, only horizontal grid lines, 12px muted axis labels, and a
 * tooltip that follows the appearance.
 */
export function chartTheme(dark: boolean): ChartTheme {
  const ink = rgbToHex(dark ? INK_DARK : INK_LIGHT)
  const muted = dark ? MUTED_DARK : MUTED_LIGHT
  const surface = rgbToHex(dark ? SURFACE_DARK : SURFACE_LIGHT)
  const grid = dark ? GRID_DARK : GRID_LIGHT
  const line = dark ? LINE_DARK : LINE_LIGHT
  return {
    color: [...SERIES_PALETTE],
    backgroundColor: 'transparent',
    textStyle: { fontFamily: 'system-ui, sans-serif', fontSize: 12, color: ink },
    categoryAxis: axisStyle(grid, muted),
    valueAxis: axisStyle(grid, muted),
    tooltip: {
      backgroundColor: surface,
      borderColor: line,
      borderWidth: 1,
      textStyle: { color: ink, fontSize: 12 },
      extraCssText: 'box-shadow: none; border-radius: 6px;',
    },
    legend: {
      textStyle: { color: muted, fontSize: 12 },
      icon: 'roundRect',
      itemWidth: 10,
      itemHeight: 10,
    },
    title: { textStyle: { color: ink, fontSize: 14, fontWeight: 600 } },
  }
}

/** Axis style shared by the two axis kinds: thin horizontal lines only, muted labels. */
function axisStyle(grid: string, muted: string): Record<string, unknown> {
  return {
    axisLine: { show: true, lineStyle: { color: grid } },
    axisTick: { show: false },
    axisLabel: { color: muted, fontSize: 12 },
    splitLine: { show: true, lineStyle: { color: grid, width: 1, type: 'solid' } },
    splitArea: { show: false },
  }
}

/** Both themes, keyed by the name they are registered under. */
export const CHART_THEMES: Readonly<Record<string, ChartTheme>> = {
  [CHART_THEME_LIGHT]: chartTheme(false),
  [CHART_THEME_DARK]: chartTheme(true),
}

/** Theme name to render an appearance with. */
export function themeNameFor(dark: boolean): string {
  return dark ? CHART_THEME_DARK : CHART_THEME_LIGHT
}
