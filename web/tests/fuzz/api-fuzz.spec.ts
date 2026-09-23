/**
 * Fuzz lane: malformed input must be refused, structured, and survivable.
 *
 * Four generators, all deterministic from a seed printed with every failure, so a
 * run is reproducible:
 *
 * 1. **random bytes** as a map image — the answer must be a classified error rather
 *    than a crash or an accepted garbage map;
 * 2. **mutated real images** — truncations and single-byte flips of a valid OMF
 *    image, which is where a parser actually breaks: the header is CRC-protected,
 *    the directory and body are covered by the whole-file hash;
 * 3. **random JSON request bodies** for job submission — the request DTOs are
 *    strict, so the answer must be a 400 naming the problem, never a 5xx;
 * 4. **absurd path parameters** — layer, level and chunk ids that do not exist.
 *
 * Each case ends by checking `/health`: an input that kills the process or leaves it
 * wedged is the failure this lane exists to catch.
 *
 * The lane needs a running service; without one it fails by default and points at
 * the command that starts the service, and only `OUREALIS_ALLOW_SKIP=1` turns that
 * back into a skip.
 */

import { expect, test, type APIRequestContext } from '@playwright/test'
import { serviceGate } from '../support/service'
import { buildSyntheticMap } from '../support/tasks'

/** API prefix, matching the service's `API_PREFIX`. */
const API = '/api/v1'

/** Seed of every generator in this file. */
const FUZZ_SEED = Number(process.env.OUREALIS_FUZZ_SEED ?? 0x5eed1234)

/** Whether this lane has a service, and what to say about it. */
const service = serviceGate()

test.beforeAll(async ({ request }) => {
  await service.probe(request)
})

test.beforeEach(() => {
  service.skipUnlessAvailable()
})

/** Deterministic 32-bit generator: mulberry32 is enough for input fuzzing. */
function makeRandom(seed: number): () => number {
  let state = seed >>> 0
  return () => {
    state = (state + 0x6d2b79f5) >>> 0
    let value = state
    value = Math.imul(value ^ (value >>> 15), value | 1)
    value ^= value + Math.imul(value ^ (value >>> 7), value | 61)
    return ((value ^ (value >>> 14)) >>> 0) / 4294967296
  }
}

/** True when the body is the documented error shape. */
function isErrorBody(body: unknown): boolean {
  if (typeof body !== 'object' || body === null) {
    return false
  }
  const error = (body as { error?: unknown }).error
  if (typeof error !== 'object' || error === null) {
    return false
  }
  const { kind, message } = error as { kind?: unknown; message?: unknown }
  return (
    typeof kind === 'string' && kind.length > 0 && typeof message === 'string' && message.length > 0
  )
}

/** Fails the test if the service stopped answering. */
async function assertHealthy(request: APIRequestContext, context: string): Promise<void> {
  const response = await request.get(`${API}/health`)
  expect(response.status(), `the service must survive ${context}`).toBe(200)
  expect(((await response.json()) as { status?: string }).status).toBe('ok')
}

/** Uploads a synthetic map and returns its id and the exact image bytes. */
async function seedMap(
  request: APIRequestContext,
): Promise<{ id: string; image: Buffer; sizeBytes: number }> {
  // Map generation is a task: submit, wait for the ticket, read the map from it.
  const id = await buildSyntheticMap(request, {
    preset: 'compact',
    seed: 11,
    with_kpath_library: false,
  })
  const summary = (await request.get(`${API}/maps/${id}`).then((reply) => reply.json())) as {
    summary: { size_bytes: number }
  }
  const sizeBytes = summary.summary.size_bytes

  const image = await request.get(`${API}/maps/${id}/image`)
  expect(image.status(), 'the stored image must be downloadable').toBe(200)
  const bytes = Buffer.from(await image.body())
  expect(bytes.length, 'the stored image is what the summary says').toBe(sizeBytes)
  return { id, image: bytes, sizeBytes }
}

test('random bytes are refused as a bad image', async ({ request }) => {
  const random = makeRandom(FUZZ_SEED)
  for (let iteration = 0; iteration < 20; iteration += 1) {
    const length = 1 + Math.floor(random() * 4096)
    const bytes = Buffer.alloc(length)
    for (let index = 0; index < length; index += 1) {
      bytes[index] = Math.floor(random() * 256)
    }
    const response = await request.post(`${API}/maps?name=fuzz`, {
      data: bytes,
      headers: { 'content-type': 'application/octet-stream' },
    })
    expect(
      response.status(),
      `random bytes of length ${length} must be refused (seed ${FUZZ_SEED}, iteration ${iteration})`,
    ).toBe(400)
    expect(isErrorBody(await response.json())).toBe(true)
  }

  // The same generator against the inspector, and against a body far too short to
  // hold a header at all.
  for (const payload of [Buffer.from([0, 1, 2, 3, 4, 5, 6, 7]), Buffer.alloc(0)]) {
    const inspect = await request.post(`${API}/omf/inspect`, {
      data: payload,
      headers: { 'content-type': 'application/octet-stream' },
    })
    expect([400, 413]).toContain(inspect.status())
    expect(isErrorBody(await inspect.json())).toBe(true)
  }
  await assertHealthy(request, 'random images')
})

