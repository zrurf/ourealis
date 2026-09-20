/*
 * Simulation jobs: the job table, one job's live state, its result pages and the
 * metric helpers the audit pages read.
 *
 * Three concerns live here rather than in the views, because all three are shared
 * between several of them and all three have to be checkable without a browser:
 *
 * - the job state machine: `queued → running → succeeded | failed | cancelled`,
 *   fed by the SSE stream or by a state read, with the elapsed timer that replaces
 *   the progress bar the service deliberately does not report (doc §4.5);
 * - the paging cache: a truth timeline or a sensor channel is read in pages and
 *   kept per channel, because a run holds tens of thousands of samples and the
 *   export needs all of them while the charts only need a downsampled view;
 * - the metric helpers: the names, units and derivations of the service's
 *   `MetricsReport` (`crates/core/src/eval/mod.rs`), so every page labels a number
 *   the same way.
 *
 * The caches are replaced rather than mutated: a page array holds thousands of
 * samples and wrapping those in reactive proxies would cost more than the render
 * it feeds, so the map itself is the reactive value.
 */
import { defineStore } from 'pinia'
import { computed, ref, shallowRef } from 'vue'
import { api } from '@/api/client'
import { isApiError } from '@/api/errors'
import {
  SENSOR_CHANNELS,
  cancelSimulation,
  compareSimulations,
  exportSimulation,
  getSensorPage,
  getSimulation,
  getSummary,
  getTruthPage,
  listSimulations,
  submitSimulation,
  type ExportFormat,
  type SensorChannel,
  type SensorSample,
  type Summary,
  type TruthSample,
} from '@/api/simulations'
import { subscribeJobEvents, type SseSubscription } from '@/api/sse'
import { isJobEvent } from '@/types/events'
import {
  isTerminalState,
  type JobState,
  type SimulationRequest,
  type SimulationState,
} from '@/types/simulation'
import type { JobEvent, LogEvent } from '@/types/events'
import { readStored, writeStored } from '@/utils/storage'
import { useNotificationsStore } from './notifications'
import { i18n } from '@/locales'

/** Samples a result page holds unless the caller asks for another size. */
export const PAGE_SIZE = 5_000

/** Points a chart series is reduced to before it is drawn. */
export const RENDER_POINTS = 4_000

/** How far a one-shot request has come. */
export type LoadStatus = 'idle' | 'loading' | 'ready' | 'failed'

/** Channels the sensor views offer, in the order they are listed. */
export const CHANNELS: readonly SensorChannel[] = SENSOR_CHANNELS

/** One line of the job's log stream, as the live panel shows it. */
export interface LogLine {
  /** Monotonic identifier, used as the list key. */
  id: number
  /** Level: `trace`, `debug`, `info`, `warn` or `error`. */
  level: string
  /** Message, English, exactly as the service wrote it. */
  message: string
  /** Wall-clock time the job had been running, seconds. */
  elapsed_s: number
}

/*
 * Metric report types.
 *
 * Field names and units follow `crates/core/src/eval/mod.rs`; the report travels
 * as `unknown` inside the summary DTO, so it is narrowed by {@link isMetricsReport}
 * rather than cast.
 */

/** Summary of a distribution, as `metrics::DistributionStats` writes it. */
export interface DistributionStats {
  /** Sample count. */
  count: number
  /** Mean, in the distribution's own unit. */
  mean: number
  /** Standard deviation. */
  std_dev: number
  /** Minimum. */
  min: number
  /** Fifth percentile. */
  p05: number
  /** Median. */
  p50: number
  /** Ninety-fifth percentile. */
  p95: number
  /** Maximum. */
  max: number
}

/** One spectral peak, hertz and magnitude. */
export interface PeakSummary {
  /** Frequency, Hz. */
  frequency_hz: number
  /** Amplitude. */
  magnitude: number
}

/** Summary of one spectrum. */
export interface SpectrumSummary {
  /** Frequency resolution, Hz. */
  resolution_hz: number
  /** Strongest peak in the band searched. */
  dominant?: PeakSummary | null
  /** Peak closest to the expected step frequency, when one was known. */
  step_peak?: PeakSummary | null
  /** Ratios of the second and third harmonic to the fundamental. */
  harmonic_ratios: number[]
}

/** GNSS error statistics against the truth positions. */
export interface GnssErrorStats {
  /** Horizontal error distribution, metres. */
  horizontal: DistributionStats
  /** Vertical error distribution, metres. */
  vertical: DistributionStats
  /** Speed error distribution, m/s. */
  speed: DistributionStats
  /** Share of epochs that produced a fix. */
  availability: number
}

