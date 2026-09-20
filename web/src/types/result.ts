/*
 * Mirror of the service's result DTOs: samples, counts, summaries and previews.
 *
 * Field names and units follow `crates/service/src/api/dto/result.rs`. The
 * trajectory renderer reads {@link TruthSampleDto} directly, so the field names
 * here are also the ones `src/render/trajectory.ts` documents as its seam.
 */
import type { Vec2 } from '@/api/types'

/** One ground-truth sample. */
export interface TruthSample {
  /** Time since the start of the recording, seconds. */
  time_s: number
  /** Reported position including jitter and bounce, metres. */
  position: Vec2
  /** Low-frequency centre-of-mass position, metres. */
  position_low: Vec2
  /** Altitude including bounce, metres. */
  z: number
  /** Terrain elevation, metres. */
  terrain_z: number
  /** Speed along the path, m/s. */
  speed: number
  /** Body heading, radians. */
  heading_rad: number
  /** Head heading, radians. */
  head_heading_rad: number
  /** Pitch, radians. */
  pitch_rad: number
  /** Roll, radians. */
  roll_rad: number
  /** Effective curvature, per metre. */
  kappa_eff: number
  /** Lateral offset from the centre line, metres. */
  offset_m: number
  /** Terrain grade along the direction of travel, rise over run. */
  grade: number
  /** True while standing. */
  standing: boolean
  /** True while turning on the spot. */
  turning: boolean
  /** Low-frequency velocity in the world frame, m/s. */
  velocity: [number, number, number]
  /** Low-frequency acceleration in the world frame, m/s^2. */
  acceleration: [number, number, number]
}

/** One sample of a sensor stream. */
export interface SensorSample {
  /** Time since the start of the recording, seconds. */
  time_s: number
  /** Channel name: `gnss`, `accel`, `gyro`, `mag` or `baro`. */
  channel: string
  /** Vector payload: position for GNSS, body-frame acceleration, angular rate or magnetic field. */
  v?: [number, number, number] | null
  /** Latitude, degrees, GNSS only. */
  latitude_deg?: number | null
  /** Longitude, degrees, GNSS only. */
  longitude_deg?: number | null
  /** Altitude, metres, GNSS only. */
  altitude_m?: number | null
  /** Ground speed, m/s, GNSS only. */
  speed_mps?: number | null
  /** Course over ground, radians, GNSS only. */
  heading_rad?: number | null
  /** Whether the fix survived the dropout model, GNSS only. */
  valid?: boolean | null
  /** Satellite count, GNSS only. */
  satellites?: number | null
  /** Pressure, pascals, barometer only. */
  pressure_pa?: number | null
}

/** Sample counts of one run. */
export interface SampleCounts {
  /** Ground-truth samples. */
  truth: number
  /** GNSS fixes. */
  gnss: number
  /** Accelerometer samples. */
  accel: number
  /** Gyroscope samples. */
  gyro: number
  /** Magnetometer samples. */
  mag: number
  /** Barometer samples. */
  baro: number
}

/** The headline numbers of a run's evaluation report. */
export interface MetricSummary {
  /** Path ratio: route length over straight-line distance. */
  path_ratio?: number | null
  /** Mean speed over the running sections, m/s. */
  mean_speed_mps?: number | null
  /** Step frequency, Hz. */
  step_frequency_hz?: number | null
  /** Coefficient of variation of the lap times, dimensionless. */
  lap_time_cv?: number | null
  /** 95th percentile of the turn rate, rad/s. */
  turn_rate_p95?: number | null
  /** Mean absolute effective curvature, per metre. */
  mean_kappa_eff?: number | null
}

/** Summary of a finished run. */
export interface Summary {
  /** Job identifier. */
  id: string
  /** Route length, metres. */
  route_length_m: number
  /** Duration of the recording, seconds. */
  duration_s: number
  /** Sample counts per stream. */
  samples: SampleCounts
  /** Backend the run resolved to. */
  backend: string
  /** Headline metrics, absent when the run did not evaluate any. */
  metrics?: MetricSummary | null
  /** Full manifest as recorded by the simulator. */
  manifest: unknown
  /** Full evaluation report, absent when the run did not evaluate one. */
  report?: unknown
}

/** One route candidate of a preview. */
export interface RoutePreviewCandidate {
  /** Route length, metres. */
  length_m: number
  /** Route cost in equivalent metres. */
  cost_equiv_m: number
  /** Probability the Logit choice model gives this candidate. */
  probability: number
  /** Path size factor, penalising overlap between candidates. */
  path_size: number
  /** Whether this candidate came from the map's stored library. */
  from_library: boolean
  /** Polyline of the candidate. */
  points: Vec2[]
}

/** A route preview: planning only, before any motion or sensor work. */
export interface RoutePreview {
  /** Candidate routes, best first. */
  candidates: RoutePreviewCandidate[]
  /** Index of the candidate the Logit draw selected. */
  chosen: number
  /** Total length of the chosen route, metres. */
  length_m: number
  /** Cost of the chosen route in equivalent metres. */
  cost_equiv_m: number
  /** Straight-line distance from start to goal, metres. */
  straight_line_m: number
  /** Time the planning step took, milliseconds. */
  planning_ms: number
  /** The smoothed path actually used for motion, when smoothing ran. */
  path: Vec2[]
  /** Speed limit along the smoothed path, m/s, sampled at the profile grid. */
  speed_limit_mps: number[]
  /** Arc length of each speed limit sample, metres. */
  speed_limit_s: number[]
}
