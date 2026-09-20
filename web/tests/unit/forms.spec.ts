/*
 * The simulation form's model: validation and the request it assembles.
 *
 * `SimulationRequest` denies unknown fields and rejects a `null` where the service
 * wants a number, so the shape is asserted field by field rather than loosely: a
 * test that only checked "a route was sent" would not catch a stray key or a
 * numeric field sent as text, which is exactly what the service answers with 400.
 */
import { expect, test } from '@playwright/test'
import {
  buildSimulationRequest,
  defaultFormState,
  emptyPoint,
  emptySensorDraft,
  filterOverrides,
  isCompletePoint,
  newCheckpoint,
  newWaypoint,
  overrideFields,
  overridesOf,
  pointToVec,
  routeSpecOf,
  sensorSettingsOf,
  settingsOf,
  setPointCoordinate,
  validateForm,
  waypointSemantics,
  type SimulationFormState,
} from '../../src/components/forms/request'

/** A complete standard route on a named map. */
function standardForm(): SimulationFormState {
  const state = defaultFormState()
  return {
    ...state,
    name: 'campus run',
    mapId: 'map-7',
    start: { x: 40, y: 60 },
    goal: { x: 250, y: 150 },
    seed: 12345,
    individual: 2,
    preset: 'moderate',
    overrides: { target_speed: 3.1 },
  }
}

test.describe('request assembly', () => {
  test('a standard run carries the exact JSON shape the service accepts', () => {
    const request = buildSimulationRequest(standardForm())
    expect(request).toEqual({
      name: 'campus run',
      map: { kind: 'id', id: 'map-7' },
      route: {
        mode: 'standard',
        start: { x: 40, y: 60 },
        goal: { x: 250, y: 150 },
        waypoints: [],
      },
      person: { preset: 'moderate', overrides: { target_speed: 3.1 } },
      seed: 12345,
      individual: 2,
      settings: {
        sensors: {},
        with_metrics: true,
        route: { smooth: true },
      },
    })
  })

  test('an unnamed run on the library default omits `name` and `map` entirely', () => {
    const request = buildSimulationRequest(standardForm())
    const bare = buildSimulationRequest({ ...standardForm(), name: '   ', mapId: null })
    expect('name' in bare).toBe(false)
    expect('map' in bare).toBe(false)
    expect(request.map).toEqual({ kind: 'id', id: 'map-7' })
  })

  test('waypoints carry their passing semantics and radius', () => {
    const state = standardForm()
    const request = buildSimulationRequest({
      ...state,
      waypoints: [
        { position: { x: 100, y: 100 }, semantics: 'pass', duration_s: 10, radius_m: 5 },
        { position: { x: 150, y: 120 }, semantics: 'slow', duration_s: 10, radius_m: 8 },
        { position: { x: 200, y: 140 }, semantics: 'dwell', duration_s: 12.5, radius_m: 5 },
      ],
    })
    expect(request.route).toEqual({
      mode: 'standard',
      start: { x: 40, y: 60 },
      goal: { x: 250, y: 150 },
      waypoints: [
        { position: { x: 100, y: 100 }, semantics: { kind: 'pass' }, radius_m: 5 },
        { position: { x: 150, y: 120 }, semantics: { kind: 'slow' }, radius_m: 8 },
        {
          position: { x: 200, y: 140 },
          semantics: { kind: 'dwell', duration_s: 12.5 },
          radius_m: 5,
        },
      ],
    })
  })

  test('a loop route carries the reference point and the laps, with a null reference when unset', () => {
    const state = standardForm()
    expect(
      buildSimulationRequest({
        ...state,
        mode: 'loop',
        reference: { x: 10, y: 20 },
        laps: 3,
      }).route,
    ).toEqual({ mode: 'loop', start: { x: 40, y: 60 }, reference: { x: 10, y: 20 }, laps: 3 })

    expect(buildSimulationRequest({ ...state, mode: 'loop', laps: 1 }).route).toEqual({
      mode: 'loop',
      start: { x: 40, y: 60 },
      reference: null,
      laps: 1,
    })
  })

  test('a dynamic route carries its checkpoints with the time each takes effect', () => {
    const state = standardForm()
    const request = buildSimulationRequest({
      ...state,
      mode: 'dynamic',
      checkpoints: [
        { position: { x: 120, y: 90 }, issued_at_s: 30 },
        { position: { x: 200, y: 120 }, issued_at_s: 75.5 },
      ],
    })
    expect(request.route).toEqual({
      mode: 'dynamic',
      start: { x: 40, y: 60 },
      goal: { x: 250, y: 150 },
      checkpoints: [
        { position: { x: 120, y: 90 }, issued_at_s: 30 },
        { position: { x: 200, y: 120 }, issued_at_s: 75.5 },
      ],
    })
  })

  test('an incomplete route is not assembled at all', () => {
    const state = standardForm()
    expect(routeSpecOf({ ...state, start: emptyPoint() })).toBeNull()
    expect(() => buildSimulationRequest({ ...state, goal: emptyPoint() })).toThrow()
    expect(() => buildSimulationRequest({ ...state, start: { x: 1, y: null } })).toThrow()
  })

  test('sensor settings carry only the fields the form set', () => {
    const sensors = emptySensorDraft()
    expect(sensorSettingsOf(sensors)).toEqual({})

    expect(
      sensorSettingsOf({
        ...sensors,
        gnss_rate_hz: 5,
        imu_rate_hz: 200,
        mount: 'head',
        multipath_enabled: false,
        force_deterministic_events: true,
      }),
    ).toEqual({
      gnss_rate_hz: 5,
      imu_rate_hz: 200,
      mount: 'head',
      multipath_enabled: false,
      force_deterministic_events: true,
    })
  })

  test('the settings block always states the metric and smoothing choices', () => {
    const state = standardForm()
    expect(settingsOf(state)).toEqual({
      sensors: {},
      with_metrics: true,
      route: { smooth: true },
    })
    expect(
      settingsOf({
        ...state,
        withMetrics: false,
        smooth: false,
        backend: 'cpu',
        motionMode: 'race',
      }),
    ).toEqual({
      sensors: {},
      with_metrics: false,
      route: { smooth: false },
      backend: 'cpu',
      mode: 'race',
    })
  })

  test('an override that is blank or not finite is dropped instead of sent', () => {
    const state = standardForm()
    expect(
      overridesOf({
        ...state,
        overrides: { target_speed: 3.1, label: '  ', k_down: Number.NaN, beta_logit: 0 },
      }),
    ).toEqual({ target_speed: 3.1, beta_logit: 0 })
  })

  test('numbers are truncated to integers where the schema wants an integer', () => {
    const request = buildSimulationRequest({
      ...standardForm(),
      seed: 7.9,
      individual: 3.4,
      laps: 2.7,
      mode: 'loop',
    })
    expect(request.seed).toBe(7)
    expect(request.individual).toBe(3)
    expect(request.route).toMatchObject({ laps: 2 })
  })
})

