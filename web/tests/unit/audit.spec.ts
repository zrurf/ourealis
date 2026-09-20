/*
 * Metric formatting and derivation.
 *
 * The audit pages label every number with the unit the report uses, so the helpers
 * that do it are checked directly: a metric that lost its unit, a null that
 * printed as `0`, or a distribution row read in the wrong order would all be
 * wrong on the page while the charts still looked plausible.
 */
import { expect, test } from '@playwright/test'
import {
  acfPoints,
  comparedValue,
  compareRows,
  displayLength,
  displaySpeed,
  distributionRows,
  downsample,
  formatMetric,
  isMetricsReport,
  manifestOf,
  metricRows,
  reportOf,
  speedUnit,
  whiteNoiseBand,
  type CompareResult,
  type MetricsReport,
} from '../../src/stores/simulations'

/** A report with every field the DTO declares. */
function report(): MetricsReport {
  return {
    path_ratio: 1.2345,
    length_m: 968.74,
    duration_s: 321.5,
    speed: {
      count: 1900,
      mean: 3.0123,
      std_dev: 0.421,
      min: 0.51,
      p05: 2.2,
      p50: 3.0,
      p95: 3.9,
      max: 4.8,
    },
    turn_rate: {
      count: 1900,
      mean: 0.02,
      std_dev: 0.011,
      min: -0.9,
      p05: -0.04,
      p50: 0.001,
      p95: 0.0812,
      max: 1.4,
    },
    speed_acf: [1, 0.6, 0.3, 0.1],
    position_acf: [1, 0.2, 0.05, 0.01],
    acf_lag_s: 0.005,
    gnss: {
      horizontal: {
        count: 900,
        mean: 2.5,
        std_dev: 1.1,
        min: 0.1,
        p05: 0.9,
        p50: 2.3,
        p95: 4.4,
        max: 9.9,
      },
      vertical: {
        count: 900,
        mean: 3.5,
        std_dev: 1.4,
        min: 0.2,
        p05: 1.1,
        p50: 3.2,
        p95: 5.6,
        max: 12,
      },
      speed: {
        count: 700,
        mean: 0.05,
        std_dev: 0.2,
        min: -0.6,
        p05: -0.3,
        p50: 0.04,
        p95: 0.4,
        max: 0.9,
      },
      availability: 0.98,
    },
    baro_bounce_m: 0.0123,
    accel_spectrum: {
      resolution_hz: 0.05,
      dominant: { frequency_hz: 2.7, magnitude: 0.42 },
      step_peak: { frequency_hz: 2.7, magnitude: 0.42 },
      harmonic_ratios: [0.31, 0.12],
    },
    baro_spectrum: null,
    bounce_consistency: 0.97,
    mean_abs_curvature: 0.01234,
    lap_times: [95.2, 96.1],
    lap_time_cv: 0.0047,
  }
}

/** A comparison reply with both runs and their difference. */
function comparison(): CompareResult {
  const a = {
    id: 'run-a',
    path_ratio: 1.2,
    mean_speed_mps: 3.0,
    step_frequency_hz: 2.7,
    lap_time_cv: 0.01,
    turn_rate_p95: 0.08,
    mean_kappa_eff: 0.012,
    route_length_m: 900,
    duration_s: 300,
    speed_samples: 1800,
  }
  const b = {
    ...a,
    id: 'run-b',
    path_ratio: 1.35,
    mean_speed_mps: 3.2,
    step_frequency_hz: 2.75,
    lap_time_cv: null,
    turn_rate_p95: 0.1,
    route_length_m: 980,
    duration_s: 305,
  }
  return {
    a,
    b,
    delta: {
      path_ratio: b.path_ratio - a.path_ratio,
      mean_speed_mps: b.mean_speed_mps - a.mean_speed_mps,
      step_frequency_hz: b.step_frequency_hz - a.step_frequency_hz,
      lap_time_cv: null,
      turn_rate_p95: b.turn_rate_p95 - a.turn_rate_p95,
      mean_kappa_eff: b.mean_kappa_eff - a.mean_kappa_eff,
      route_length_m: b.route_length_m - a.route_length_m,
      duration_s: b.duration_s - a.duration_s,
    },
    speed_ks: { statistic: 0.21, p_value: 0.0004, samples_a: 1800, samples_b: 1810 },
  }
}

