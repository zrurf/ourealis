/*
 * The simulation form's model: its state, its validation and the request it
 * assembles.
 *
 * Pure by construction — no Vue, no DOM, no store — because this is the part that
 * has to be right about the wire format: `SimulationRequest` denies unknown fields
 * (`crates/service/src/api/dto/simulation.rs`), so a stray key or a `null` where
 * the service wants a number is a 400 rather than a wrong result. The unit lane
 * checks the assembled object against the exact JSON shape the service accepts.
 *
 * A draft field holds `null` for "leave it to the service": an absent key in the
 * request is what keeps the simulator's own default in effect, which is what the
 * form promises the user.
 */
import type { Vec2 } from '@/api/types'
import type {
  CheckpointSpec,
  PersonOverrides,
  RouteSpec,
  SensorSettings,
  SimulationRequest,
  SimulationSettings,
  WaypointSemantics,
  WaypointSpec,
} from '@/types/simulation'

/**
 * A field the map surface can fill.
 *
 * The route form reports the target and the view owning the canvas places the
 * point, which is what keeps the form usable with and without a map.
 */
export type PickTarget =
  | { kind: 'start' }
  | { kind: 'goal' }
  | { kind: 'reference' }
  | { kind: 'waypoint'; index: number }
  | { kind: 'checkpoint'; index: number }

/** A point the form can still be missing. */
export interface PointDraft {
  /** Easting, metres, or `null` while the field is empty. */
  x: number | null
  /** Northing, metres, or `null` while the field is empty. */
  y: number | null
}

/** Passing behaviour of one waypoint draft. */
export type SemanticsKind = 'pass' | 'slow' | 'dwell'

/** One waypoint of the standard route. */
export interface WaypointDraft {
  /** Position in the local plane. */
  position: PointDraft
  /** Passing behaviour. */
  semantics: SemanticsKind
  /** Standing time of a `dwell` waypoint, seconds. */
  duration_s: number
  /** Radius the `slow` behaviour applies over, metres. */
  radius_m: number
}

/** One checkpoint of the dynamic route. */
export interface CheckpointDraft {
  /** Position the runner is redirected to. */
  position: PointDraft
  /** Time the checkpoint takes effect, seconds from the start. */
  issued_at_s: number
}

/** A sensor setting the form may leave to the service. */
export interface SensorDraft {
  /** GNSS fix rate, Hz. */
  gnss_rate_hz: number | null
  /** Inertial rate, Hz. */
  imu_rate_hz: number | null
  /** Magnetometer rate, Hz. */
  mag_rate_hz: number | null
  /** Barometer rate, Hz. */
  baro_rate_hz: number | null
  /** Whether multipath events fire. */
  multipath_enabled: boolean | null
  /** Whether magnetic disturbances fire. */
  magnetic_disturbance_enabled: boolean | null
  /** Whether the position jitter layer runs. */
  jitter_enabled: boolean | null
  /** Jitter standard deviation, metres. */
  jitter_sigma_m: number | null
  /** Where the inertial unit is carried. */
  mount: 'body' | 'head' | null
  /** Force the deterministic event mode. */
  force_deterministic_events: boolean | null
  /** Pressure at mean sea level, pascals. */
  reference_pressure_pa: number | null
}

/** A route point the map can place or retag, as the context menu names it. */
export interface RoutePointRef {
  /** Field the point belongs to. */
  kind: 'start' | 'goal' | 'reference' | 'waypoint' | 'checkpoint'
  /** Index inside `waypoints` or `checkpoints`; `-1` appends a waypoint. */
  index: number
}

/** Values a person override may carry: a number, a choice or a label. */
export type OverrideValue = number | string | null

/** Everything the form holds. */
export interface SimulationFormState {
  /** Optional name of the job. */
  name: string
  /** Map to run on, or `null` for the first map in the library. */
  mapId: string | null
  /** Route mode; the tagged union is built from it. */
  mode: 'standard' | 'loop' | 'dynamic'
  /** Start of every mode. */
  start: PointDraft
  /** Goal of the standard and dynamic modes. */
  goal: PointDraft
  /** Ordered waypoints of the standard mode. */
  waypoints: WaypointDraft[]
  /** Reference point of the loop mode. */
  reference: PointDraft
  /** Laps of the loop mode. */
  laps: number
  /** Checkpoints of the dynamic mode. */
  checkpoints: CheckpointDraft[]
  /** Preset the individual starts from. */
  preset: string
  /** Field-level overrides, keyed by the names `/presets` reports. */
  overrides: Record<string, OverrideValue>
  /** Seed of the run's random streams. */
  seed: number
  /** Index of the individual inside a batch. */
  individual: number
  /** Sensor settings. */
  sensors: SensorDraft
  /** Whether the run evaluates the metrics report. */
  withMetrics: boolean
  /** Compute backend, or `null` for the service's own policy. */
  backend: 'auto' | 'cpu' | 'gpu' | null
  /** Motion mode whose cost weights are used, or `null` for the map's prior. */
  motionMode: 'jog' | 'moderate' | 'race' | null
  /** Whether the planner smooths the path. */
  smooth: boolean
}