/** The complete evaluation report of one run. */
export interface MetricsReport {
  /** Path ratio `L_path / d_euclid`. */
  path_ratio: number
  /** Path length, metres. */
  length_m: number
  /** Duration, seconds. */
  duration_s: number
  /** Speed distribution, m/s. */
  speed: DistributionStats
  /** Turn-rate distribution from the effective curvature, rad/s. */
  turn_rate: DistributionStats
  /** Autocorrelation of the speed residual, lag zero first. */
  speed_acf: number[]
  /** Autocorrelation of the lateral position residual, lag zero first. */
  position_acf: number[]
  /** Time between two consecutive autocorrelation lags, seconds. */
  acf_lag_s: number
  /** GNSS error statistics, when GNSS was simulated. */
  gnss?: GnssErrorStats | null
  /** Vertical bounce amplitude at the step frequency, metres. */
  baro_bounce_m: number
  /** Accelerometer spectrum summary. */
  accel_spectrum?: SpectrumSummary | null
  /** Barometer spectrum summary. */
  baro_spectrum?: SpectrumSummary | null
  /** Ratio of the measured accelerometer fundamental to the predicted one. */
  bounce_consistency?: number | null
  /** Mean absolute curvature, per metre. */
  mean_abs_curvature: number
  /** Lap times of a loop session, seconds. */
  lap_times: number[]
  /** Coefficient of variation of the lap times, when there is more than one. */
  lap_time_cv?: number | null
}

/** The manifest fields the result pages read (`sim::output::RunManifest`). */
export interface RunManifestView {
  /** Crate version that produced the run. */
  generator?: string
  /** Global seed. */
  seed?: number
  /** Individual index inside the batch. */
  individual?: number
  /** Planning mode name. */
  mode?: string
  /** Map name, when the map carries one. */
  map_name?: string | null
  /** Sensor rates actually used: GNSS, IMU, magnetometer, barometer, hertz. */
  rates_hz?: number[]
  /** Compute backend that produced the run. */
  backend?: string
  /** Inertial mount. */
  mount?: string
  /** Recording duration, seconds. */
  duration_s?: number
  /** Requested start point, metres. */
  start?: number[]
  /** Requested goal, metres. */
  goal?: number[]
}

/** One metric of a report, ready for a table cell. */
export interface MetricRow {
  /** Stable key, used as the row key. */
  key: string
  /** Translation key of the label. */
  labelKey: string
  /** Value in the unit `unit` names, or `null` when the report has none. */
  value: number | null
  /** Unit symbol, as the domain uses it (`m`, `m/s`, `rad/s`, `1/m`, `−`). */
  unit: string
  /** Decimals a reader needs for this metric. */
  digits: number
}

/** One run of a comparison, as `api::simulations::ComparedRun` writes it. */
export interface ComparedRun {
  /** Job identifier. */
  id: string
  /** Path ratio. */
  path_ratio: number
  /** Mean moving speed, m/s. */
  mean_speed_mps: number
  /** Step frequency, Hz. */
  step_frequency_hz: number
  /** Coefficient of variation of the lap times, when the run had more than one. */
  lap_time_cv?: number | null
  /** 95th percentile of the turn rate, rad/s. */
  turn_rate_p95: number
  /** Mean absolute effective curvature, per metre. */
  mean_kappa_eff: number
  /** Route length, metres. */
  route_length_m: number
  /** Duration, seconds. */
  duration_s: number
  /** Moving-speed samples the comparison used. */
  speed_samples: number
}

/** `b - a` for every metric the compared runs share. */
export interface CompareDelta {
  /** Difference in path ratio. */
  path_ratio: number
  /** Difference in mean speed, m/s. */
  mean_speed_mps: number
  /** Difference in step frequency, Hz. */
  step_frequency_hz: number
  /** Difference in lap-time coefficient of variation, when both runs have one. */
  lap_time_cv?: number | null
  /** Difference in the 95th percentile of the turn rate, rad/s. */
  turn_rate_p95: number
  /** Difference in mean absolute effective curvature, per metre. */
  mean_kappa_eff: number
  /** Difference in route length, metres. */
  route_length_m: number
  /** Difference in duration, seconds. */
  duration_s: number
}

/** Outcome of the two-sample Kolmogorov–Smirnov test of the speed distributions. */
export interface SpeedKs {
  /** KS statistic: the largest gap between the two empirical distributions. */
  statistic: number
  /** Asymptotic p-value at these sample counts. */
  p_value: number
  /** Samples the first run contributed. */
  samples_a: number
  /** Samples the second run contributed. */
  samples_b: number
}

/**
 * The comparison reply as the service sends it.
 *
 * `api/simulations.ts` types the reply as `{a: string, b: string, metrics}`; the
 * service answers with two `ComparedRun` objects, a `delta` block and `speed_ks`
 * (`crates/service/src/api/simulations.rs`), so the store re-types it and the
 * audit page's `metrics` key is never read.
 */
