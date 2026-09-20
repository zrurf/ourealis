/*
 * The simulation store: its job state machine and its result-page cache.
 *
 * The store talks to the service through the shared HTTP client, which takes its
 * `fetch` when it is constructed — so the fake below is installed before the store
 * module is imported (`beforeAll`), and every request the store makes is answered
 * from the fake instead of the network. Each test therefore reads the same request
 * log the store produced, which is how the caching rules are checked: a page that
 * is already cached must not produce a request at all.
 */
import { expect, test } from '@playwright/test'
import { createPinia, setActivePinia } from 'pinia'
import type * as SimulationsModule from '../../src/stores/simulations'
import type { SimulationState } from '../../src/types/simulation'

/** Requests the store issued, as `path?query` strings. */
let requests: string[] = []

/** Sample counts the fake service reports. */
const TOTALS: Record<string, number> = { truth: 12_000, accel: 3_000, gyro: 2_000 }

/** The store module, imported after the fake transport is installed. */
let mod: typeof SimulationsModule

/** One page of truth samples, `time_s` equal to the sample index. */
function truthPage(offset: number, limit: number): unknown {
  const total = TOTALS.truth ?? 0
  const count = Math.max(0, Math.min(limit, total - offset))
  return {
    items: Array.from({ length: count }, (_, index) => ({
      time_s: offset + index,
      position: { x: index, y: 0 },
      position_low: { x: index, y: 0 },
      z: 0,
      terrain_z: 0,
    })),
    total,
    offset,
  }
}

/** One page of a sensor channel. */
function sensorPage(channel: string, offset: number, limit: number): unknown {
  const total = TOTALS[channel] ?? 0
  const count = Math.max(0, Math.min(limit, total - offset))
  return {
    items: Array.from({ length: count }, (_, index) => ({
      time_s: offset + index,
      channel,
      v: [0, 0, 0],
    })),
    total,
    offset,
  }
}

/** A summary the store can parse, carrying a minimal metrics report. */
function summaryBody(id: string): unknown {
  return {
    id,
    route_length_m: 1000,
    duration_s: 300,
    samples: {
      truth: TOTALS.truth,
      gnss: 0,
      accel: TOTALS.accel,
      gyro: TOTALS.gyro,
      mag: 0,
      baro: 0,
    },
    backend: 'cpu-rayon',
    metrics: { path_ratio: 1.2 },
    manifest: { seed: 1, individual: 0 },
    report: null,
  }
}

/** Answers one request the way the service would. */
function respond(url: string): Response {
  const parsed = new URL(url, 'http://service.test')
  const offset = Number(parsed.searchParams.get('offset') ?? '0')
  const limit = Number(parsed.searchParams.get('limit') ?? '5000')
  const path = parsed.pathname
  const truth = /\/simulations\/([^/]+)\/truth$/.exec(path)
  if (truth !== null) {
    return json(truthPage(offset, limit))
  }
  const sensors = /\/simulations\/([^/]+)\/sensors\/([^/]+)$/.exec(path)
  if (sensors !== null) {
    return json(sensorPage(decodeURIComponent(sensors[2] ?? ''), offset, limit))
  }
  const summary = /\/simulations\/([^/]+)\/summary$/.exec(path)
  if (summary !== null) {
    return json(summaryBody(decodeURIComponent(summary[1] ?? '')))
  }
  return json({ error: { kind: 'not_found', message: `no route for ${path}` } }, 404)
}

/** A JSON response. */
function json(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { 'content-type': 'application/json' },
  })
}

/** A job state with the fields the DTO requires. */
function stateOf(id: string, patch: Partial<SimulationState> = {}): SimulationState {
  return {
    id,
    state: 'running',
    stage: 'planning',
    map_id: 'map-1',
    mode: 'standard',
    created_at: new Date(0).toISOString(),
    ...patch,
  }
}

test.beforeAll(async () => {
  // The client takes its `fetch` when it is constructed, and the unit lane shares
  // one module registry between spec files — another file may have loaded the
  // client long before this one. Both the global and the client's own field are
  // therefore replaced, and both specifier spellings are patched because the
  // alias and the relative path can resolve to different instances.
  const fake = (async (input: RequestInfo | URL) => {
    const url = typeof input === 'string' ? input : String(input)
    requests.push(url)
    return respond(url)
  }) as typeof fetch
  globalThis.fetch = fake
  const clients = [await import('@/api/client'), await import('../../src/api/client')]
  for (const clientModule of clients) {
    ;(clientModule.api as unknown as { fetchImpl: unknown }).fetchImpl = fake
  }
  mod = await import('../../src/stores/simulations')
})