/** One validation failure of the form. */
export interface FormIssue {
  /** Dotted path of the field, for highlighting and tests. */
  field: string
  /** Catalog key of the message (`simulation.route.startEqualsGoal`, …). */
  key: string
}

/** An empty point. */
export function emptyPoint(): PointDraft {
  return { x: null, y: null }
}

/** An empty sensor block: every field left to the service. */
export function emptySensorDraft(): SensorDraft {
  return {
    gnss_rate_hz: null,
    imu_rate_hz: null,
    mag_rate_hz: null,
    baro_rate_hz: null,
    multipath_enabled: null,
    magnetic_disturbance_enabled: null,
    jitter_enabled: null,
    jitter_sigma_m: null,
    mount: null,
    force_deterministic_events: null,
    reference_pressure_pa: null,
  }
}

/** The form a fresh page starts from. */
export function defaultFormState(): SimulationFormState {
  return {
    name: '',
    mapId: null,
    mode: 'standard',
    start: emptyPoint(),
    goal: emptyPoint(),
    waypoints: [],
    reference: emptyPoint(),
    laps: 1,
    checkpoints: [],
    preset: 'moderate',
    overrides: {},
    seed: 12345,
    individual: 0,
    sensors: emptySensorDraft(),
    withMetrics: true,
    backend: null,
    motionMode: null,
    smooth: true,
  }
}

/** A waypoint with the defaults the service applies when fields are absent. */
export function newWaypoint(position: PointDraft = emptyPoint()): WaypointDraft {
  return { position, semantics: 'pass', duration_s: 10, radius_m: 5 }
}

/** A checkpoint at time zero, which is what a fresh row means. */
export function newCheckpoint(position: PointDraft = emptyPoint()): CheckpointDraft {
  return { position, issued_at_s: 0 }
}

/** True when a point carries both coordinates. */
export function isCompletePoint(point: PointDraft): boolean {
  return (
    point.x !== null && point.y !== null && Number.isFinite(point.x) && Number.isFinite(point.y)
  )
}

/** A complete point as the wire type, or `null` while one coordinate is missing. */
export function pointToVec(point: PointDraft): Vec2 | null {
  return isCompletePoint(point) ? { x: point.x as number, y: point.y as number } : null
}

/** The wire semantics of a waypoint draft. */
export function waypointSemantics(draft: WaypointDraft): WaypointSemantics {
  switch (draft.semantics) {
    case 'pass':
      return { kind: 'pass' }
    case 'slow':
      return { kind: 'slow' }
    case 'dwell':
      return { kind: 'dwell', duration_s: draft.duration_s }
  }
}

/** The route specification of a form state, or `null` when a required point is missing. */
export function routeSpecOf(state: SimulationFormState): RouteSpec | null {
  const start = pointToVec(state.start)
  if (start === null) {
    return null
  }
  switch (state.mode) {
    case 'standard': {
      const goal = pointToVec(state.goal)
      if (goal === null) {
        return null
      }
      const waypoints: WaypointSpec[] = []
      for (const draft of state.waypoints) {
        const position = pointToVec(draft.position)
        if (position === null) {
          continue
        }
        waypoints.push({
          position,
          semantics: waypointSemantics(draft),
          radius_m: draft.radius_m,
        })
      }
      return { mode: 'standard', start, goal, waypoints }
    }
    case 'loop': {
      const reference = pointToVec(state.reference)
      return {
        mode: 'loop',
        start,
        reference,
        laps: Math.max(1, Math.trunc(state.laps)),
      }
    }
    case 'dynamic': {
      const goal = pointToVec(state.goal)
      if (goal === null) {
        return null
      }
      const checkpoints: CheckpointSpec[] = []
      for (const draft of state.checkpoints) {
        const position = pointToVec(draft.position)
        if (position === null) {
          continue
        }
        checkpoints.push({ position, issued_at_s: Math.max(0, draft.issued_at_s) })
      }
      return { mode: 'dynamic', start, goal, checkpoints }
    }
  }
}