export interface CompareResult {
  /** The baseline run. */
  a: ComparedRun
  /** The run compared against it. */
  b: ComparedRun
  /** `b - a` per metric. */
  delta: CompareDelta
  /** Two-sample KS test of the moving-speed distributions. */
  speed_ks: SpeedKs
}

/** Formats a metric value with the unit the domain uses. */
export function formatMetric(
  value: number | null | undefined,
  unit: string,
  locale: string,
  digits = 3,
): string {
  if (value === null || value === undefined || !Number.isFinite(value)) {
    return '—'
  }
  const text = new Intl.NumberFormat(locale, { maximumFractionDigits: digits }).format(value)
  return unit === '' || unit === '−' ? text : `${text} ${unit}`
}

/** A row of the primary-metric table, in the order the doc lists them. */
export function metricRows(report: MetricsReport): MetricRow[] {
  return [
    {
      key: 'pathRatio',
      labelKey: 'simulation.audit.pathRatio',
      value: report.path_ratio,
      unit: '−',
      digits: 3,
    },
    {
      key: 'length',
      labelKey: 'simulation.audit.length',
      value: report.length_m,
      unit: 'm',
      digits: 1,
    },
    {
      key: 'duration',
      labelKey: 'simulation.audit.duration',
      value: report.duration_s,
      unit: 's',
      digits: 2,
    },
    {
      key: 'meanSpeed',
      labelKey: 'simulation.audit.meanSpeed',
      value: report.speed.mean,
      unit: 'm/s',
      digits: 3,
    },
    {
      key: 'speedP95',
      labelKey: 'simulation.audit.speedP95',
      value: report.speed.p95,
      unit: 'm/s',
      digits: 3,
    },
    {
      key: 'turnRateP95',
      labelKey: 'simulation.audit.turnRateP95',
      value: report.turn_rate.p95,
      unit: 'rad/s',
      digits: 4,
    },
    {
      key: 'meanKappa',
      labelKey: 'simulation.audit.meanAbsCurvature',
      value: report.mean_abs_curvature,
      unit: '1/m',
      digits: 5,
    },
    {
      key: 'bounce',
      labelKey: 'simulation.audit.baroBounce',
      value: report.baro_bounce_m,
      unit: 'm',
      digits: 4,
    },
    {
      key: 'bounceConsistency',
      labelKey: 'simulation.audit.bounceConsistency',
      value: report.bounce_consistency ?? null,
      unit: '−',
      digits: 3,
    },
    {
      key: 'lapCv',
      labelKey: 'simulation.audit.lapTimeCv',
      value: report.lap_time_cv ?? null,
      unit: '−',
      digits: 4,
    },
  ]
}

/** Rows of a distribution summary, from the sample count to the maximum. */
export function distributionRows(stats: DistributionStats, unit: string, digits = 3): MetricRow[] {
  return [
    { key: 'count', labelKey: 'simulation.audit.count', value: stats.count, unit: '', digits: 0 },
    { key: 'mean', labelKey: 'simulation.audit.mean', value: stats.mean, unit, digits },
    { key: 'stdDev', labelKey: 'simulation.audit.stdDev', value: stats.std_dev, unit, digits },
    { key: 'min', labelKey: 'simulation.audit.min', value: stats.min, unit, digits },
    { key: 'p05', labelKey: 'simulation.audit.p05', value: stats.p05, unit, digits },
    { key: 'p50', labelKey: 'simulation.audit.p50', value: stats.p50, unit, digits },
    { key: 'p95', labelKey: 'simulation.audit.p95', value: stats.p95, unit, digits },
    { key: 'max', labelKey: 'simulation.audit.max', value: stats.max, unit, digits },
  ]
}

/** The comparison table: each metric's difference, keyed by the service's name. */
export function compareRows(result: CompareResult): MetricRow[] {
  return [
    {
      key: 'path_ratio',
      labelKey: 'simulation.audit.pathRatio',
      value: result.delta.path_ratio,
      unit: '−',
      digits: 4,
    },
    {
      key: 'mean_speed_mps',
      labelKey: 'simulation.audit.meanSpeed',
      value: result.delta.mean_speed_mps,
      unit: 'm/s',
      digits: 4,
    },
    {
      key: 'step_frequency_hz',
      labelKey: 'simulation.audit.stepFrequency',
      value: result.delta.step_frequency_hz,
      unit: 'Hz',
      digits: 4,
    },
    {
      key: 'lap_time_cv',
      labelKey: 'simulation.audit.lapTimeCv',
      value: result.delta.lap_time_cv ?? null,
      unit: '−',
      digits: 4,
    },
    {
      key: 'turn_rate_p95',
      labelKey: 'simulation.audit.turnRateP95',
      value: result.delta.turn_rate_p95,
      unit: 'rad/s',
      digits: 4,
    },
    {
      key: 'mean_kappa_eff',
      labelKey: 'simulation.audit.meanAbsCurvature',
      value: result.delta.mean_kappa_eff,
      unit: '1/m',
      digits: 5,
    },
    {
      key: 'route_length_m',
      labelKey: 'simulation.audit.length',
      value: result.delta.route_length_m,
      unit: 'm',
      digits: 2,
    },
    {
      key: 'duration_s',
      labelKey: 'simulation.audit.duration',
      value: result.delta.duration_s,
      unit: 's',
      digits: 2,
    },
  ]
}