test('a truncated or flipped image is refused rather than half-read', async ({ request }) => {
  const { id, image } = await seedMap(request)
  const random = makeRandom(FUZZ_SEED + 1)

  // Truncations: every prefix short of the whole file must be refused.
  for (let iteration = 0; iteration < 12; iteration += 1) {
    const cut = 1 + Math.floor(random() * (image.length - 1))
    const response = await request.post(`${API}/omf/inspect`, {
      data: image.subarray(0, cut),
      headers: { 'content-type': 'application/octet-stream' },
    })
    expect(
      response.status(),
      `a ${cut}-byte prefix of a ${image.length}-byte image must be refused`,
    ).toBe(400)
    expect(isErrorBody(await response.json())).toBe(true)
  }

  // Single-byte flips anywhere: the header is CRC-protected and the body is covered
  // by the file hash, so no flip may pass.
  for (let iteration = 0; iteration < 12; iteration += 1) {
    const corrupted = Buffer.from(image)
    const offset = Math.floor(random() * corrupted.length)
    corrupted[offset] = corrupted[offset]! ^ (1 << Math.floor(random() * 8))
    const response = await request.post(`${API}/omf/inspect`, {
      data: corrupted,
      headers: { 'content-type': 'application/octet-stream' },
    })
    expect(
      response.status(),
      `a flip at byte ${offset} must be refused (seed ${FUZZ_SEED + 1})`,
    ).toBe(400)
    expect(isErrorBody(await response.json())).toBe(true)
  }

  // The untouched image still opens, so the refusals are about the damage.
  const clean = await request.post(`${API}/omf/inspect`, {
    data: image,
    headers: { 'content-type': 'application/octet-stream' },
  })
  expect(clean.status()).toBe(200)
  await request.delete(`${API}/maps/${id}`)
  await assertHealthy(request, 'mutated images')
})

test('random job bodies are refused with a named field', async ({ request }) => {
  const random = makeRandom(FUZZ_SEED + 2)
  const pool: unknown[] = [
    null,
    0,
    -1,
    1e308,
    '',
    'standard',
    true,
    [],
    {},
    { mode: 'standard' },
    { mode: 'loop' },
    { x: 1, y: 2 },
    ['standard', 'loop'],
  ]
  const keys = [
    'name',
    'map',
    'route',
    'person',
    'seed',
    'individual',
    'settings',
    'mode',
    'kind',
    'start',
    'goal',
    'waypoints',
    'preset',
    'overrides',
    'target_speed',
    'sensors',
    'force_deterministic_events',
    'laps',
    'checkpoints',
    'position',
    'semantics',
  ]

  let refusals = 0
  for (let iteration = 0; iteration < 30; iteration += 1) {
    const fields = 1 + Math.floor(random() * 5)
    const body: Record<string, unknown> = {}
    for (let field = 0; field < fields; field += 1) {
      const key = keys[Math.floor(random() * keys.length)]!
      body[key] = pool[Math.floor(random() * pool.length)]
    }
    const response = await request.post(`${API}/simulations`, { data: body })
    const status = response.status()
    // 400 for a body the DTOs reject, 404 for a well-formed request naming a map that
    // does not exist, 202 for the rare body that happens to be valid. A 5xx would mean
    // the request mapper panicked on a client's input.
    expect(
      [400, 404, 202],
      `body ${JSON.stringify(body)} produced ${status} (seed ${FUZZ_SEED + 2}, iteration ${iteration})`,
    ).toContain(status)
    if (status === 400) {
      const parsed = (await response.json()) as unknown
      expect(isErrorBody(parsed)).toBe(true)
      refusals += 1
    }
  }
  expect(refusals, 'most random bodies must be refused').toBeGreaterThan(5)
  await assertHealthy(request, 'random job bodies')
})

test('absurd path parameters are client errors, not server faults', async ({ request }) => {
  const requests = [
    `${API}/maps/does-not-exist/layers/1/chunks/0/0`,
    `${API}/maps/does-not-exist`,
    `${API}/simulations/does-not-exist`,
    `${API}/simulations/does-not-exist/summary`,
    `${API}/simulations/does-not-exist/truth`,
  ]
  for (const path of requests) {
    const response = await request.get(path)
    expect(response.status(), `${path} must be a client error`).toBe(404)
    expect(isErrorBody(await response.json())).toBe(true)
  }

  // Layer, level and chunk ids far outside anything the map holds.
  const { id } = await seedMap(request)
  for (const path of [
    `${API}/maps/${id}/layers/65535/chunks/255/4294967295`,
    `${API}/maps/${id}/layers/1/chunks/9/0`,
    `${API}/maps/${id}/layers/0/chunks/0/0`,
  ]) {
    const response = await request.get(path)
    expect([400, 404, 200], `${path} produced ${response.status()}`).toContain(response.status())
    if (response.status() !== 200) {
      expect(isErrorBody(await response.json())).toBe(true)
    }
  }
  await request.delete(`${API}/maps/${id}`)
  await assertHealthy(request, 'absurd path parameters')
})
