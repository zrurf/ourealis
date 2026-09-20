/*
 * Scatter chart.
 *
 * Used for the GNSS error cloud and for the step-frequency-against-speed
 * regression, where a fitted line is drawn beside the points. The fit is computed
 * here rather than by echarts so the residual a caller reports matches the line
 * it sees.
 */
import type { EChartsCoreOption } from 'echarts/core'
import {
  baseOption,
  ITEM_TOOLTIP,
  lineSeries,
  scatterSeries,
  type AxisLabels,
  type Point,
} from './types'

/** A least-squares line `y = slope * x + intercept`. */
export interface LinearFit {
  /** Slope of the line. */
  slope: number
  /** Value at `x = 0`. */
  intercept: number
  /** Coefficient of determination, 0 to 1; zero when the samples do not vary. */
  r2: number
}

/**
 * Fits a straight line through points by least squares.
 *
 * Fewer than two points, or a set whose x values do not vary, has no line: the
 * fit reports a zero slope and intercept so a caller draws a flat line at the
 * mean rather than a division by zero.
 */
export function linearFit(points: readonly Point[]): LinearFit {
  const count = points.length
  if (count < 2) {
    return { slope: 0, intercept: count === 1 ? (points[0]?.y ?? 0) : 0, r2: 0 }
  }
  let sumX = 0
  let sumY = 0
  for (const point of points) {
    sumX += point.x
    sumY += point.y
  }
  const meanX = sumX / count
  const meanY = sumY / count
  let covariance = 0
  let varianceX = 0
  let varianceY = 0
  for (const point of points) {
    const dx = point.x - meanX
    const dy = point.y - meanY
    covariance += dx * dy
    varianceX += dx * dx
    varianceY += dy * dy
  }
  if (varianceX === 0) {
    return { slope: 0, intercept: meanY, r2: 0 }
  }
  const slope = covariance / varianceX
  const intercept = meanY - slope * meanX
  const r2 = varianceY === 0 ? 0 : (covariance * covariance) / (varianceX * varianceY)
  return { slope, intercept, r2 }
}

/** Options of {@link scatterOption}. */
export interface ScatterOptions extends AxisLabels {
  /** Points to draw. */
  points: Point[]
  /** Series name, shown in the tooltip. */
  name?: string
  /** Chart title. */
  title?: string
  /** Symbol size in pixels. */
  symbolSize?: number
  /** Whether to draw the least-squares line through the points. */
  fit?: boolean
  /** X extent the fitted line is drawn over; defaults to the point extent. */
  domain?: [number, number]
}

/** Builds a scatter option, optionally with its least-squares line. */
export function scatterOption(options: ScatterOptions): EChartsCoreOption {
  const base = baseOption(
    { x: options.x, y: options.y },
    { title: options.title, tooltip: { ...ITEM_TOOLTIP }, legend: options.fit === true },
  )
  const series: Record<string, unknown>[] = [
    scatterSeries(options.name ?? options.y, options.points, { symbolSize: options.symbolSize }),
  ]
  if (options.fit === true && options.points.length >= 2) {
    const fit = linearFit(options.points)
    const xs = options.domain ?? extentOf(options.points.map((point) => point.x))
    series.push(
      lineSeries(`${options.y} fit`, [
        { x: xs[0], y: fit.slope * xs[0] + fit.intercept },
        { x: xs[1], y: fit.slope * xs[1] + fit.intercept },
      ]),
    )
  }
  return { ...base, series }
}

/** Smallest and largest value of an array; `[0, 0]` when it is empty. */
export function extentOf(values: readonly number[]): [number, number] {
  let min = Number.POSITIVE_INFINITY
  let max = Number.NEGATIVE_INFINITY
  for (const value of values) {
    if (!Number.isFinite(value)) {
      continue
    }
    min = Math.min(min, value)
    max = Math.max(max, value)
  }
  if (min === Number.POSITIVE_INFINITY) {
    return [0, 0]
  }
  return [min, max]
}
