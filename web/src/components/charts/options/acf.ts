/*
 * Autocorrelation chart.
 *
 * The noise ACF of a sensor channel, drawn as bars with the confidence band of
 * white noise as a mark line: a coefficient inside the band is indistinguishable
 * from zero, which is the only reading a reader makes from this chart.
 */
import type { EChartsCoreOption } from 'echarts/core'
import { baseOption, barSeries, yAxis, type AxisLabels } from './types'

/** Options of {@link acfOption}. */
export interface AcfOptions extends AxisLabels {
  /** Lag of each coefficient, in samples or seconds. */
  lags: readonly number[]
  /** Autocorrelation coefficients, `-1` to `1`. */
  values: readonly number[]
  /** Chart title. */
  title?: string
  /** Series name, shown in the tooltip. */
  name?: string
  /**
   * Half-width of the white-noise confidence band.
   *
   * For `N` samples the usual band is `1.96 / sqrt(N)`; the caller computes it so
   * an unbiased estimator can pass its own value.
   */
  confidence?: number
}

/** Builds an ACF option: bars over the lags with the confidence band as a mark line. */
export function acfOption(options: AcfOptions): EChartsCoreOption {
  const name = options.name ?? options.y
  const series = barSeries(
    name,
    options.lags.map((lag, index) => ({ x: lag, y: options.values[index] ?? 0 })),
  )
  if (options.confidence !== undefined && options.confidence > 0) {
    series.markLine = confidenceBand(options.confidence)
  }
  return {
    ...baseOption(
      { x: options.x, y: options.y },
      { title: options.title, tooltip: { trigger: 'axis' } },
    ),
    yAxis: yAxis(options.y, false),
    series: [series],
  }
}

/** Upper and lower confidence bounds as a mark line pair. */
function confidenceBand(confidence: number): Record<string, unknown> {
  return {
    silent: true,
    symbol: 'none',
    lineStyle: { type: 'dashed' },
    data: [{ yAxis: confidence }, { yAxis: -confidence }],
  }
}

/** Half-width of the white-noise band for a sample count, or zero for none. */
export function whiteNoiseBand(sampleCount: number, z = 1.96): number {
  return sampleCount > 0 ? z / Math.sqrt(sampleCount) : 0
}