/** The two runs' own value of one metric, for the side-by-side columns. */
export function comparedValue(
  result: CompareResult,
  key: string,
): { a: number | null; b: number | null } {
  const read = (run: ComparedRun): number | null => {
    if (key === 'lap_time_cv') {
      return run.lap_time_cv ?? null
    }
    const value = (run as unknown as Record<string, unknown>)[key]
    return typeof value === 'number' ? value : null
  }
  return { a: read(result.a), b: read(result.b) }
}

/** Autocorrelation coefficients with the lag of each, ascending. */
export function acfPoints(
  coefficients: readonly number[],
  lag_s: number,
): Array<{ x: number; y: number }> {
  const step = Number.isFinite(lag_s) && lag_s > 0 ? lag_s : 1
  return coefficients.map((value, index) => ({ x: index * step, y: value }))
}

/** Half-width of the white-noise confidence band for a sample count. */
export function whiteNoiseBand(sampleCount: number, z = 1.96): number {
  return sampleCount > 0 ? z / Math.sqrt(sampleCount) : 0
}

/**
 * Reduces a series to at most `maxPoints` for rendering.
 *
 * The raw pages stay in the cache for the export; a chart draws a strided view of
 * them, keeping the first and the last sample so the drawn extent is still the
 * run's extent rather than wherever the stride happened to stop.
 */
export function downsample<T>(items: readonly T[], maxPoints = RENDER_POINTS): T[] {
  if (maxPoints <= 0) {
    return []
  }
  if (items.length <= maxPoints) {
    return [...items]
  }
  const stride = Math.ceil(items.length / maxPoints)
  const out: T[] = []
  for (let index = 0; index < items.length; index += stride) {
    const item = items[index]
    if (item !== undefined) {
      out.push(item)
    }
  }
  const last = items[items.length - 1]
  if (last !== undefined && out[out.length - 1] !== last) {
    out.push(last)
  }
  return out
}

/** Narrows an untrusted value to a metric report. */
export function isMetricsReport(value: unknown): value is MetricsReport {
  if (typeof value !== 'object' || value === null) {
    return false
  }
  const record = value as Record<string, unknown>
  return (
    typeof record.path_ratio === 'number' &&
    typeof record.speed === 'object' &&
    record.speed !== null &&
    typeof record.turn_rate === 'object' &&
    record.turn_rate !== null &&
    Array.isArray(record.speed_acf)
  )
}

/** The report of a summary, or `null` when the run evaluated none. */
export function reportOf(summary: Summary | null): MetricsReport | null {
  if (summary === null || summary.report === null || summary.report === undefined) {
    return null
  }
  return isMetricsReport(summary.report) ? summary.report : null
}

/** The manifest of a summary, or `null` when the reply carried none. */
export function manifestOf(summary: Summary | null): RunManifestView | null {
  if (summary === null || typeof summary.manifest !== 'object' || summary.manifest === null) {
    return null
  }
  return summary.manifest as RunManifestView
}

/**
 * Reads a run's export.
 *
 * Outside the store because it touches no store state; the caller turns the bytes
 * into a download.
 */
export function exportJob(
  id: string,
  format: ExportFormat,
): Promise<{ bytes: Uint8Array; fileName: string | null }> {
  return exportSimulation(id, format)
}

/** Message of a failure, preferring the service's own text. */
export function messageOf(error: unknown): string {
  return isApiError(error) ? error.message : String(error)
}

/** Units the interface may display a speed in; the wire stays metres per second. */
export type UnitSystem = 'metric' | 'imperial'

/** Storage key of the unit preference. */
export const UNITS_KEY = 'ourealis.units'

/** Metres per second in one kilometre per hour. */
export const MPS_TO_KMH = 3.6

/** Metres in one mile. */
export const METRES_PER_MILE = 1609.344

/** Metres in one foot. */
export const METRES_PER_FOOT = 0.3048

/** The unit preference as it was stored, or `metric` when nothing valid is stored. */
export function readUnitSystem(): UnitSystem {
  return readStored(UNITS_KEY) === 'imperial' ? 'imperial' : 'metric'
}

/**
 * Converts a speed for display.
 *
 * The wire format stays metres per second (doc §6.6); only the label and the
 * number a reader sees change.
 */