test.describe('validation', () => {
  test('a complete standard form has no issues', () => {
    expect(validateForm(standardForm())).toEqual([])
  })

  test('a missing or degenerate route is reported per field', () => {
    const state = standardForm()
    expect(validateForm({ ...state, start: emptyPoint() })).toEqual([
      { field: 'start', key: 'simulation.route.start' },
    ])
    expect(validateForm({ ...state, goal: { x: 40, y: 60 } })).toEqual([
      { field: 'goal', key: 'simulation.route.startEqualsGoal' },
    ])
    expect(validateForm({ ...state, goal: emptyPoint() })).toEqual([
      { field: 'goal', key: 'simulation.route.goal' },
    ])
  })

  test('a loop needs at least one lap, a dynamic route at least one checkpoint', () => {
    const state = standardForm()
    expect(validateForm({ ...state, mode: 'loop', laps: 0 })).toEqual([
      { field: 'laps', key: 'simulation.route.laps' },
    ])
    expect(validateForm({ ...state, mode: 'dynamic' })).toEqual([
      { field: 'checkpoints', key: 'simulation.route.checkpoints' },
    ])
  })

  test('a waypoint with a missing coordinate or a bad dwell time is reported', () => {
    const state = standardForm()
    const incomplete = validateForm({
      ...state,
      waypoints: [{ ...newWaypoint(), position: { x: null, y: 5 } }],
    })
    expect(incomplete).toEqual([{ field: 'waypoints.0', key: 'simulation.route.waypoint' }])

    const negative = validateForm({
      ...state,
      waypoints: [{ ...newWaypoint({ x: 1, y: 1 }), semantics: 'dwell', duration_s: -1 }],
    })
    expect(negative).toEqual([
      { field: 'waypoints.0.duration_s', key: 'simulation.route.duration' },
    ])

    const slow = validateForm({
      ...state,
      waypoints: [{ ...newWaypoint({ x: 1, y: 1 }), semantics: 'slow', radius_m: 0 }],
    })
    expect(slow).toEqual([{ field: 'waypoints.0.radius_m', key: 'simulation.route.radius' }])
  })

  test('a checkpoint in the past or without a position is reported', () => {
    const state = standardForm()
    expect(
      validateForm({
        ...state,
        mode: 'dynamic',
        checkpoints: [{ ...newCheckpoint({ x: null, y: null }), issued_at_s: -5 }],
      }),
    ).toEqual([
      { field: 'checkpoints.0', key: 'simulation.route.checkpoint' },
      { field: 'checkpoints.0.issued_at_s', key: 'simulation.route.issuedAt' },
    ])
  })

  test('a rate of zero, a negative seed and a fractional individual are reported', () => {
    const state = standardForm()
    expect(
      validateForm({
        ...state,
        seed: -1,
        individual: 1.5,
        sensors: { ...emptySensorDraft(), gnss_rate_hz: 0 },
      }),
    ).toEqual([
      { field: 'sensors.gnss_rate_hz', key: 'simulation.sensors.rates' },
      { field: 'seed', key: 'simulation.form.seed' },
      { field: 'individual', key: 'simulation.form.individual' },
    ])
  })

  test('a preset from a catalog the service does not know is not submitted', () => {
    expect(validateForm({ ...standardForm(), preset: 'sprint' })).toEqual([])
    expect(validateForm({ ...standardForm(), preset: '  ' })).toEqual([
      { field: 'preset', key: 'simulation.person.preset' },
    ])
  })
})

