/*
 * Shared pieces of the chart option builders.
 *
 * Every builder is a pure function returning a plain option object, so the unit
 * lane can assert on a builder's output without a canvas and the EChart wrapper
 * stays the only place that knows about echarts itself. Axis and grid defaults
 * live here; the theme supplies the colours.
 */
import type { EChartsCoreOption } from 'echarts/core'

/** One labelled value a chart draws. */
export interface Point {
  /** Position on the x axis, in the unit the axis label names. */
  x: number
  /** Position on the y axis, in the unit the axis label names. */
  y: number
}

/** Axis labels a builder carries into the option. */
export interface AxisLabels {
  /** Label under the x axis. */
  x: string
  /** Label beside the y axis. */
  y: string
}

/** Grid margins that leave room for the axis labels of a dense chart. */
export const DEFAULT_GRID = {
  left: 56,
  right: 24,
  top: 32,
  bottom: 44,
  containLabel: true,
} as const

/** Tooltip settings every builder shares: axis-triggered, so a line reports its own point. */
export const AXIS_TOOLTIP = {
  trigger: 'axis',
  axisPointer: { type: 'line' },
} as const

/** Tooltip settings of a chart whose points do not line up on an x axis. */
export const ITEM_TOOLTIP = {
  trigger: 'item',
} as const

/** A category or value x axis; every builder draws against metres, seconds or hertz. */
export function xAxis(name: string, type: 'value' | 'category' = 'value'): Record<string, unknown> {
  return {
    type,
    name,
    nameGap: 24,
    nameLocation: 'middle',
    scale: true,
    boundaryGap: type === 'category',
  }
}

/** A value y axis. */
export function yAxis(name: string, scale = true): Record<string, unknown> {
  return { type: 'value', name, nameGap: 12, nameLocation: 'end', scale }
}

/** A line series with a smooth-free polyline and no point markers, for a dense signal. */
export function lineSeries(
  name: string,
  data: Point[] | number[],
  options: { color?: string; area?: boolean; width?: number } = {},
): Record<string, unknown> {
  return {
    name,
    type: 'line',
    data,
    showSymbol: false,
    sampling: 'lttb',
    lineStyle: { width: options.width ?? 1.5 },
    ...(options.color === undefined ? {} : { itemStyle: { color: options.color } }),
    ...(options.area === true ? { areaStyle: { opacity: 0.15 } } : {}),
  }
}

/** A bar series; `data` is either values or `[x, y]` pairs. */
export function barSeries(name: string, data: number[] | Point[]): Record<string, unknown> {
  return { name, type: 'bar', data, barMaxWidth: 24 }
}

/** A scatter series. */
export function scatterSeries(
  name: string,
  data: Point[],
  options: { symbolSize?: number } = {},
): Record<string, unknown> {
  return { name, type: 'scatter', data, symbolSize: options.symbolSize ?? 6 }
}

/** An option with only the shared skeleton filled in; a builder adds series on top. */
export function baseOption(
  labels: AxisLabels,
  options: {
    tooltip?: Record<string, unknown>
    legend?: boolean
    title?: string
    /** Whether the x axis is categorical rather than a value axis. */
    categoryAxis?: boolean
  } = {},
): EChartsCoreOption {
  return {
    grid: { ...DEFAULT_GRID },
    tooltip: { ...(options.tooltip ?? AXIS_TOOLTIP) },
    xAxis: xAxis(labels.x, options.categoryAxis === true ? 'category' : 'value'),
    yAxis: yAxis(labels.y),
    ...(options.legend === true ? { legend: { top: 0, right: 0 } } : {}),
    ...(options.title === undefined ? {} : { title: { text: options.title, left: 0, top: 0 } }),
  }
}