export function displaySpeed(mps: number, units: UnitSystem): number {
  return units === 'imperial' ? (mps * MPS_TO_KMH) / (METRES_PER_MILE / 1000) : mps
}

/** Converts a length for display. */
export function displayLength(metres: number, units: UnitSystem): number {
  return units === 'imperial' ? metres / METRES_PER_FOOT : metres
}

/** Symbol of the display unit of a speed. */
export function speedUnit(units: UnitSystem): string {
  return units === 'imperial' ? 'mph' : 'm/s'
}

/** Symbol of the display unit of a length. */
export function lengthUnit(units: UnitSystem): string {
  return units === 'imperial' ? 'ft' : 'm'
}

/** Hands bytes to the browser as a download; a no-op outside a page. */
export function downloadBytes(
  bytes: Uint8Array,
  fileName: string,
  type = 'application/octet-stream',
): void {
  if (typeof document === 'undefined' || typeof URL === 'undefined') {
    return
  }
  const blob = new Blob([bytes as unknown as BlobPart], { type })
  const url = URL.createObjectURL(blob)
  const anchor = document.createElement('a')
  anchor.href = url
  anchor.download = fileName
  document.body.appendChild(anchor)
  anchor.click()
  anchor.remove()
  URL.revokeObjectURL(url)
}