test.describe('override fields', () => {
  test('the controls are typed from the preset’s own parameters', () => {
    const fields = overrideFields(['label', 'target_speed', 'pace_strategy', 'unknown_field'], {
      label: 'moderate',
      target_speed: 3.0,
      pace_strategy: 'even',
    })
    expect(fields.map((field) => `${field.name}:${field.kind}`)).toEqual([
      'label:text',
      'target_speed:number',
      'pace_strategy:choice',
      'unknown_field:number',
    ])
    expect(fields[2]?.options?.map((option) => option.value)).toEqual([
      'even',
      'positive_split',
      'negative_split',
    ])
  })

  test('an override outside the reported field list never reaches the request', () => {
    const fields = overrideFields(['target_speed'], { target_speed: 3 })
    expect(filterOverrides({ target_speed: 3.1, not_a_field: 9 }, fields)).toEqual({
      target_speed: 3.1,
    })
  })
})

test.describe('point helpers', () => {
  test('a point needs both coordinates to become a wire vector', () => {
    expect(isCompletePoint({ x: 1, y: 2 })).toBe(true)
    expect(isCompletePoint({ x: null, y: 2 })).toBe(false)
    expect(isCompletePoint({ x: Number.NaN, y: 2 })).toBe(false)
    expect(pointToVec({ x: 1, y: 2 })).toEqual({ x: 1, y: 2 })
    expect(pointToVec({ x: 1, y: null })).toBeNull()
  })

  test('setting one coordinate leaves the other alone', () => {
    const point = setPointCoordinate({ x: 1, y: 2 }, 'x', 9)
    expect(point).toEqual({ x: 9, y: 2 })
  })

  test('the wire semantics of a waypoint follow its kind', () => {
    expect(waypointSemantics(newWaypoint())).toEqual({ kind: 'pass' })
    expect(waypointSemantics({ ...newWaypoint(), semantics: 'slow' })).toEqual({ kind: 'slow' })
    expect(waypointSemantics({ ...newWaypoint(), semantics: 'dwell', duration_s: 4 })).toEqual({
      kind: 'dwell',
      duration_s: 4,
    })
  })
})
