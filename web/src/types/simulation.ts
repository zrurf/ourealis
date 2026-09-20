/*
 * Mirror of the service's simulation DTOs.
 *
 * Field names, tags and units follow `crates/service/src/api/dto/simulation.rs`.
 * Both route and map references are internally tagged (`"kind"` / `"mode"`), and
 * every settings block is a sparse override: a field left out keeps the core
 * default, so an unfinished form can be submitted as it stands.
 */
import type { Vec2 } from '@/api/types'

/** Lifecycle state of a job. */
export type JobState = 'queued' | 'running' | 'succeeded' | 'failed' | 'cancelled'

/** True once a job cannot change state again. */
export function isTerminalState(state: JobState): boolean {
  return state === 'succeeded' || state === 'failed' || state === 'cancelled'
}

/**
 * Stage names the service reports, in order.
 *
 * The service's simulator runs its pipeline in one call, so it reports the stages it
 * can observe rather than a per-phase progression: a job is `queued`, then
 * `running`, then one of the terminal stages. `progress` is correspondingly absent
 * while running and the interface shows elapsed time and the log stream instead.
 */
export const PIPELINE_STAGES = ['queued', 'running', 'done', 'failed', 'cancelled'] as const

/** State of one job. */
export interface SimulationState {
  /** Job identifier. */
  id: string
  /** Lifecycle state. */
  state: JobState
  /** Reported stage: one of {@link PIPELINE_STAGES}. */
  stage: string
  /** Fraction of the run completed, 0 to 1, or absent while the service cannot say. */
  progress?: number | null
  /** Wall-clock time the job has been running, seconds, absent while queued. */
  elapsed_s?: number | null
  /** Name given at submission, when one was given. */
  name?: string | null
  /** Map the run used. */
  map_id: string
  /** Planning mode: `standard`, `loop` or `dynamic`. */
  mode: string
  /** Failure message, absent unless the state is `failed`. */
  error?: string | null
  /** Failure classification, absent unless the state is `failed`. */
  error_kind?: string | null
  /** Submission time, RFC 3339. */
  created_at: string
  /** Time the worker started, RFC 3339, absent while queued. */
  started_at?: string | null
  /** Time the job finished, RFC 3339, absent while it runs. */
  finished_at?: string | null
}

/** Reply to a submission. */
export interface SubmitReply {
  /** Identifier of the queued job. */
  id: string
  /** State right after submission, normally `queued`. */
  state: JobState
}

/** Where the map of a run comes from. */
export type MapRef =
  | { kind: 'id'; id: string }
  | { kind: 'synthetic'; spec: SyntheticSpec }
  | { kind: 'inline'; omf_base64: string; name?: string | null }

/** Settings of the synthetic map generator. */
export interface SyntheticSpec {
  /** Shape preset: `default`, `compact` or `wide`. */
  preset?: string
  /** Map width, metres. Ignored when a preset is named. */
  width_m?: number
  /** Map height, metres. */
  height_m?: number
  /** Cell resolution, metres. */
  resolution_m?: number
  /** Chunk side length in cells. */
  chunk_size?: number
  /** Seed of the generator. */
  seed?: number
  /** Include a candidate path library. */
  with_kpath_library?: boolean
}

/** Passing behaviour of a waypoint. */
export type WaypointSemantics =
  | { kind: 'pass' }
  | { kind: 'slow' }
  | { kind: 'dwell'; duration_s: number }

/** One waypoint of a standard route. */
export interface WaypointSpec {
  /** Position in the local plane. */
  position: Vec2
  /** Passing behaviour; absent means `pass`. */
  semantics?: WaypointSemantics
  /** Radius the `slow` behaviour applies over, metres. */
  radius_m?: number
}

/** One checkpoint of a dynamic route. */
export interface CheckpointSpec {
  /** Position the runner is redirected to. */
  position: Vec2
  /** Time the checkpoint takes effect, seconds from the start. */
  issued_at_s: number
}

/** How the route is specified; the tag names the planning mode. */
export type RouteSpec =
  | { mode: 'standard'; start: Vec2; goal: Vec2; waypoints?: WaypointSpec[] }
  | { mode: 'loop'; start: Vec2; reference?: Vec2 | null; laps?: number }
  | { mode: 'dynamic'; start: Vec2; goal: Vec2; checkpoints: CheckpointSpec[] }

/** Pace distribution strategy. */
export type PaceStrategy = 'even' | 'positive_split' | 'negative_split'

