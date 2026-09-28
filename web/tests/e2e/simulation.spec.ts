/*
 * The simulation flow, end to end: submit → progress → summary → trajectory →
 * sensors → audit → export.
 *
 * The lane needs a running service; without one it fails by default, naming the
 * command that starts it, and skips only when OUREALIS_ALLOW_SKIP=1 is set (see
 * `tests/support/service.ts`). The run is submitted over the API, because the form
 * has its own lane (`run.spec.ts`) and what is under test here is the result page;
 * the waiting is done through the API too, because a poll from the test does not
 * depend on the page staying open.
 *
 * Run it against a service with:
 *   OUREALIS_SERVICE_URL=http://127.0.0.1:8080 pnpm test:e2e
 */
import { expect, test, type APIRequestContext, type Page } from '@playwright/test'
import { serviceGate } from '../support/service'
import { buildSyntheticMap } from '../support/tasks'

/** Whether this lane has a service, and what to say about it. */
const service = serviceGate()

/** Identifier of the run the flow produced, shared by the later tests. */
let jobId = ''

/** Identifier of the map the lane created, which the run is submitted against. */
let mapId = ''

/** Coordinates that fit inside both synthetic map presets, metres. */
const START = { x: 20, y: 20 }
const GOAL = { x: 200, y: 120 }

test.describe.configure({ mode: 'serial' })

