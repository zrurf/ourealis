/*
 * Simulation jobs: submission, state, results and export.
 *
 * The progress transports — SSE, WebSocket, NDJSON — live in `sse.ts`, `ws.ts`
 * and `ndjson.ts`; this module is the request/reply half. Result pages default to
 * a small window because a truth timeline of 100k samples is tens of megabytes of
 * JSON, which is why the doc asks for paging plus a cache rather than one fetch.
 */
import { api, ApiClient, collectPages, pageQuery } from './client'
import type { Page } from './types'
import type { SensorSample, Summary, TruthSample } from '@/types/result'
import type { SimulationRequest, SimulationState, SubmitReply } from '@/types/simulation'

export type {
  JobState,
  MapRef,
  PersonSpec,
  RouteSpec,
  SimulationRequest,
  SimulationSettings,
  SimulationState,
  SubmitReply,
} from '@/types/simulation'
export type {
  MetricSummary,
  SampleCounts,
  SensorSample,
  Summary,
  TruthSample,
} from '@/types/result'

/** Channels the sensor endpoint serves. */
export const SENSOR_CHANNELS = ['gnss', 'accel', 'gyro', 'mag', 'baro'] as const

/** A sensor channel name. */
export type SensorChannel = (typeof SENSOR_CHANNELS)[number]

/** A page of jobs. */
export function listSimulations(
  offset = 0,
  limit = 1_000,
  client: ApiClient = api,
  signal?: AbortSignal,
): Promise<Page<SimulationState>> {
  return client.get<Page<SimulationState>>('simulations', {
    query: pageQuery(offset, limit),
    signal,
  })
}

/** Submits a run; the reply carries the identifier the caller polls or watches. */
export function submitSimulation(
  request: SimulationRequest,
  client: ApiClient = api,
  signal?: AbortSignal,
): Promise<SubmitReply> {
  return client.post<SubmitReply>('simulations', {
    body: JSON.stringify(request),
    signal,
  })
}

/** State of one job. */
export function getSimulation(
  id: string,
  client: ApiClient = api,
  signal?: AbortSignal,
): Promise<SimulationState> {
  return client.get<SimulationState>(`simulations/${encodeURIComponent(id)}`, { signal })
}

/** Cancels a job: a queued one is removed, a running one stops at a stage boundary. */
export function cancelSimulation(
  id: string,
  client: ApiClient = api,
  signal?: AbortSignal,
): Promise<null> {
  return client.delete<null>(`simulations/${encodeURIComponent(id)}`, { signal })
}

/** Manifest, metrics and sample counts of a finished run. */
export function getSummary(
  id: string,
  client: ApiClient = api,
  signal?: AbortSignal,
): Promise<Summary> {
  return client.get<Summary>(`simulations/${encodeURIComponent(id)}/summary`, { signal })
}

/** A page of ground-truth samples. */
export function getTruthPage(
  id: string,
  offset = 0,
  limit = 5_000,
  client: ApiClient = api,
  signal?: AbortSignal,
): Promise<Page<TruthSample>> {
  return client.get<Page<TruthSample>>(`simulations/${encodeURIComponent(id)}/truth`, {
    query: pageQuery(offset, limit),
    signal,
  })
}

/**
 * Every ground-truth sample of a finished run.
 *
 * A whole timeline only fits in memory for the runs the page windows show one at
 * a time; a caller that renders a long run should page instead.
 */
export function getAllTruth(
  id: string,
  client: ApiClient = api,
  signal?: AbortSignal,
): Promise<TruthSample[]> {
  return collectPages((offset, limit) => getTruthPage(id, offset, limit, client, signal))
}

/** A page of one sensor channel. */
export function getSensorPage(
  id: string,
  channel: SensorChannel,
  offset = 0,
  limit = 5_000,
  client: ApiClient = api,
  signal?: AbortSignal,
): Promise<Page<SensorSample>> {
  return client.get<Page<SensorSample>>(
    `simulations/${encodeURIComponent(id)}/sensors/${encodeURIComponent(channel)}`,
    { query: pageQuery(offset, limit), signal },
  )
}

/** Formats the export endpoint accepts. */
export type ExportFormat = 'json' | 'csv' | 'geojson'

/** Exports a finished run as bytes, with the download name the service chose. */
export async function exportSimulation(
  id: string,
  format: ExportFormat = 'json',
  client: ApiClient = api,
  signal?: AbortSignal,
): Promise<{ bytes: Uint8Array; fileName: string | null }> {
  const response = await client.raw(`simulations/${encodeURIComponent(id)}/export`, {
    query: { format },
    signal,
  })
  const header = response.headers.get('content-disposition')
  const match = header === null ? null : /filename="?([^";]+)"?/i.exec(header)
  return {
    bytes: new Uint8Array(await response.arrayBuffer()),
    fileName: match?.[1] ?? null,
  }
}

/** Two runs to compare. */
export interface CompareRequest {
  /** Identifier of the first run. */
  a: string
  /** Identifier of the second run. */
  b: string
}

/** Difference between two runs, as the audit page reports it. */
/** Headline numbers of one run inside a comparison. */
export interface ComparedRun {
  /** Job identifier. */
  id: string
  /** Path ratio: route length over straight-line distance. */
  path_ratio: number
  /** Mean moving speed, m/s. */
  mean_speed_mps: number
  /** Step frequency, Hz. */
  step_frequency_hz: number
  /** Coefficient of variation of the lap times, or `null` for a single-lap run. */
  lap_time_cv: number | null
  /** 95th percentile of the turn rate, rad/s. */
  turn_rate_p95: number
  /** Mean absolute effective curvature, per metre. */
  mean_kappa_eff: number
  /** Route length, metres. */
  route_length_m: number
  /** Duration, seconds. */
  duration_s: number
  /** Moving-speed samples the comparison drew on. */
  speed_samples: number
}

/**
 * `b - a` for every metric two runs share.
 *
 * Declared separately from {@link ComparedRun}: the delta carries no sample count,
 * because the two runs need not have the same number of moving samples.
 */
export interface CompareDelta {
  /** Difference in path ratio. */
  path_ratio: number
  /** Difference in mean moving speed, m/s. */
  mean_speed_mps: number
  /** Difference in step frequency, Hz. */
  step_frequency_hz: number
  /** Difference in lap-time variation, or `null` when a run had a single lap. */
  lap_time_cv: number | null
  /** Difference in the 95th percentile of the turn rate, rad/s. */
  turn_rate_p95: number
  /** Difference in mean absolute effective curvature, per metre. */
  mean_kappa_eff: number
  /** Difference in route length, metres. */
  route_length_m: number
  /** Difference in duration, seconds. */
  duration_s: number
}

/** Two-sample Kolmogorov-Smirnov result of the two speed distributions. */
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

/** Reply of a two-run comparison: the two runs, `b - a` per metric, and the KS test. */
export interface CompareReply {
  /** The baseline run. */
  a: ComparedRun
  /** The run compared against the baseline. */
  b: ComparedRun
  /** `b - a` for every metric the two runs share. */
  delta: CompareDelta
  /** KS test of the moving-speed distributions. */
  speed_ks: SpeedKs
}

/** Compares two finished runs. */
export function compareSimulations(
  request: CompareRequest,
  client: ApiClient = api,
  signal?: AbortSignal,
): Promise<CompareReply> {
  return client.post<CompareReply>('simulations/compare', {
    body: JSON.stringify(request),
    signal,
  })
}
