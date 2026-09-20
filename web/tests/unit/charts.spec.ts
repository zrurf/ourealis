/*
 * Chart option builders.
 *
 * Each builder is a pure function, so the assertions are on the option object
 * itself: which series kind it produced, what data it handed over, and whether
 * the reading the chart promises — a confidence band, a zero baseline, a fit — is
 * actually in the option.
 */
import { expect, test } from '@playwright/test'
import { readFileSync } from 'node:fs'
import { acfOption, whiteNoiseBand } from '../../src/components/charts/options/acf'
import { histogram, histogramOption } from '../../src/components/charts/options/histogram'
import { lineOption } from '../../src/components/charts/options/line'
import { extentOf, linearFit, scatterOption } from '../../src/components/charts/options/scatter'
import { compareOption, subtract } from '../../src/components/charts/options/series-compare'
import { spectrumOption } from '../../src/components/charts/options/spectrum'
import { chartTheme, themeNameFor } from '../../src/components/charts/theme'
import {
  CATEGORY_PALETTE,
  SERIES_PALETTE,
  normalize,
  parseCssColor,
  rampAt,
  rgbToHex,
} from '../../src/types/colormap'

/** Reads one nested field of an option, narrowing as it goes. */
function field(option: unknown, key: string): unknown {
  return typeof option === 'object' && option !== null
    ? (option as Record<string, unknown>)[key]
    : undefined
}

/** The series array of an option. */
function series(option: unknown): Array<Record<string, unknown>> {
  const value = field(option, 'series')
  return Array.isArray(value) ? (value as Array<Record<string, unknown>>) : []
}

test.describe('line option', () => {
  test('carries the labels, the series names and the points', () => {
    const option = lineOption({
      x: 'time (s)',
      y: 'speed (m/s)',
      series: [
        { name: 'run A', data: [{ x: 0, y: 3 }] },
        { name: 'run B', data: [{ x: 0, y: 2 }] },
      ],
    })
    expect(field(field(option, 'xAxis'), 'name')).toBe('time (s)')
    expect(field(field(option, 'yAxis'), 'name')).toBe('speed (m/s)')
    expect(series(option).map((entry) => entry.name)).toEqual(['run A', 'run B'])
    expect(series(option)[0]?.type).toBe('line')
    // Two series need a legend; one does not.
    expect(field(option, 'legend')).toBeDefined()
    expect(
      field(lineOption({ x: 'x', y: 'y', series: [{ name: 'a', data: [] }] }), 'legend'),
    ).toBeUndefined()
  })

  test('a categorical axis is marked as one', () => {
    const option = lineOption({ x: 'lag', y: 'acf', series: [], categoryAxis: true })
    expect(field(field(option, 'xAxis'), 'type')).toBe('category')
  })
})

test.describe('histogram', () => {
  test('bins a sample set between the smallest and largest value', () => {
    const bins = histogram([0, 1, 2, 3, 4, 5, 6, 7, 8, 9], 5)
    // The range is divided into equal bins, so the edges follow the width rather
    // than the integers the samples happen to be.
    expect(bins.width).toBeCloseTo(1.8, 10)
    expect(bins.edges[0]).toBe(0)
    expect(bins.edges[4]).toBeCloseTo(7.2, 10)
    expect(bins.counts).toEqual([2, 2, 2, 2, 2])
    expect(bins.upperEdges[0]).toBeCloseTo(1.8, 10)
  })

  test('a constant sample set still produces one bin', () => {
    const bins = histogram([3, 3, 3], 10)
    expect(bins.counts).toEqual([3, 0, 0, 0, 0, 0, 0, 0, 0, 0])
    expect(bins.width).toBe(1)
  })

  test('an empty sample set produces one empty bin rather than a crash', () => {
    expect(histogram([], 4)).toEqual({ edges: [0], upperEdges: [0], counts: [0], width: 0 })
  })

  test('the option labels the axis with the bin edges and can report density', () => {
    const option = histogramOption({ x: 'error (m)', y: 'count', values: [0, 1, 2, 3], bins: 2 })
    const axisData = field(field(option, 'xAxis'), 'data')
    expect(axisData).toEqual(['0.00', '1.50'])
    expect(series(option)[0]?.data).toEqual([2, 2])
    const density = histogramOption({
      x: 'error (m)',
      y: 'density',
      values: [0, 1, 2, 3],
      bins: 2,
      scale: 'density',
    })
    // Two samples in a bin of width 1.5 out of four samples in total.
    expect(series(density)[0]?.data).toEqual([1 / 3, 1 / 3])
    expect(series(density)[0]?.type).toBe('bar')
  })
})