test.beforeEach(() => {
  requests = []
  setActivePinia(createPinia())
})

test.describe('job state machine', () => {
  test('state, stage and log events update the selected job', () => {
    const store = mod.useSimulationsStore()
    store.jobs = [stateOf('job-1')]
    store.select('job-1')
    store.current = stateOf('job-1')

    store.applyEvent({
      type: 'state',
      state: 'running',
      stage: 'motion',
      progress: null,
      elapsed_s: 4,
    })
    expect(store.current?.state).toBe('running')
    expect(store.current?.stage).toBe('motion')
    expect(store.elapsed_s).toBe(4)
    expect(store.currentIsLive).toBe(true)

    store.applyEvent({ type: 'stage', stage: 'sensors', progress: null, elapsed_s: 9 })
    expect(store.current?.stage).toBe('sensors')
    expect(store.current?.state).toBe('running')
    expect(store.elapsed_s).toBe(9)

    store.applyEvent({
      type: 'log',
      level: 'info',
      message: 'route planned: 968.7 m',
      elapsed_s: 1.24,
    })
    expect(store.logLines).toHaveLength(1)
    expect(store.logLines[0]?.message).toBe('route planned: 968.7 m')
    expect(store.logLines[0]?.elapsed_s).toBeCloseTo(1.24)
  })

  test('a done event ends the run and starts reading its summary', async () => {
    const store = mod.useSimulationsStore()
    store.jobs = [stateOf('job-2')]
    store.select('job-2')
    store.current = stateOf('job-2')
    expect(store.summaryStatus).toBe('idle')

    store.applyEvent({
      type: 'done',
      state: 'succeeded',
      summary_url: '/api/v1/simulations/job-2/summary',
    })
    expect(store.current?.state).toBe('succeeded')
    expect(store.current?.stage).toBe('done')
    expect(store.currentIsLive).toBe(false)
    expect(store.elapsed_s).toBe(0)
    // The read is fired from the event rather than awaited by it, so the assertion
    // is that the store both started one and finished it against the fake transport:
    // `loading` would mean the reply was never applied, and `failed` that it was
    // refused, and both are the store's own report rather than the event's.
    await expect.poll(() => store.summaryStatus).toBe('ready')
  })

  test('an error event records the failure and the service message', () => {
    const store = mod.useSimulationsStore()
    store.jobs = [stateOf('job-3')]
    store.select('job-3')
    store.current = stateOf('job-3')

    store.applyEvent({
      type: 'error',
      kind: 'core',
      message: 'no path exists between the start and the goal',
    })
    expect(store.current?.state).toBe('failed')
    expect(store.current?.error).toBe('no path exists between the start and the goal')
    expect(store.current?.error_kind).toBe('core')
    expect(store.streamStatus).toBe('failed')
    expect(store.currentIsLive).toBe(false)
  })

  test('the elapsed timer only advances while the job is live and never rewinds', () => {
    const store = mod.useSimulationsStore()
    store.jobs = [stateOf('job-4')]
    store.select('job-4')
    store.current = stateOf('job-4')

    store.tick()
    store.tick()
    expect(store.elapsed_s).toBe(2)

    // An event carrying a later time wins; an earlier one is ignored.
    store.setElapsed(10)
    expect(store.elapsed_s).toBe(10)
    store.setElapsed(3)
    expect(store.elapsed_s).toBe(10)

    store.applyEvent({ type: 'done', state: 'succeeded', summary_url: '' })
    store.tick()
    expect(store.elapsed_s).toBe(10)
  })

  test('selecting another job drops the log of the previous one', () => {
    const store = mod.useSimulationsStore()
    store.jobs = [stateOf('job-5'), stateOf('job-6')]
    store.select('job-5')
    store.applyEvent({ type: 'log', level: 'info', message: 'first', elapsed_s: 1 })
    expect(store.logLines).toHaveLength(1)

    store.select('job-6')
    expect(store.logLines).toHaveLength(0)
    expect(store.elapsed_s).toBe(0)
    expect(store.currentId).toBe('job-6')
  })
})

