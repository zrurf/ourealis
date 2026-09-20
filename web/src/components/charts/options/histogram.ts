/*
 * Histogram.
 *
 * Binning is a pure function of its own so a caller can label the axis with the
 * bin edges it actually drew, and so the unit lane can check the edge cases: a
 * constant sample set, a single sample, and a range that is already normalised.
 */
import type { EChartsCoreOption } from 'echarts/core'
import { barSeries, baseOption, xAxis, yAxis, type AxisLabels } from './types'

/** Bins of a sample set. */
export interface Histogram {
  /** Lower edge of each bin. */
  edges: number[]
  /** Upper edge of each bin. */
  upperEdges: number[]
  /** Count of each bin. */
  counts: number[]
  /** Width of every bin; zero for an empty sample set. */
  width: number
}

/**
 * Bins a sample set.
 *
 * A sample set with no spread still produces one bin, so a histogram of a
 * constant signal draws a single bar rather than an empty chart.
 */
export function histogram(values: readonly number[], binCount = 30): Histogram {
  const finite = values.filter((value) => Number.isFinite(value))
  const bins = Math.max(1, Math.trunc(binCount))
  if (finite.length === 0) {
    return { edges: [0], upperEdges: [0], counts: [0], width: 0 }
  }
  const min = Math.min(...finite)
  const max = Math.max(...finite)
  const width = max - min === 0 ? 1 : (max - min) / bins
  const counts = Array.from({ length: bins }, () => 0)
  for (const value of finite) {
    const slot = Math.min(bins - 1, Math.floor((value - min) / width))
    counts[slot] = (counts[slot] ?? 0) + 1
  }
  const edges = Array.from({ length: bins }, (_, index) => min + index * width)
  return { edges, upperEdges: edges.map((edge) => edge + width), counts, width }
}

/** Options of {@link histogramOption}. */
export interface HistogramOptions extends AxisLabels {
  /** Samples to bin. */
  values: readonly number[]
  /** Number of bins. */
  bins?: number
  /** Chart title. */
  title?: string
  /** Counts or probability mass; `count` is the default. */
  scale?: 'count' | 'density'
  /** Decimals used for the bin labels; defaults to two. */
  decimals?: number
}

/** Builds a histogram option over its own bin edges as category labels. */
export function histogramOption(options: HistogramOptions): EChartsCoreOption {
  const bins = histogram(options.values, options.bins ?? 30)
  const total = bins.counts.reduce((sum, count) => sum + count, 0)
  const decimals = Math.max(0, Math.trunc(options.decimals ?? 2))
  const data =
    options.scale === 'density' && total > 0 && bins.width > 0
      ? bins.counts.map((count) => count / (total * bins.width))
      : bins.counts
  const base = baseOption(
    { x: options.x, y: options.y },
    { title: options.title, tooltip: { trigger: 'item' } },
  )
  return {
    ...base,
    xAxis: {
      ...xAxis(options.x, 'category'),
      data: bins.edges.map((edge) => edge.toFixed(decimals)),
    },
    yAxis: yAxis(options.y, false),
    series: [barSeries(options.y, data)],
  }
}