test.describe('scatter option', () => {
  test('fits a line when asked and reports the fit', () => {
    const points = [
      { x: 0, y: 1 },
      { x: 1, y: 3 },
      { x: 2, y: 5 },
    ]
    const fit = linearFit(points)
    expect(fit.slope).toBeCloseTo(2, 10)
    expect(fit.intercept).toBeCloseTo(1, 10)
    expect(fit.r2).toBeCloseTo(1, 10)
  })

  test('a degenerate sample set has no line rather than a division by zero', () => {
    expect(linearFit([])).toEqual({ slope: 0, intercept: 0, r2: 0 })
    expect(linearFit([{ x: 4, y: 7 }])).toEqual({ slope: 0, intercept: 7, r2: 0 })
    expect(
      linearFit([
        { x: 1, y: 1 },
        { x: 1, y: 2 },
      ]),
    ).toEqual({ slope: 0, intercept: 1.5, r2: 0 })
  })

  test('the fit series spans the given domain', () => {
    const option = scatterOption({
      x: 'speed (m/s)',
      y: 'step frequency (Hz)',
      points: [
        { x: 2, y: 2.6 },
        { x: 3, y: 2.7 },
        { x: 4, y: 2.8 },
      ],
      fit: true,
      domain: [1, 5],
    })
    const fitSeries = series(option)[1]
    expect(fitSeries?.type).toBe('line')
    const data = fitSeries?.data
    expect(Array.isArray(data) ? data[0] : null).toEqual({ x: 1, y: expect.any(Number) })
    expect(series(option)[0]?.type).toBe('scatter')
  })

  test('the extent of a point set ignores non-finite values', () => {
    expect(extentOf([3, Number.NaN, -1, Number.POSITIVE_INFINITY])).toEqual([-1, 3])
    expect(extentOf([])).toEqual([0, 0])
  })
})

test.describe('spectrum option', () => {
  test('pairs frequencies with magnitudes and can go logarithmic', () => {
    const option = spectrumOption({
      x: 'frequency (Hz)',
      y: 'density (m²/Hz)',
      frequencies: [0, 1, 2],
      magnitudes: [10, 1, 0.1],
      logMagnitude: true,
    })
    expect(series(option)[0]?.data).toEqual([
      { x: 0, y: 10 },
      { x: 1, y: 1 },
      { x: 2, y: 0.1 },
    ])
    expect(field(field(option, 'yAxis'), 'type')).toBe('log')
    expect(field(field(option, 'xAxis'), 'type')).toBe('value')
  })

  test('a missing magnitude reads as zero rather than shifting the series', () => {
    const option = spectrumOption({
      x: 'f',
      y: 'd',
      frequencies: [0, 1, 2],
      magnitudes: [5, 4],
    })
    expect(series(option)[0]?.data).toEqual([
      { x: 0, y: 5 },
      { x: 1, y: 4 },
      { x: 2, y: 0 },
    ])
  })
})

test.describe('acf option', () => {
  test('draws the confidence band as a mark line', () => {
    const option = acfOption({
      x: 'lag (s)',
      y: 'acf',
      lags: [0, 1, 2],
      values: [1, 0.2, -0.1],
      confidence: whiteNoiseBand(400),
    })
    const markLine = series(option)[0]?.markLine
    expect(field(markLine, 'data')).toEqual([{ yAxis: 0.098 }, { yAxis: -0.098 }])
  })

  test('a sample count of zero has no band', () => {
    expect(whiteNoiseBand(0)).toBe(0)
    const option = acfOption({ x: 'lag', y: 'acf', lags: [0], values: [1] })
    expect(series(option)[0]?.markLine).toBeUndefined()
  })
})