/** The overrides of the form, with the unset ones left out. */
export function overridesOf(state: SimulationFormState): PersonOverrides {
  const out: Record<string, number | string> = {}
  for (const [key, value] of Object.entries(state.overrides)) {
    if (value === null) {
      continue
    }
    if (typeof value === 'number') {
      if (Number.isFinite(value)) {
        out[key] = value
      }
      continue
    }
    if (value.trim() !== '') {
      out[key] = value
    }
  }
  return out as PersonOverrides
}

/** The sensor block, with the unset fields left out. */
export function sensorSettingsOf(draft: SensorDraft): SensorSettings {
  const out: SensorSettings = {}
  if (isRate(draft.gnss_rate_hz)) {
    out.gnss_rate_hz = draft.gnss_rate_hz
  }
  if (isRate(draft.imu_rate_hz)) {
    out.imu_rate_hz = draft.imu_rate_hz
  }
  if (isRate(draft.mag_rate_hz)) {
    out.mag_rate_hz = draft.mag_rate_hz
  }
  if (isRate(draft.baro_rate_hz)) {
    out.baro_rate_hz = draft.baro_rate_hz
  }
  if (draft.multipath_enabled !== null) {
    out.multipath_enabled = draft.multipath_enabled
  }
  if (draft.magnetic_disturbance_enabled !== null) {
    out.magnetic_disturbance_enabled = draft.magnetic_disturbance_enabled
  }
  if (draft.jitter_enabled !== null) {
    out.jitter_enabled = draft.jitter_enabled
  }
  if (isRate(draft.jitter_sigma_m)) {
    out.jitter_sigma_m = draft.jitter_sigma_m
  }
  if (draft.mount !== null) {
    out.mount = draft.mount
  }
  if (draft.force_deterministic_events !== null) {
    out.force_deterministic_events = draft.force_deterministic_events
  }
  if (isRate(draft.reference_pressure_pa)) {
    out.reference_pressure_pa = draft.reference_pressure_pa
  }
  return out
}

/** True when a nullable number field carries a usable value. */
function isRate(value: number | null): value is number {
  return value !== null && Number.isFinite(value)
}

/** The settings block of a form state. */
export function settingsOf(state: SimulationFormState): SimulationSettings {
  const settings: SimulationSettings = {
    sensors: sensorSettingsOf(state.sensors),
    with_metrics: state.withMetrics,
    route: { smooth: state.smooth },
  }
  if (state.backend !== null) {
    settings.backend = state.backend
  }
  if (state.motionMode !== null) {
    settings.mode = state.motionMode
  }
  return settings
}

/**
 * Assembles the request the service accepts.
 *
 * `route` is required by the schema, so a caller has to validate first; this
 * function throws rather than sending a request that would come back as a 400.
 */
export function buildSimulationRequest(state: SimulationFormState): SimulationRequest {
  const route = routeSpecOf(state)
  if (route === null) {
    throw new Error('the route is incomplete: a start, a goal or a checkpoint is missing')
  }
  const request: SimulationRequest = {
    route,
    person: { preset: state.preset, overrides: overridesOf(state) },
    seed: Math.max(0, Math.trunc(state.seed)),
    individual: Math.max(0, Math.trunc(state.individual)),
    settings: settingsOf(state),
  }
  const name = state.name.trim()
  if (name !== '') {
    request.name = name
  }
  if (state.mapId !== null && state.mapId !== '') {
    request.map = { kind: 'id', id: state.mapId }
  }
  return request
}