/** State of a job and the results read for it. */
export const useSimulationsStore = defineStore('simulations', () => {
  const jobs = shallowRef<SimulationState[]>([])
  const listStatus = ref<LoadStatus>('idle')
  const listError = ref<string | null>(null)

  const currentId = ref<string | null>(null)
  const current = shallowRef<SimulationState | null>(null)
  const logLines = shallowRef<LogLine[]>([])
  const streamStatus = ref<'idle' | 'connecting' | 'live' | 'closed' | 'failed'>('idle')
  const streamError = ref<string | null>(null)
  const elapsed_s = ref(0)

  const summaries = shallowRef<Map<string, Summary>>(new Map())
  const summaryStatus = ref<LoadStatus>('idle')
  const summaryError = ref<string | null>(null)

  const truthPages = shallowRef<Map<number, TruthSample[]>>(new Map())
  const truthTotal = ref(0)
  const sensorPages = shallowRef<Map<string, Map<number, SensorSample[]>>>(new Map())
  const sensorTotals = shallowRef<Record<string, number>>({})
  const resultStatus = ref<LoadStatus>('idle')
  const resultError = ref<string | null>(null)

  const compareResult = shallowRef<CompareResult | null>(null)
  const compareStatus = ref<LoadStatus>('idle')
  const compareError = ref<string | null>(null)

  /** Unit system the pages display in; the wire format is always metric. */
  const units = ref<UnitSystem>(readUnitSystem())

  let subscription: SseSubscription | null = null
  let nextLogId = 1

  /** Sets and persists the display units. */
  function setUnits(value: UnitSystem): void {
    units.value = value
    writeStored(UNITS_KEY, value)
  }

  /** True while the selected job can still change state. */
  const currentIsLive = computed(
    () => current.value !== null && !isTerminalState(current.value.state),
  )

  /** The summary of the selected job, when it has been read. */
  const currentSummary = computed<Summary | null>(() =>
    currentId.value === null ? null : (summaries.value.get(currentId.value) ?? null),
  )

  /** Jobs still queued or running. */
  const activeJobs = computed(() => jobs.value.filter((job) => !isTerminalState(job.state)))

  /** Reads the job table; a later call replaces it. */
  async function loadJobs(): Promise<void> {
    listStatus.value = 'loading'
    listError.value = null
    try {
      const page = await listSimulations(0, 1_000)
      jobs.value = page.items
      listStatus.value = 'ready'
    } catch (error) {
      listError.value = messageOf(error)
      listStatus.value = 'failed'
    }
  }

  /** Reads one job's state and merges it into the table. */
  async function refreshJob(id: string): Promise<SimulationState | null> {
    try {
      const state = await getSimulation(id)
      mergeJob(state)
      if (currentId.value === id) {
        current.value = state
        setElapsed(state.elapsed_s)
      }
      return state
    } catch (error) {
      streamError.value = messageOf(error)
      return null
    }
  }

  /** Selects a job, dropping the logs and the cached pages of the previous one. */
  function select(id: string): void {
    if (currentId.value !== id) {
      closeStream()
      logLines.value = []
      elapsed_s.value = 0
      clearPages()
    }
    currentId.value = id
    current.value = jobs.value.find((job) => job.id === id) ?? current.value
  }

  /**
   * Submits a run and selects it.
   *
   * The reply carries only the identifier and the queued state; the row is added
   * to the table so a list on another page shows the new job too.
   */
  async function submit(request: SimulationRequest): Promise<string> {
    const reply = await submitSimulation(request)
    const state: SimulationState = {
      id: reply.id,
      state: reply.state,
      stage: 'queued',
      map_id: request.map?.kind === 'id' ? request.map.id : '',
      mode: request.route.mode,
      name: request.name ?? null,
      created_at: new Date().toISOString(),
    }
    jobs.value = [state, ...jobs.value.filter((job) => job.id !== reply.id)]
    select(reply.id)
    openStream(reply.id)
    return reply.id
  }

  /** Cancels a job; a queued one disappears, a running one stops at a stage boundary. */
  async function cancel(id: string): Promise<void> {
    const notifications = useNotificationsStore()
    try {
      await cancelSimulation(id)
      const row = jobs.value.find((job) => job.id === id)
      if (row !== undefined) {
        mergeJob({ ...row, state: 'cancelled' })
      }
      if (currentId.value === id && current.value !== null) {
        current.value = { ...current.value, state: 'cancelled' }
      }
      closeStream()
    } catch (error) {
      notifications.pushError(i18n.global.t('simulation.live.cancelFailed'), error)
      throw error
    }
  }

  /** Reads a finished run's summary and keeps it under its identifier. */
  async function loadSummary(id: string, force = false): Promise<Summary | null> {
    const cached = summaries.value.get(id)
    if (cached !== undefined && !force) {
      return cached
    }
    summaryStatus.value = 'loading'
    summaryError.value = null
    try {
      const summary = await getSummary(id)
      summaries.value = new Map(summaries.value).set(id, summary)
      summaryStatus.value = 'ready'
      return summary
    } catch (error) {
      summaryError.value = messageOf(error)
      summaryStatus.value = 'failed'
      return null
    }
  }

  /** Reads the summary of the selected job. */
  async function loadCurrentSummary(force = false): Promise<Summary | null> {
    const id = currentId.value
    return id === null ? null : loadSummary(id, force)
  }

  /** Applies one event of the job's stream to the state machine. */
  function applyEvent(event: JobEvent): void {
    if (event.type === 'log') {
      appendLog(event)
      setElapsed(event.elapsed_s)
      return
    }
    if (event.type === 'done') {
      const state = event.state === '' ? 'succeeded' : (event.state as JobState)
      updateCurrent({ state, stage: 'done' })
      // The stream is over for this job. It is closed here rather than left to the
      // transport, because a finished job's stream would otherwise be re-opened by
      // the reconnect loop and replay its last event.
      closeStream()
      const id = currentId.value
      if (id !== null) {
        void loadSummary(id, true)
      }
      return
    }
    if (event.type === 'error') {
      updateCurrent({ state: 'failed', error: event.message, error_kind: event.kind })
      closeStream()
      streamStatus.value = 'failed'
      streamError.value = event.message
      return
    }
    setElapsed(event.elapsed_s)
    if (event.type === 'state') {
      updateCurrent({ state: event.state as JobState, stage: event.stage })
      return
    }
    updateCurrent({ stage: event.stage })
  }

  /** Adds one log line, keeping the newest lines last. */
  function appendLog(event: LogEvent): void {
    const line: LogLine = {
      id: nextLogId,
      level: event.level,
      message: event.message,
      elapsed_s: event.elapsed_s,
    }
    nextLogId += 1
    logLines.value = [...logLines.value, line]
  }

  /** Merges fields into the selected job's state and the table row. */
  function updateCurrent(patch: Partial<SimulationState>): void {
    const id = currentId.value
    if (id === null || current.value === null) {
      return
    }
    const next = { ...current.value, ...patch }
    current.value = next
    mergeJob(next)
  }

  /** Replaces one row of the job table, or prepends it when it is new. */
  function mergeJob(state: SimulationState): void {
    const index = jobs.value.findIndex((job) => job.id === state.id)
    if (index < 0) {
      jobs.value = [state, ...jobs.value]
      return
    }
    const copy = [...jobs.value]
    copy[index] = { ...copy[index], ...state }
    jobs.value = copy
  }

  /**
   * Advances the elapsed timer by a second.
   *
   * The service reports no percentage, so the elapsed time is the progress
   * indicator (doc §4.5). A view calls this on an interval while the job runs; an
   * event that carried a larger value wins, so the timer neither runs backwards
   * nor overtakes the service.
   */
  function tick(): void {
    if (currentIsLive.value) {
      elapsed_s.value += 1
    }
  }

  /** Sets the elapsed time from a state read or an event. */
  function setElapsed(seconds: number | null | undefined): void {
    if (seconds !== null && seconds !== undefined && Number.isFinite(seconds)) {
      elapsed_s.value = Math.max(elapsed_s.value, Math.max(0, seconds))
    }
  }

  /**
   * Opens the job's event stream.
   *
   * A stream that fails to open is not fatal: a state read still advances the job,
   * so the failure is recorded rather than thrown.
   */
  function openStream(id: string): void {
    closeStream()
    streamStatus.value = 'connecting'
    streamError.value = null
    subscription = subscribeJobEvents(api.url(`simulations/${encodeURIComponent(id)}/events`), {
      onOpen: () => {
        streamStatus.value = 'live'
      },
      onEvent: (event) => {
        applyEvent(event)
      },
      // The service frames its events as `{"at_ms": …, "event": {…}}`, one wrapper
      // around the payload the transport documents (doc §4.5). A frame that is a
      // wrapped job event is unwrapped here, so the transport stays the frozen
      // module and the wrapper is handled where the service's shape is known.
      onUnknown: (payload) => {
        const wrapped = unwrapEvent(payload)
        if (wrapped !== null) {
          applyEvent(wrapped)
        }
      },
      onRetry: () => {
        streamStatus.value = 'connecting'
      },
      onError: (error) => {
        streamStatus.value = 'failed'
        streamError.value = error.message
      },
      onClose: () => {
        if (streamStatus.value !== 'failed') {
          streamStatus.value = 'closed'
        }
      },
    })
  }

  /** Closes the stream; a later event is never delivered. */
  function closeStream(): void {
    subscription?.close()
    subscription = null
    if (streamStatus.value === 'live' || streamStatus.value === 'connecting') {
      streamStatus.value = 'closed'
    }
  }

  /*
   * Result pages.
   *
   * A timeline is read in {@link PAGE_SIZE} windows and cached under the offset it
   * started at, so a page already read is never fetched twice and the export can
   * walk the cache instead of the network.
   */

  /** Reads the pages covering `[offset, offset + limit)` of the truth timeline. */
  async function loadTruthWindow(offset = 0, limit = PAGE_SIZE): Promise<void> {
    await loadWindow(offset, limit, {
      cached: (page) => truthPages.value.has(page),
      fetch: async (page, size, id) => {
        const reply = await getTruthPage(id, page, size)
        if (currentId.value !== id) {
          return []
        }
        truthTotal.value = reply.total
        return storeTruthPage(page, reply.items)
      },
    })
  }

  /** Reads every truth page the run has. */
  async function loadAllTruth(): Promise<TruthSample[]> {
    await loadWindow(0, truthTotal.value || Number.MAX_SAFE_INTEGER, {
      cached: (page) => truthPages.value.has(page),
      fetch: async (page, size, id) => {
        const reply = await getTruthPage(id, page, size)
        if (currentId.value !== id) {
          return []
        }
        truthTotal.value = reply.total
        return storeTruthPage(page, reply.items)
      },
    })
    return truthSamples()
  }

  /** Every cached truth sample, ordered by page offset. */
  function truthSamples(): TruthSample[] {
    return flattenPages(truthPages.value)
  }

  /** Reads the pages covering a window of one sensor channel. */
  async function loadSensorWindow(
    channel: SensorChannel,
    offset = 0,
    limit = PAGE_SIZE,
  ): Promise<void> {
    await loadWindow(offset, limit, {
      cached: (page) => channelPages(channel).has(page),
      fetch: async (page, size, id) => {
        const reply = await getSensorPage(id, channel, page, size)
        if (currentId.value !== id) {
          return []
        }
        sensorTotals.value = { ...sensorTotals.value, [channel]: reply.total }
        return storeSensorPage(channel, page, reply.items)
      },
    })
  }

  /** Reads every page of one sensor channel. */
  async function loadAllSensor(channel: SensorChannel): Promise<SensorSample[]> {
    await loadWindow(0, sensorTotals.value[channel] ?? Number.MAX_SAFE_INTEGER, {
      cached: (page) => channelPages(channel).has(page),
      fetch: async (page, size, id) => {
        const reply = await getSensorPage(id, channel, page, size)
        if (currentId.value !== id) {
          return []
        }
        sensorTotals.value = { ...sensorTotals.value, [channel]: reply.total }
        return storeSensorPage(channel, page, reply.items)
      },
    })
    return sensorSamples(channel)
  }

  /** Every cached sample of one channel, ordered by page offset. */
  function sensorSamples(channel: SensorChannel): SensorSample[] {
    return flattenPages(channelPages(channel))
  }

  /** Total number of samples the service reported for a channel. */
  function totalOf(channel: SensorChannel): number {
    return sensorTotals.value[channel] ?? 0
  }

  /** Pages of one channel, read out of the per-channel map. */
  function channelPages(channel: SensorChannel): Map<number, SensorSample[]> {
    return sensorPages.value.get(channel) ?? new Map()
  }

  /** Replaces one truth page, leaving the others in place. */
  function storeTruthPage(offset: number, items: TruthSample[]): TruthSample[] {
    truthPages.value = new Map(truthPages.value).set(offset, items)
    return items
  }

  /** Replaces one sensor page, leaving the other channels in place. */
  function storeSensorPage(
    channel: SensorChannel,
    offset: number,
    items: SensorSample[],
  ): SensorSample[] {
    const outer = new Map(sensorPages.value)
    const inner = new Map(outer.get(channel) ?? [])
    inner.set(offset, items)
    outer.set(channel, inner)
    sensorPages.value = outer
    return items
  }

  /**
   * Reads a window as whole pages, skipping the ones already cached.
   *
   * The window is rounded out to page boundaries so a scroll that asks for a
   * partly-cached window only pays for the page that is missing.
   */
  async function loadWindow(
    offset: number,
    limit: number,
    reader: {
      cached: (page: number) => boolean
      fetch: (page: number, size: number, id: string) => Promise<unknown[]>
    },
  ): Promise<void> {
    const id = currentId.value
    if (id === null) {
      return
    }
    const start = Math.max(0, Math.floor(offset / PAGE_SIZE) * PAGE_SIZE)
    const end = Math.max(start + PAGE_SIZE, offset + limit)
    resultStatus.value = 'loading'
    resultError.value = null
    try {
      for (let page = start; page < end; page += PAGE_SIZE) {
        if (reader.cached(page)) {
          continue
        }
        // The selection can move while this walk is in flight. Every request carries
        // the job it was started for — reading `currentId` at fetch time would pull
        // the *new* job's pages into the old job's offsets — and a walk whose job is
        // no longer selected stops rather than writing into the new job's cache.
        if (currentId.value !== id) {
          return
        }
        // Pages are read one after another: the cache decides which ones are
        // missing, and the next request only depends on what came back.
        // oxlint-disable-next-line no-await-in-loop
        const items = await reader.fetch(page, PAGE_SIZE, id)
        if (currentId.value !== id) {
          return
        }
        if (items.length < PAGE_SIZE) {
          break
        }
      }
      resultStatus.value = 'ready'
    } catch (error) {
      resultError.value = messageOf(error)
      resultStatus.value = 'failed'
    }
  }

  /** Drops every cached result page of the selected job. */
  function clearPages(): void {
    truthPages.value = new Map()
    truthTotal.value = 0
    sensorPages.value = new Map()
    sensorTotals.value = {}
    resultStatus.value = 'idle'
    resultError.value = null
  }

  /**
   * Compares two finished runs.
   *
   * The frozen client types this reply as `{a: string, b: string, metrics}`, which
   * is not what the service sends; the cast goes through `unknown` so the mismatch
   * is visible here and reported rather than hidden behind a wrong type.
   */
  async function compare(a: string, b: string): Promise<CompareResult | null> {
    compareStatus.value = 'loading'
    compareError.value = null
    try {
      const reply = (await compareSimulations({ a, b })) as unknown as CompareResult
      compareResult.value = reply
      compareStatus.value = 'ready'
      return reply
    } catch (error) {
      compareError.value = messageOf(error)
      compareStatus.value = 'failed'
      return null
    }
  }

  return {
    jobs,
    listStatus,
    listError,
    currentId,
    current,
    currentIsLive,
    currentSummary,
    activeJobs,
    logLines,
    streamStatus,
    streamError,
    elapsed_s,
    summaries,
    summaryStatus,
    summaryError,
    truthPages,
    truthTotal,
    sensorPages,
    sensorTotals,
    resultStatus,
    resultError,
    compareResult,
    compareStatus,
    compareError,
    units,
    setUnits,
    loadJobs,
    refreshJob,
    select,
    submit,
    cancel,
    loadSummary,
    loadCurrentSummary,
    applyEvent,
    tick,
    setElapsed,
    openStream,
    closeStream,
    loadTruthWindow,
    loadAllTruth,
    truthSamples,
    loadSensorWindow,
    loadAllSensor,
    sensorSamples,
    totalOf,
    clearPages,
    compare,
    exportJob,
  }
})

/** A job event carried inside the service's `{at_ms, event}` envelope, or null. */
export function unwrapEvent(payload: unknown): JobEvent | null {
  if (isJobEvent(payload)) {
    return payload
  }
  if (typeof payload !== 'object' || payload === null) {
    return null
  }
  const inner = (payload as Record<string, unknown>)['event']
  return isJobEvent(inner) ? inner : null
}

/** Concatenates the cached pages in offset order. */
function flattenPages<T>(pages: Map<number, T[]>): T[] {
  // A fresh array is sorted, so nothing outside this function is reordered.
  // oxlint-disable-next-line unicorn/no-array-sort
  const offsets = [...pages.keys()].sort((a, b) => a - b)
  const out: T[] = []
  for (const offset of offsets) {
    out.push(...(pages.get(offset) ?? []))
  }
  return out
}