test.describe('series comparison', () => {
  test('the difference is taken against a shared x axis only', () => {
    const difference = subtract(
      [
        { x: 0, y: 3 },
        { x: 1, y: 4 },
        { x: 2, y: 5 },
      ],
      [
        { x: 0, y: 1 },
        { x: 2, y: 1 },
      ],
    )
    expect(difference).toEqual([
      { x: 0, y: 2 },
      { x: 2, y: 4 },
    ])
  })

  test('the option draws both runs and the difference when asked', () => {
    const option = compareOption({
      x: 'time (s)',
      y: 'speed (m/s)',
      series: [
        { name: 'A', data: [{ x: 0, y: 3 }] },
        { name: 'B', data: [{ x: 0, y: 2 }] },
      ],
      difference: true,
      differenceLabel: 'A − B',
    })
    expect(series(option).map((entry) => entry.name)).toEqual(['A', 'B', 'A − B'])
    expect(field(option, 'legend')).toBeDefined()
  })

  test('a comparison of one series has no difference series', () => {
    const option = compareOption({
      x: 'x',
      y: 'y',
      series: [{ name: 'A', data: [] }],
      difference: true,
    })
    expect(series(option)).toHaveLength(1)
  })
})

test.describe('chart theme', () => {
  test('the two appearances differ and the palettes are the data palette', () => {
    const light = chartTheme(false)
    const dark = chartTheme(true)
    expect(light.color).toEqual(dark.color)
    expect(light.color).toEqual([...SERIES_PALETTE])
    expect(light.color).toHaveLength(8)
    expect(light.textStyle.color).not.toBe(dark.textStyle.color)
    expect(themeNameFor(true)).toBe('ourealis-dark')
    expect(themeNameFor(false)).toBe('ourealis-light')
  })

  test('the shared neutrals are the ones the stylesheet carries', () => {
    const css = readFileSync(
      new URL('../../src/styles/theme.css', import.meta.url),
      'utf8',
    ).toLowerCase()
    // A canvas cannot read a CSS variable, so the few colours the interface and the
    // charts both use are literals here and checked against the tokens instead.
    for (const color of [
      rgbToHex([28, 28, 30]),
      rgbToHex([255, 255, 255]),
      rgbToHex([63, 125, 78]),
    ]) {
      expect(css).toContain(color.toLowerCase())
    }
  })

  test('the series palette is eight distinct data colours', () => {
    expect(new Set(SERIES_PALETTE).size).toBe(SERIES_PALETTE.length)
    expect(SERIES_PALETTE).toHaveLength(8)
  })
})

test.describe('colour mapping', () => {
  test('a ramp is clamped at both ends and interpolates between stops', () => {
    expect(rampAt(CATEGORY_PALETTE, -1)).toEqual(CATEGORY_PALETTE[0])
    expect(rampAt(CATEGORY_PALETTE, 2)).toEqual(CATEGORY_PALETTE[CATEGORY_PALETTE.length - 1])
    expect(rampAt(CATEGORY_PALETTE, 0.5)).toHaveLength(3)
  })

  test('a CSS colour is read in every form a token file may use', () => {
    expect(parseCssColor('#ffffff')).toEqual([255, 255, 255])
    expect(parseCssColor(' #fff ')).toEqual([255, 255, 255])
    expect(parseCssColor('#3f7d4e')).toEqual([63, 125, 78])
    expect(parseCssColor('#3f7d4eff')).toEqual([63, 125, 78])
    expect(parseCssColor('rgb(63 125 78)')).toEqual([63, 125, 78])
    expect(parseCssColor('rgba(63, 125, 78, 0.5)')).toEqual([63, 125, 78])
    // Anything else must not read as black, which would look like a rendering bug.
    expect(parseCssColor('var(--nope)')).toEqual([128, 128, 128])
  })

  test('normalising a value saturates and survives a degenerate range', () => {
    expect(normalize(5, 0, 10)).toBe(0.5)
    expect(normalize(-5, 0, 10)).toBe(0)
    expect(normalize(50, 0, 10)).toBe(1)
    expect(normalize(1, 1, 1)).toBe(0.5)
    // A value that is not a number has no position on a ramp; the bottom is the
    // readable answer, since a transparent middle would look like data.
    expect(normalize(Number.NaN, 0, 1)).toBe(0)
  })
})