test.describe('simulation flow', () => {
  test.beforeAll(async ({ request }) => {
    await service.probe(request)
    if (!service.available) {
      return
    }
    // A synthetic map keeps the lane independent of any fixture file and of what
    // another lane left in the library; it is also the smallest map there is. The
    // run names it explicitly, because a request without a map is only accepted
    // while the library holds exactly one.
    const name = `e2e simulation ${Date.now()}`
    mapId = await buildSyntheticMap(request, { preset: 'compact', seed: 0x0ddb1a5e }, name)
  })

  test('a submitted run reaches its result page', async ({ page }) => {
    service.skipUnlessAvailable()
    test.setTimeout(180_000)
    test.skip(mapId === '', 'no map was built for the run')

    const problems: string[] = []
    page.on('pageerror', (error) => problems.push(error.message))
    await page.addInitScript(() => globalThis.localStorage.setItem('ourealis.locale', 'en'))

    // The run is submitted over the API rather than through the workspace form: the
    // form has its own lane (`run.spec.ts`), and what this lane is about is the
    // result page — the live panel, the summary and the three detail tabs.
    const reply = await page.request.post('/api/v1/simulations', {
      data: {
        name: `e2e run ${Date.now()}`,
        map: { kind: 'id', id: mapId },
        route: {
          mode: 'standard',
          start: { x: START.x, y: START.y },
          goal: { x: GOAL.x, y: GOAL.y },
          waypoints: [],
        },
        person: { preset: 'moderate', overrides: { target_speed: 3.4 } },
        seed: 4242,
        individual: 0,
        settings: { with_metrics: true, sensors: {} },
      },
    })
    expect(reply.ok(), 'the service must accept the run').toBe(true)
    jobId = ((await reply.json()) as { id: string }).id
    expect(jobId).not.toBe('')

    await page.goto(`/runs/${jobId}`)
    await expect(page.locator('h1')).toHaveText('Run detail')
    expect(jobIdFrom(page)).toBe(jobId)

    await expect(page.getByTestId('job-state')).toBeVisible()
    await expect(page.getByTestId('elapsed')).toBeVisible()

    // Either the run is still going and its log is streaming, or it is already
    // done — both are the service's own report at that instant, never a guess.
    await expect
      .poll(
        async () => {
          const logs = await page.getByTestId('log-stream').count()
          const state = await page.getByTestId('job-state').innerText()
          return logs > 0 || state === 'Succeeded'
        },
        { timeout: 60_000 },
      )
      .toBe(true)

    const state = await waitForTerminal(page, jobId)
    expect(state).toBe('succeeded')

    await page.reload()
    await expect(page.getByTestId('route-length')).toBeVisible()
    await expect(page.getByTestId('headline-metrics').locator('tbody tr').first()).toBeVisible()

    expect(problems).toEqual([])
  })

  test('the trajectory tab draws the timeline and the synchronised charts', async ({ page }) => {
    service.skipUnlessAvailable()
    test.skip(jobId === '', 'no run was submitted')

    await page.addInitScript(() => globalThis.localStorage.setItem('ourealis.locale', 'en'))
    // The detail tabs live in the URL query, so a deep link opens the same one.
    await page.goto(`/runs/${jobId}?tab=trajectory`)
    await expect(page.locator('h1')).toHaveText('Run detail')
    await expect(page.getByTestId('trajectory-view')).toBeVisible()

    await expect(page.getByTestId('timeline')).toBeVisible()
    await expect(page.getByTestId('playback-controls')).toBeVisible()
    await expect(page.getByTestId('speed-gauge')).toBeVisible()
    await expect(page.getByTestId('trajectory-canvas')).toBeVisible()
    await expect(page.getByTestId('chart-speed').locator('canvas')).toBeVisible()

    // Scrubbing moves the playhead and the readouts follow it. The timeline's slider
    // is bound to the sample index, so clicking it halfway through drags the playhead
    // out of the opening seconds — which this run spends standing still — and the
    // clock and the speed gauge both have to follow.
    const clock = page.getByTestId('timeline-current')
    const gauge = page.getByTestId('gauge-speed')
    const rail = page.getByTestId('timeline-slider').locator('.t-slider__rail')
    const before = { clock: await clock.innerText(), speed: await gauge.innerText() }
    const box = await rail.boundingBox()
    expect(box, 'the timeline slider must be laid out').not.toBeNull()
    await rail.click({ position: { x: (box?.width ?? 0) * 0.5, y: (box?.height ?? 0) / 2 } })
    await expect(clock).not.toHaveText(before.clock)
    await expect(gauge).not.toHaveText(before.speed)

    // The playhead step moves on from there one sample at a time, and the gauge reads
    // the speed of the sample it lands on, which changes with every step in the moving
    // part of the run.
    const scrubbed = await gauge.innerText()
    await expect
      .poll(
        async () => {
          await page.getByTestId('playback-forward').click()
          return gauge.innerText()
        },
        { timeout: 15_000 },
      )
      .not.toBe(scrubbed)
  })

  test('the sensors tab plots a channel and exports the run', async ({ page }) => {
    service.skipUnlessAvailable()
    test.skip(jobId === '', 'no run was submitted')
    test.setTimeout(120_000)

    await page.addInitScript(() => globalThis.localStorage.setItem('ourealis.locale', 'en'))
    await page.goto(`/runs/${jobId}?tab=sensors`)
    await expect(page.locator('h1')).toHaveText('Run detail')
    await expect(page.getByTestId('sensor-view')).toBeVisible()

    await expect(page.getByTestId('channel-chart').locator('canvas')).toBeVisible()
    await expect(page.getByTestId('gnss-error-chart').locator('canvas')).toBeVisible()
    await expect(page.getByTestId('spectrum-chart').locator('canvas')).toBeVisible()

    const download = page.waitForEvent('download')
    await page.getByTestId('export-json').click()
    const file = await download
    expect(file.suggestedFilename()).toContain('.json')
  })

  test('the audit tab renders the metrics report and compares against a second run', async ({
    page,
  }) => {
    service.skipUnlessAvailable()
    test.skip(jobId === '', 'no run was submitted')
    test.setTimeout(300_000)

    await page.addInitScript(() => globalThis.localStorage.setItem('ourealis.locale', 'en'))
    await page.goto(`/runs/${jobId}?tab=audit`)
    await expect(page.locator('h1')).toHaveText('Run detail')
    await expect(page.getByTestId('audit-view')).toBeVisible()

    await expect(page.getByTestId('audit-primary').locator('tbody tr').first()).toBeVisible()
    await expect(page.getByTestId('audit-speed-histogram').locator('canvas')).toBeVisible()
    await expect(page.getByTestId('audit-acf-speed').locator('canvas')).toBeVisible()
    await expect(page.getByTestId('compare-select')).toBeVisible()

    // A comparison needs two finished runs. A restarted service keeps its maps but
    // not its job history, so when this lane's run is the only one, a second run is
    // submitted here and waited for; the delta table and the KS block are then the
    // service's own answer rather than a client-side subtraction of two summaries.
    const other = await secondFinishedRun(page)
    if (other !== null) {
      // The candidate list is read when the page opens, so a run that was submitted
      // just now has to be there before it can be chosen.
      await page.reload()
      await expect(page.getByTestId('compare-select')).toBeVisible()
      await page.getByTestId('compare-select').click()
      await page.locator('.t-select-option').filter({ hasText: other.name }).first().click()
      await page.getByTestId('compare-run').click()
      await expect(page.getByTestId('compare-table').locator('tbody tr').first()).toBeVisible()
      await expect(page.getByTestId('ks-statistic')).toBeVisible()
    }
  })

  test('the OMF inspector reads an image and writes an edit back', async ({ page }) => {
    service.skipUnlessAvailable()
    test.setTimeout(120_000)

    const target = await firstMapId(page)
    test.skip(target === null, 'the library holds no map to inspect')
    const image = await page.request
      .get(`/api/v1/maps/${target}/image`)
      .then((response) => response.body())

    await page.addInitScript(() => globalThis.localStorage.setItem('ourealis.locale', 'en'))
    // The inspector is the map library's drawer, so the lane opens it the way a
    // reader does: from the library header.
    await page.goto('/maps')
    await expect(page.locator('h1')).toHaveText('Maps')
    await page.getByTestId('map-inspect').click()
    await expect(page.getByTestId('omf-inspector')).toBeVisible()

    await page.getByTestId('omf-file').setInputFiles({
      name: 'library.omf',
      mimeType: 'application/octet-stream',
      buffer: image,
    })

    // The structure tree is the service's parse of the image; its layer and
    // directory tables are what the page renders from it.
    await expect(page.getByTestId('omf-layers').locator('tbody tr').first()).toBeVisible({
      timeout: 30_000,
    })
    await expect(page.getByTestId('omf-directory').locator('tbody tr').first()).toBeVisible()
    await expect(page.getByTestId('omf-metadata').locator('tbody tr').first()).toBeVisible()

    await page.getByTestId('edit-name').locator('input').fill('renamed by the e2e lane')
    await page.getByTestId('edit-apply').click()
    await expect(page.getByTestId('edit-download')).toBeEnabled({ timeout: 60_000 })

    const download = page.waitForEvent('download')
    await page.getByTestId('edit-download').click()
    const file = await download
    expect(file.suggestedFilename()).toMatch(/\.omf$/)
  })

  test('the remaining pages of the slice render without an uncaught error', async ({ page }) => {
    service.skipUnlessAvailable()

    const problems: string[] = []
    page.on('pageerror', (error) => problems.push(error.message))
    await page.addInitScript(() => globalThis.localStorage.setItem('ourealis.locale', 'en'))

    const pages: Array<{ path: string; heading: string }> = [
      { path: '/maps', heading: 'Maps' },
      { path: '/run', heading: 'Run workspace' },
      { path: '/runs', heading: 'Run history' },
      { path: '/settings', heading: 'Settings' },
    ]
    for (const entry of pages) {
      // Pages are opened one after another so a failure names the page it happened
      // on; each is a full navigation, so the loop is also the isolation.
      // oxlint-disable-next-line no-await-in-loop
      await page.goto(entry.path)
      // oxlint-disable-next-line no-await-in-loop
      await expect(page.locator('h1')).toHaveText(entry.heading)
    }
    expect(problems).toEqual([])
  })

  test('the map studio draws a region and exports an edited image', async ({ page }) => {
    service.skipUnlessAvailable()
    test.setTimeout(180_000)

    const target = await firstMapId(page)
    test.skip(target === null, 'the library holds no map to draw on')

    await page.addInitScript(() => globalThis.localStorage.setItem('ourealis.locale', 'en'))
    // The studio is the library's other drawer, opened from the row of the map the
    // lane draws on.
    await page.goto('/maps')
    const row = page.locator('[data-testid="map-table"] tbody tr').first()
    await expect(row).toBeVisible({ timeout: 30_000 })
    await row.getByTestId('map-edit').click()
    await expect(page.getByTestId('map-studio')).toBeVisible()

    const engine = await debugEngine(page)
    test.skip(engine === null, 'the studio needs a renderer backend for its surface')

    // The base image is the library map, which is also the file the edits are
    // applied to; the API has no route that returns a map's bytes, so the page
    // downloads it from the image endpoint.
    await page.getByTestId('studio-base').click()
    await expect(page.getByTestId('studio-export')).toBeEnabled({ timeout: 30_000 })

    const canvas = page.getByTestId('studio-canvas')
    await expect(canvas).toBeVisible()
    const box = await canvas.boundingBox()
    expect(box).not.toBeNull()
    if (box !== null) {
      for (const fraction of [0.3, 0.6, 0.45]) {
        // Three vertices make an outline; each click is a pick on the surface.
        // oxlint-disable-next-line no-await-in-loop
        await canvas.click({ position: { x: box.width * fraction, y: box.height * 0.7 } })
      }
      await page.getByTestId('studio-finish-region').click()
      await expect(page.getByTestId('studio-regions').locator('tbody tr')).toHaveCount(1)

      const download = page.waitForEvent('download')
      await page.getByTestId('studio-export').click()
      const file = await download
      expect(file.suggestedFilename()).toMatch(/\.omf$/)
    }
  })

  test('a run of an unknown identifier says so instead of failing silently', async ({ page }) => {
    service.skipUnlessAvailable()

    await page.addInitScript(() => globalThis.localStorage.setItem('ourealis.locale', 'en'))
    await page.goto('/runs/does-not-exist')
    await expect(page.getByTestId('simulation-view')).toBeVisible()
    await expect(page.getByTestId('job-state')).toHaveCount(0)
    await expect(page.getByTestId('job-missing')).toBeVisible()
  })
})