/**
 * Field-level overrides of an individual's parameters.
 *
 * Only these names are accepted; the service rejects an unknown one with the list
 * it does take, so the form can be filled from `/presets` instead of from a copy
 * of this type.
 */
export interface PersonOverrides {
  label?: string
  target_speed?: number
  step_frequency?: number
  a_max?: number
  a_lat_max?: number
  look_ahead_m?: number
  beta_logit?: number
  lateral_offset_mean?: number
  lateral_offset_std?: number
  lateral_offset_tau_s?: number
  pace_drift_sigma?: number
  pace_drift_tau_s?: number
  critical_speed_ratio?: number
  fatigue_tau_s?: number
  pace_strategy?: PaceStrategy
  split_amplitude?: number
  k_down?: number
  lean_max_deg?: number
  bounce_amplitude_m?: number
  head_look_ahead_s?: number
  turn_omega_max?: number
}

/** The individual to simulate. */
export interface PersonSpec {
  /** Preset the parameters start from: `jog`, `moderate` or `race`. */
  preset: string
  /** Field-level overrides applied on top of the preset. */
  overrides: PersonOverrides
}

/** Motion mode whose cost weights are used. */
export type MotionMode = 'jog' | 'moderate' | 'race'

/** Compute backend policy. */
export type ComputeBackend = 'auto' | 'cpu' | 'gpu'

/** Roadmap handling of a run. */
export type RoadmapSettings =
  | { kind: 'none' }
  | { kind: 'stored'; batch?: number }
  | { kind: 'generate'; seed: number; spacing_m: number; connect_radius_m: number }

/** Route planning settings. */
export interface RouteSettings {
  epsilon?: number
  single_leg_epsilon?: number
  turn_penalty?: number
  max_expansions?: number
  candidates_k?: number
  penalty_mu?: number
  max_overlap_ratio?: number
  d_attach_m?: number
  corner_radius_m?: number
  smooth?: boolean
  sample_spacing_m?: number
  smoothing_iterations?: number
}

/** Motion generation settings. */
export interface MotionSettings {
  sample_rate_hz?: number
  start_stand_s?: number
  end_stand_s?: number
  maneuvers_enabled?: boolean
  profile_spacing_m?: number
  bounce_beta2?: number
  head_look_ahead_s?: number
  turn_alpha?: number
}

/** Lateral offset settings. */
export interface OffsetSettings {
  mean_m?: number
  std_m?: number
  tau_s?: number
  smoothing_s?: number
  a_lat_max?: number
}

/** Speed limit settings. */
export interface LimitSettings {
  look_ahead_m?: number
  look_ahead_mode?: 'distance_weighted_mean' | 'worst_case'
  a_lat_max?: number
  k_down?: number
  grade_clamp?: number
}

/** Sensor simulation settings. */
export interface SensorSettings {
  gnss_rate_hz?: number
  imu_rate_hz?: number
  mag_rate_hz?: number
  baro_rate_hz?: number
  multipath_enabled?: boolean
  magnetic_disturbance_enabled?: boolean
  jitter_enabled?: boolean
  jitter_sigma_m?: number
  mount?: 'body' | 'head'
  force_deterministic_events?: boolean
  reference_pressure_pa?: number
}

/** Attention gate settings. */
export interface AttentionSettings {
  context: number[]
  clip_min?: number
  clip_max?: number
  w_q?: number[]
  w_k?: number[]
}

/**
 * Simulator settings a request may override.
 *
 * Every field is optional and the service applies only the ones present, so an
 * empty object is a valid `settings` block that changes nothing.
 */
export interface SimulationSettings {
  mode?: MotionMode
  weight_override?: number[]
  attention?: AttentionSettings
  backend?: ComputeBackend
  coarse_enabled?: boolean
  coarse_max_size_m?: number
  roadmap?: RoadmapSettings
  route?: RouteSettings
  motion?: MotionSettings
  offset?: OffsetSettings
  limits?: LimitSettings
  sensors?: SensorSettings
  with_metrics?: boolean
}

/** A simulation request. */
export interface SimulationRequest {
  /** Optional name for the job. */
  name?: string | null
  /** Map to run on, or absent to use the first map in the library. */
  map?: MapRef | null
  /** Route to run. */
  route: RouteSpec
  /** Individual to simulate. */
  person: PersonSpec
  /** Seed of the run's random streams. */
  seed?: number
  /** Index of the individual inside a batch. */
  individual?: number
  /** Simulator settings; every field is optional. */
  settings?: SimulationSettings
}
