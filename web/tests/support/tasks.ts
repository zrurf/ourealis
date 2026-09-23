/*
 * Building a map through the task API.
 *
 * Map generation is a task: the submission returns a ticket and the map appears in the
 * library when it finishes. A test that needs a map therefore submits, polls and waits
 * — the same sequence the page runs, which is why it is worth going through the API
 * rather than seeding the store directly.
 */
import { expect, type APIRequestContext } from '@playwright/test'

/** The service under test; the lane's runner sets this. */
const SERVICE_URL = process.env.OUREALIS_SERVICE_URL ?? 'http://127.0.0.1:8080'

/** How long a build may take before the test gives up, milliseconds. */
const BUILD_TIMEOUT_MS = 120_000

/**
 * Submits a synthetic map build and waits for it, returning the new map's id.
 *
 * Polling rather than sleeping a fixed time: a debug build rasterises the 300 x 200 m
 * fixture in about a second and a large map in minutes, and a fixed wait would be both
 * too short for the second case and wasted time in the first.
 */
export async function buildSyntheticMap(
  request: APIRequestContext,
  spec: Record<string, unknown>,
  name?: string,
): Promise<string> {
  const submitted = await request.post(`${SERVICE_URL}/api/v1/tasks`, {
    data: { kind: 'synthetic_map', spec, ...(name === undefined ? {} : { name }) },
  })
  expect(submitted.status(), 'a map build must be accepted').toBe(202)
  const ticket = ((await submitted.json()) as { id: string }).id

  const deadline = Date.now() + BUILD_TIMEOUT_MS
  for (;;) {
    // eslint-disable-next-line no-await-in-loop
    const state = await request.get(`${SERVICE_URL}/api/v1/tasks/${ticket}`)
    const body = (await state.json()) as { state: string; error?: string | null }
    if (body.state === 'succeeded') {
      break
    }
    expect(body.state, `the build must not fail: ${body.error ?? ''}`).not.toBe('failed')
    expect(Date.now(), 'the build must finish').toBeLessThan(deadline)
    // eslint-disable-next-line no-await-in-loop
    await new Promise((resolve) => setTimeout(resolve, 100))
  }

  const result = await request.get(`${SERVICE_URL}/api/v1/tasks/${ticket}/result`)
  const body = (await result.json()) as { map: { id: string } }
  return body.map.id
}