/**
 * The renderer backend the studio reported, or null when it did not start.
 *
 * The studio probes for a backend as its drawer opens, so the state is polled rather
 * than read once; a machine without a renderer never reports one, and the caller
 * skips instead of failing.
 */
async function debugEngine(page: Page): Promise<string | null> {
  const handle = await page
    .waitForFunction(
      () => {
        const state = (
          globalThis as unknown as Record<string, { engine?: string | null } | undefined>
        )['__ourealis_debug']
        return state?.engine ?? null
      },
      undefined,
      { timeout: 30_000 },
    )
    .catch(() => null)
  if (handle === null) {
    return null
  }
  const engine = (await handle.jsonValue()) as unknown
  return typeof engine === 'string' ? engine : null
}

/**
 * A finished run other than the one under test.
 *
 * The existing history is preferred; when the lane's own run is the only one, a
 * second run is submitted over the API and waited for, because the comparison the
 * audit page offers needs two summaries to compare.
 */
async function secondFinishedRun(page: Page): Promise<{ id: string; name: string } | null> {
  const existing = await finishedRuns(page)
  const candidate = existing.find((entry) => entry.id !== jobId)
  if (candidate !== undefined) {
    return candidate
  }
  const target = await firstMapId(page)
  if (target === null) {
    return null
  }
  const name = `e2e comparison ${Date.now()}`
  const reply = await page.request
    .post('/api/v1/simulations', {
      failOnStatusCode: false,
      data: {
        name,
        map: { kind: 'id', id: target },
        route: {
          mode: 'standard',
          start: { x: START.x, y: START.y },
          goal: { x: GOAL.x, y: GOAL.y },
          waypoints: [],
        },
        person: { preset: 'moderate', overrides: { target_speed: 3.4 } },
        seed: 4242,
        individual: 1,
        settings: { with_metrics: true, sensors: {} },
      },
    })
    .then((response) => (response.ok() ? response.json() : null))
    .catch(() => null)
  const id = (reply as { id?: unknown } | null)?.id
  if (typeof id !== 'string') {
    return null
  }
  for (let attempt = 0; attempt < 120; attempt += 1) {
    // oxlint-disable-next-line no-await-in-loop
    await page.waitForTimeout(1_000)
    // oxlint-disable-next-line no-await-in-loop
    const state = await page.request
      .get(`/api/v1/simulations/${id}`, { failOnStatusCode: false })
      .then((response) => (response.ok() ? response.json() : null))
      .catch(() => null)
    const current = (state as { state?: unknown } | null)?.state
    if (current === 'succeeded') {
      return { id, name }
    }
    if (current === 'failed' || current === 'cancelled') {
      return null
    }
  }
  return null
}