test.describe('result page cache', () => {
  test('a truth window is read once and served from the cache afterwards', async () => {
    const store = mod.useSimulationsStore()
    store.select('job-7')

    await store.loadTruthWindow(0, 5_000)
    expect(requests.filter((url) => url.includes('/truth'))).toHaveLength(1)
    expect(store.truthSamples()).toHaveLength(5_000)
    expect(store.truthTotal).toBe(12_000)

    await store.loadTruthWindow(0, 5_000)
    expect(requests.filter((url) => url.includes('/truth'))).toHaveLength(1)
    expect(store.resultStatus).toBe('ready')

    await store.loadTruthWindow(5_000, 5_000)
    expect(requests.filter((url) => url.includes('/truth'))).toHaveLength(2)
    expect(store.truthSamples()).toHaveLength(10_000)
  })

  test('reading the whole timeline extends the cache instead of replacing it', async () => {
    const store = mod.useSimulationsStore()
    store.select('job-8')

    await store.loadTruthWindow(0, 5_000)
    const all = await store.loadAllTruth()
    expect(all).toHaveLength(12_000)
    // Two windows plus the last short page; the first page is never re-read.
    expect(requests.filter((url) => url.includes('/truth'))).toHaveLength(3)
    expect(store.truthSamples()[0]?.time_s).toBe(0)
    expect(store.truthSamples()[11_999]?.time_s).toBe(11_999)
  })

  test('channels are cached independently of each other', async () => {
    const store = mod.useSimulationsStore()
    store.select('job-9')

    await store.loadAllSensor('accel')
    expect(requests.filter((url) => url.includes('/sensors/accel'))).toHaveLength(1)
    expect(store.sensorSamples('accel')).toHaveLength(3_000)
    expect(store.totalOf('accel')).toBe(3_000)

    await store.loadAllSensor('gyro')
    expect(requests.filter((url) => url.includes('/sensors/gyro'))).toHaveLength(1)
    expect(store.sensorSamples('accel')).toHaveLength(3_000)
    expect(store.sensorSamples('gyro')).toHaveLength(2_000)

    await store.loadAllSensor('accel')
    expect(requests.filter((url) => url.includes('/sensors/accel'))).toHaveLength(1)
  })

  test('a channel with no samples is an empty page, not a failure', async () => {
    const store = mod.useSimulationsStore()
    store.select('job-10')

    const samples = await store.loadAllSensor('baro')
    expect(samples).toEqual([])
    expect(store.resultStatus).toBe('ready')
    expect(store.resultError).toBeNull()
  })

  test('selecting another job clears the cached pages', async () => {
    const store = mod.useSimulationsStore()
    store.select('job-11')
    await store.loadTruthWindow(0, 5_000)
    expect(store.truthSamples()).toHaveLength(5_000)

    store.select('job-12')
    expect(store.truthSamples()).toEqual([])
    expect(store.truthTotal).toBe(0)
    expect(store.sensorSamples('accel')).toEqual([])
    expect(store.resultStatus).toBe('idle')
  })
})

test.describe('event envelope', () => {
  test('a frame wrapped by the service is unwrapped into the documented event', () => {
    // The service writes `{"at_ms": …, "event": {…}}`; the transport documents the
    // inner object, so the store has to reach through the envelope.
    const unwrapped = mod.unwrapEvent({
      at_ms: 1789890606606,
      event: {
        type: 'done',
        state: 'succeeded',
        summary_url: '/api/v1/simulations/x/summary',
      },
    })
    expect(unwrapped).toEqual({
      type: 'done',
      state: 'succeeded',
      summary_url: '/api/v1/simulations/x/summary',
    })

    // An unwrapped event and a frame with nothing usable are both handled.
    expect(mod.unwrapEvent({ type: 'log', level: 'info', message: 'x', elapsed_s: 1 })).toEqual({
      type: 'log',
      level: 'info',
      message: 'x',
      elapsed_s: 1,
    })
    expect(mod.unwrapEvent({ at_ms: 1, event: { type: 'pong' } })).toBeNull()
    expect(mod.unwrapEvent(null)).toBeNull()
  })

  test('a wrapped log event reaches the log stream', () => {
    const store = mod.useSimulationsStore()
    store.jobs = [stateOf('job-11')]
    store.select('job-11')
    store.current = stateOf('job-11')

    const wrapped = mod.unwrapEvent({
      at_ms: 2000,
      event: { type: 'log', level: 'info', message: 'route planned: 968.7 m', elapsed_s: 1.24 },
    })
    expect(wrapped).not.toBeNull()
    if (wrapped !== null) {
      store.applyEvent(wrapped)
    }
    expect(store.logLines[0]?.message).toBe('route planned: 968.7 m')
  })
})

test.describe('paging helper', () => {
  test('a series is reduced to the requested size and keeps its extent', () => {
    const values = Array.from({ length: 10_000 }, (_, index) => index)
    const reduced = mod.downsample(values, 1_000)
    expect(reduced.length).toBeLessThanOrEqual(1_001)
    expect(reduced[0]).toBe(0)
    expect(reduced[reduced.length - 1]).toBe(9_999)

    expect(mod.downsample(values, 0)).toHaveLength(0)
    expect(mod.downsample(values.slice(0, 10), 100)).toHaveLength(10)
  })
})
