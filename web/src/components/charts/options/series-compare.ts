/*
 * Series comparison.
 *
 * Two runs on one chart: the audit page plots a channel of run A against run B and
 * the difference between them. The difference series is drawn against a zero
 * baseline, because "did the change move this metric" is the question the chart
 * answers.
 */
import type { EChartsCoreOption } from 'echarts/core'
import { baseOption, lineSeries, yAxis, type AxisLabels, type Point } from './types'

/** One run of a comparison. */
export interface ComparedSeries {
  /** Series name: the run's label. */
  name: string
  /** Points of this run. */
  data: Point[]
  /** Explicit colour; the theme palette applies when absent. */
  color?: string
}

/** Options of {@link compareOption}. */
export interface CompareOptions extends AxisLabels {
  /** The series to draw; two is the usual count. */
  series: ComparedSeries[]
  /** Chart title. */
  title?: string
  /** Whether to add a third series of `a - b`, drawn against zero. */
  difference?: boolean
  /** Label of the difference series. */
  differenceLabel?: string
}

/** Builds a comparison option. */
export function compareOption(options: CompareOptions): EChartsCoreOption {
  const series = options.series.map((entry) =>
    lineSeries(entry.name, entry.data, { color: entry.color }),
  )
  if (options.difference === true && options.series.length >= 2) {
    const [first, second] = options.series
    if (first !== undefined && second !== undefined) {
      series.push(
        lineSeries(options.differenceLabel ?? 'difference', subtract(first.data, second.data), {
          area: true,
        }),
      )
    }
  }
  return {
    ...baseOption(
      { x: options.x, y: options.y },
      { title: options.title, legend: series.length > 1 },
    ),
    yAxis: yAxis(options.y),
    series,
  }
}

/**
 * Difference of two point series, aligned by x.
 *
 * A point present in one series and missing from the other is skipped rather than
 * read as zero: a gap in one run is not a measured difference, and drawing it as
 * one would invent data.
 */
export function subtract(minuend: readonly Point[], subtrahend: readonly Point[]): Point[] {
  const byX = new Map<number, number>()
  for (const point of subtrahend) {
    byX.set(point.x, point.y)
  }
  const out: Point[] = []
  for (const point of minuend) {
    const other = byX.get(point.x)
    if (other !== undefined) {
      out.push({ x: point.x, y: point.y - other })
    }
  }
  return out
}