test.describe('formatting', () => {
  test('a value keeps its unit and its precision, a missing one prints as a dash', () => {
    expect(formatMetric(1.2345, '−', 'en', 3)).toBe('1.235')
    expect(formatMetric(968.74, 'm', 'en', 1)).toBe('968.7 m')
    expect(formatMetric(0.01234, '1/m', 'en', 5)).toBe('0.01234 1/m')
    expect(formatMetric(null, 'm/s', 'en')).toBe('—')
    expect(formatMetric(undefined, 'm/s', 'en')).toBe('—')
    expect(formatMetric(Number.NaN, 'm/s', 'en')).toBe('—')
    expect(formatMetric(3.5, '', 'en', 1)).toBe('3.5')
  })

  test('the primary table lists the report’s own fields with their units', () => {
    const rows = metricRows(report())
    expect(rows.map((row) => row.key)).toEqual([
      'pathRatio',
      'length',
      'duration',
      'meanSpeed',
      'speedP95',
      'turnRateP95',
      'meanKappa',
      'bounce',
      'bounceConsistency',
      'lapCv',
    ])
    expect(rows[0]?.unit).toBe('−')
    expect(rows[1]).toMatchObject({ unit: 'm', value: 968.74 })
    expect(rows[5]).toMatchObject({ unit: 'rad/s', value: 0.0812 })
    expect(rows[6]).toMatchObject({ unit: '1/m', value: 0.01234 })
  })

  test('an optional metric the report omitted is a null row, not a zero', () => {
    const rows = metricRows({ ...report(), lap_time_cv: null, bounce_consistency: null })
    expect(rows.find((row) => row.key === 'lapCv')?.value).toBeNull()
    expect(rows.find((row) => row.key === 'bounceConsistency')?.value).toBeNull()
  })

  test('a distribution row set runs from the sample count to the maximum', () => {
    const rows = distributionRows(report().speed, 'm/s', 3)
    expect(rows.map((row) => row.key)).toEqual([
      'count',
      'mean',
      'stdDev',
      'min',
      'p05',
      'p50',
      'p95',
      'max',
    ])
    expect(rows[0]).toMatchObject({ value: 1900, unit: '', digits: 0 })
    expect(rows[7]).toMatchObject({ value: 4.8, unit: 'm/s' })
  })

  test('the comparison table reads as differences, with the two runs beside them', () => {
    const rows = compareRows(comparison())
    expect(rows.map((row) => row.key)).toContain('path_ratio')
    expect(rows.find((row) => row.key === 'path_ratio')?.value).toBeCloseTo(0.15)
    expect(rows.find((row) => row.key === 'lap_time_cv')?.value).toBeNull()

    expect(comparedValue(comparison(), 'path_ratio')).toEqual({ a: 1.2, b: 1.35 })
    expect(comparedValue(comparison(), 'lap_time_cv')).toEqual({ a: 0.01, b: null })
    expect(comparedValue(comparison(), 'not_a_metric')).toEqual({ a: null, b: null })
  })
})

test.describe('derived series', () => {
  test('autocorrelation coefficients become lags in seconds', () => {
    const points = acfPoints([1, 0.5, 0.25], 0.005)
    expect(points).toEqual([
      { x: 0, y: 1 },
      { x: 0.005, y: 0.5 },
      { x: 0.01, y: 0.25 },
    ])
    // A report without a usable lag still draws, one sample per lag.
    expect(acfPoints([1, 0.5], 0).map((point) => point.x)).toEqual([0, 1])
    expect(acfPoints([1, 0.5], Number.NaN).map((point) => point.x)).toEqual([0, 1])
  })

  test('the white-noise band narrows with the sample count', () => {
    expect(whiteNoiseBand(0)).toBe(0)
    expect(whiteNoiseBand(400)).toBeCloseTo(0.098, 3)
    expect(whiteNoiseBand(1600)).toBeLessThan(whiteNoiseBand(400))
  })

  test('a malformed report is not read as a metric report', () => {
    expect(isMetricsReport(report())).toBe(true)
    expect(isMetricsReport({})).toBe(false)
    expect(isMetricsReport({ path_ratio: 1, speed: {}, turn_rate: {}, speed_acf: [] })).toBe(true)
    expect(isMetricsReport({ path_ratio: '1.2', speed: {}, turn_rate: {}, speed_acf: [] })).toBe(
      false,
    )
    expect(isMetricsReport(null)).toBe(false)
  })

  test('a summary without a report yields no report and no manifest', () => {
    expect(reportOf(null)).toBeNull()
    expect(
      reportOf({
        id: 'x',
        route_length_m: 1,
        duration_s: 1,
        samples: { truth: 0, gnss: 0, accel: 0, gyro: 0, mag: 0, baro: 0 },
        backend: 'cpu',
        manifest: {},
        report: null,
      }),
    ).toBeNull()
    expect(
      manifestOf({
        id: 'x',
        route_length_m: 1,
        duration_s: 1,
        samples: { truth: 0, gnss: 0, accel: 0, gyro: 0, mag: 0, baro: 0 },
        backend: 'cpu',
        manifest: { seed: 42, individual: 3 },
        report: null,
      }),
    ).toEqual({ seed: 42, individual: 3 })
    expect(manifestOf(null)).toBeNull()
  })

  test('a long series is strided down and keeps both ends', () => {
    const values = Array.from({ length: 1000 }, (_, index) => index)
    const reduced = downsample(values, 100)
    expect(reduced.length).toBeLessThanOrEqual(101)
    expect(reduced[0]).toBe(0)
    expect(reduced[reduced.length - 1]).toBe(999)
  })
})

test.describe('display units', () => {
  test('metric is the identity and imperial converts without touching the wire value', () => {
    expect(displaySpeed(3.6, 'metric')).toBe(3.6)
    expect(displaySpeed(3.6, 'imperial')).toBeCloseTo(8.053, 3)
    expect(speedUnit('metric')).toBe('m/s')
    expect(speedUnit('imperial')).toBe('mph')

    expect(displayLength(100, 'metric')).toBe(100)
    expect(displayLength(304.8, 'imperial')).toBeCloseTo(1000, 6)
  })
})
