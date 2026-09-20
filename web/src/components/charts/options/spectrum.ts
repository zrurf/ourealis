/*
 * Spectrum chart.
 *
 * A one-sided power spectral density, which is what the sensor pages read: peaks
 * from the step rate and its harmonics, and the noise floor between them. The
 * x axis is linear by default because a first harmonic is what a reader looks
 * for; a logarithmic one is available for a wideband view.
 */
import type { EChartsCoreOption } from 'echarts/core'
import { baseOption, lineSeries, xAxis, yAxis, type AxisLabels } from './types'

/** Options of {@link spectrumOption}. */
export interface SpectrumOptions extends AxisLabels {
  /** Bin centre frequencies, hertz, ascending. */
  frequencies: readonly number[]
  /** Density or magnitude at each frequency, in the unit the y label names. */
  magnitudes: readonly number[]
  /** Chart title. */
  title?: string
  /** Series name, shown in the tooltip. */
  name?: string
  /** Whether the y axis is logarithmic, which suits a density over decades. */
  logMagnitude?: boolean
  /** Whether the x axis is logarithmic, which suits a wideband view. */
  logFrequency?: boolean
}

/** Builds a spectrum option: a filled line over the bin centres. */
export function spectrumOption(options: SpectrumOptions): EChartsCoreOption {
  const points = options.frequencies.map((frequency, index) => ({
    x: frequency,
    y: options.magnitudes[index] ?? 0,
  }))
  const horizontal = xAxis(options.x)
  const vertical = yAxis(options.y)
  return {
    ...baseOption({ x: options.x, y: options.y }, { title: options.title }),
    xAxis:
      options.logFrequency === true
        ? { ...horizontal, type: 'log', logBase: 10, scale: false }
        : horizontal,
    yAxis:
      options.logMagnitude === true
        ? { ...vertical, type: 'log', logBase: 10, scale: false }
        : vertical,
    series: [lineSeries(options.name ?? options.y, points, { area: true })],
  }
}
