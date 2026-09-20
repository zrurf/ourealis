/*
 * Line chart.
 *
 * The workhorse of the result pages: a channel over time, a speed profile over arc
 * length, an error series beside a reference. Each series carries its own name, so
 * a legend and a tooltip read the same labels.
 */
import type { EChartsCoreOption } from 'echarts/core'
import { baseOption, lineSeries, type AxisLabels, type Point } from './types'

/** One named line. */
export interface LineInput {
  /** Series name, shown in the legend and the tooltip. */
  name: string
  /** Points, or bare y values against the index when the x axis is a category. */
  data: Point[] | number[]
  /** Explicit colour; the theme palette applies when absent. */
  color?: string
  /** Fill under the line, for a density-like signal. */
  area?: boolean
}

/** Options of {@link lineOption}. */
export interface LineOptions extends AxisLabels {
  /** Series to draw. */
  series: LineInput[]
  /** Whether to show a legend; a single series needs none. */
  legend?: boolean
  /** Chart title, drawn at the top left. */
  title?: string
  /** Whether the x axis is categorical. */
  categoryAxis?: boolean
}

/** Builds a line chart option. */
export function lineOption(options: LineOptions): EChartsCoreOption {
  const base = baseOption(
    { x: options.x, y: options.y },
    {
      legend: options.legend ?? options.series.length > 1,
      title: options.title,
      categoryAxis: options.categoryAxis,
    },
  )
  return {
    ...base,
    series: options.series.map((series) =>
      lineSeries(series.name, series.data, { color: series.color, area: series.area }),
    ),
  }
}