/** Validation issues of a form state; an empty array means it can be submitted. */
export function validateForm(state: SimulationFormState): FormIssue[] {
  const issues: FormIssue[] = []
  const start = pointToVec(state.start)
  if (start === null) {
    issues.push({ field: 'start', key: 'simulation.route.start' })
  }
  if (state.mode !== 'loop') {
    const goal = pointToVec(state.goal)
    if (goal === null) {
      issues.push({ field: 'goal', key: 'simulation.route.goal' })
    } else if (start !== null && goal.x === start.x && goal.y === start.y) {
      issues.push({ field: 'goal', key: 'simulation.route.startEqualsGoal' })
    }
  }
  if (state.mode === 'loop' && (!Number.isFinite(state.laps) || state.laps < 1)) {
    issues.push({ field: 'laps', key: 'simulation.route.laps' })
  }
  if (state.mode === 'dynamic' && state.checkpoints.length === 0) {
    issues.push({ field: 'checkpoints', key: 'simulation.route.checkpoints' })
  }
  state.waypoints.forEach((waypoint, index) => {
    if (!isCompletePoint(waypoint.position)) {
      issues.push({ field: `waypoints.${index}`, key: 'simulation.route.waypoint' })
    }
    if (
      waypoint.semantics === 'dwell' &&
      (!Number.isFinite(waypoint.duration_s) || waypoint.duration_s < 0)
    ) {
      issues.push({ field: `waypoints.${index}.duration_s`, key: 'simulation.route.duration' })
    }
    if (
      waypoint.semantics === 'slow' &&
      (!Number.isFinite(waypoint.radius_m) || waypoint.radius_m <= 0)
    ) {
      issues.push({ field: `waypoints.${index}.radius_m`, key: 'simulation.route.radius' })
    }
  })
  state.checkpoints.forEach((checkpoint, index) => {
    if (!isCompletePoint(checkpoint.position)) {
      issues.push({ field: `checkpoints.${index}`, key: 'simulation.route.checkpoint' })
    }
    if (!Number.isFinite(checkpoint.issued_at_s) || checkpoint.issued_at_s < 0) {
      issues.push({ field: `checkpoints.${index}.issued_at_s`, key: 'simulation.route.issuedAt' })
    }
  })
  for (const [name, rate] of [
    ['gnss_rate_hz', state.sensors.gnss_rate_hz],
    ['imu_rate_hz', state.sensors.imu_rate_hz],
    ['mag_rate_hz', state.sensors.mag_rate_hz],
    ['baro_rate_hz', state.sensors.baro_rate_hz],
  ] as const) {
    if (rate !== null && (!Number.isFinite(rate) || rate <= 0)) {
      issues.push({ field: `sensors.${name}`, key: 'simulation.sensors.rates' })
    }
  }
  if (!Number.isFinite(state.seed) || state.seed < 0) {
    issues.push({ field: 'seed', key: 'simulation.form.seed' })
  }
  if (!Number.isInteger(state.individual) || state.individual < 0) {
    issues.push({ field: 'individual', key: 'simulation.form.individual' })
  }
  if (state.preset.trim() === '') {
    issues.push({ field: 'preset', key: 'simulation.person.preset' })
  }
  return issues
}

/** Sets one field of a point, leaving the other coordinate alone. */
/**
 * Reads a numeric field, treating an empty one as "no value".
 *
 * A numeric input reports `undefined` while it is being edited and `''` after it is
 * cleared, and `Number()` turns both into something wrong — `NaN` for the first, `0`
 * for the second. A coordinate that becomes `NaN` serialises to `null` and the service
 * refuses the whole request; a latitude that silently becomes `0` is worse, because the
 * request is accepted and the run is not the one that was asked for.
 */
export function numberOrNull(value: unknown): number | null {
  if (value === null || value === undefined || value === '') {
    return null
  }
  const parsed = typeof value === 'number' ? value : Number(value)
  return Number.isFinite(parsed) ? parsed : null
}

export function setPointCoordinate(
  point: PointDraft,
  axis: 'x' | 'y',
  value: number | null,
): PointDraft {
  return { ...point, [axis]: value }
}

/**
 * The override fields `/presets` reports for the chosen preset, with the type of
 * each taken from the resolved parameter vector.
 *
 * The form is generated from this rather than from a copy of `PersonParams`, so a
 * field core adds appears in the UI as soon as the service reports it.
 */
export interface OverrideField {
  /** Wire name of the field. */
  name: string
  /** Control to draw: a number, a free text or a fixed choice. */
  kind: 'number' | 'text' | 'choice'
  /** Choices of a `choice` field. */
  options?: Array<{ value: string; labelKey: string }>
}

/** The pace strategies the service accepts, with their catalog keys. */
const PACE_STRATEGIES: Array<{ value: string; labelKey: string }> = [
  { value: 'even', labelKey: 'simulation.person.strategyEven' },
  { value: 'positive_split', labelKey: 'simulation.person.strategyPositiveSplit' },
  { value: 'negative_split', labelKey: 'simulation.person.strategyNegativeSplit' },
]

/** Builds the override fields of one preset from its resolved parameters. */
export function overrideFields(names: readonly string[], params: unknown): OverrideField[] {
  const record =
    typeof params === 'object' && params !== null ? (params as Record<string, unknown>) : {}
  return names.map((name) => {
    if (name === 'pace_strategy') {
      return { name, kind: 'choice' as const, options: PACE_STRATEGIES }
    }
    const value = record[name]
    if (name === 'label' || typeof value === 'string') {
      return { name, kind: 'text' as const }
    }
    return { name, kind: 'number' as const }
  })
}

/** Keeps only the overrides the field list accepts. */
export function filterOverrides(
  overrides: Record<string, OverrideValue>,
  fields: readonly OverrideField[],
): Record<string, OverrideValue> {
  const allowed = new Set(fields.map((field) => field.name))
  const out: Record<string, OverrideValue> = {}
  for (const [key, value] of Object.entries(overrides)) {
    if (allowed.has(key)) {
      out[key] = value
    }
  }
  return out
}