/** Finished runs the service reports, newest first. */
async function finishedRuns(page: Page): Promise<Array<{ id: string; name: string }>> {
  const list = await page.request
    .get('/api/v1/simulations?limit=50', { failOnStatusCode: false })
    .then((response) => (response.ok() ? response.json() : null))
    .catch(() => null)
  const items = (
    list as { items?: Array<{ id?: unknown; name?: unknown; state?: unknown }> } | null
  )?.items
  if (!Array.isArray(items)) {
    return []
  }
  return items
    .filter((entry) => entry.state === 'succeeded' && typeof entry.id === 'string')
    .map((entry) => ({
      id: entry.id as string,
      name: typeof entry.name === 'string' ? entry.name : (entry.id as string),
    }))
}

/** Identifier of any map in the library, or null when it is empty. */
async function firstMapId(page: Page): Promise<string | null> {
  const list = await page.request
    .get('/api/v1/maps', { failOnStatusCode: false })
    .then((response) => (response.ok() ? response.json() : null))
    .catch(() => null)
  const items = (list as { items?: Array<{ id?: unknown }> } | null)?.items
  const id = Array.isArray(items) ? items[0]?.id : undefined
  return typeof id === 'string' ? id : null
}

/** Reads the job identifier out of the current path. */
function jobIdFrom(page: Page): string {
  const match = /\/runs\/([^/?]+)/.exec(page.url())
  return match?.[1] ?? ''
}

/** Waits until the service reports the job in a terminal state. */
async function waitForTerminal(page: Page, id: string): Promise<string> {
  const request: APIRequestContext = page.request
  let state = 'queued'
  await expect
    .poll(
      async () => {
        const reply = await request
          .get(`/api/v1/simulations/${id}`, { failOnStatusCode: false })
          .then((response) => (response.ok() ? response.json() : null))
          .catch(() => null)
        state = typeof reply?.state === 'string' ? reply.state : 'queued'
        return ['succeeded', 'failed', 'cancelled'].includes(state)
      },
      { timeout: 150_000, intervals: [1_000] },
    )
    .toBe(true)
  return state
}
