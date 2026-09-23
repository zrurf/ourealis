/*
 * The simulation flow, end to end: submit → progress → summary → trajectory →
 * sensors → audit → export.
 *
 * The lane needs a running service; without one it fails by default, naming the
 * command that starts it, and skips only when OUREALIS_ALLOW_SKIP=1 is set (see
 * `tests/support/service.ts`). The run itself is created through the UI — the form
 * is the thing under test — and only the waiting is done through the API, because
 * a poll from the test does not depend on the page staying open.
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

/** Name of the map the lane created, which the run is submitted against. */
let mapName = ''

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
    mapName = `e2e simulation ${Date.now()}`
    await buildSyntheticMap(request, { preset: 'compact', seed: 0x0ddb1a5e }, mapName)
  })

  test('a run submitted through the form reaches its result page', async ({ page }) => {
    service.skipUnlessAvailable()
    test.setTimeout(180_000)

    const problems: string[] = []
    page.on('pageerror', (error) => problems.push(error.message))
    await page.addInitScript(() => globalThis.localStorage.setItem('ourealis.locale', 'en'))

    await page.goto('/simulations/new')
    await expect(page.locator('h1')).toHaveText('Run parameters')

    await page.getByTestId('form-name').locator('input').fill(`e2e run ${Date.now()}`)
    // The form preselects a map, because the service refuses a request that names
    // none while the library holds several; the run below is against the map this
    // lane created only when the library was empty, so the assertion is only that
    // a map is named.
    const mapInput = page.getByTestId('route-map').locator('input')
    await expect(mapInput).not.toHaveValue('')
    expect(mapName).not.toBe('')
    await fillNumber(page, 'start-x', START.x)
    await fillNumber(page, 'start-y', START.y)
    await fillNumber(page, 'goal-x', GOAL.x)
    await fillNumber(page, 'goal-y', GOAL.y)
    await page.getByTestId('form-submit').click()

    // The reply routes the page at the new job; its identifier is the contract the
    // rest of the lane works from. `new` is the submission route itself, so the
    // wait is for the path to name a job rather than merely to exist.
    await expect.poll(() => jobIdFrom(page), { timeout: 30_000 }).not.toBe('new')
    jobId = await jobIdFrom(page)
    expect(jobId).not.toBe('')

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

  test('the trajectory page draws the timeline and the synchronised charts', async ({ page }) => {
    service.skipUnlessAvailable()
    test.skip(jobId === '', 'no run was submitted')

    await page.addInitScript(() => globalThis.localStorage.setItem('ourealis.locale', 'en'))
    await page.goto(`/simulations/${jobId}/trajectory`)
    await expect(page.locator('h1')).toHaveText('Trajectory')

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

  test('the sensor page plots a channel and exports the run', async ({ page }) => {
    service.skipUnlessAvailable()
    test.skip(jobId === '', 'no run was submitted')
    test.setTimeout(120_000)

    await page.addInitScript(() => globalThis.localStorage.setItem('ourealis.locale', 'en'))
    await page.goto(`/simulations/${jobId}/sensors`)
    await expect(page.locator('h1')).toHaveText('Sensors')

    await expect(page.getByTestId('channel-chart').locator('canvas')).toBeVisible()
    await expect(page.getByTestId('gnss-error-chart').locator('canvas')).toBeVisible()
    await expect(page.getByTestId('spectrum-chart').locator('canvas')).toBeVisible()

    const download = page.waitForEvent('download')
    await page.getByTestId('export-json').click()
    const file = await download
    expect(file.suggestedFilename()).toContain('.json')
  })

  test('the audit page renders the metrics report and compares against a second run', async ({
    page,
  }) => {
    service.skipUnlessAvailable()
    test.skip(jobId === '', 'no run was submitted')
    test.setTimeout(300_000)

    await page.addInitScript(() => globalThis.localStorage.setItem('ourealis.locale', 'en'))
    await page.goto(`/simulations/${jobId}/audit`)
    await expect(page.locator('h1')).toHaveText('Audit')

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

    const mapId = await firstMapId(page)
    test.skip(mapId === null, 'the library holds no map to inspect')
    const image = await page.request
      .get(`/api/v1/maps/${mapId}/image`)
      .then((response) => response.body())

    await page.addInitScript(() => globalThis.localStorage.setItem('ourealis.locale', 'en'))
    await page.goto('/omf')
    await expect(page.locator('h1')).toHaveText('OMF inspector')

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

    const mapId = await firstMapId(page)
    const pages: Array<{ path: string; heading: string }> = [
      { path: '/', heading: 'Overview' },
      { path: '/run', heading: 'Run workspace' },
      { path: '/batch', heading: 'Batch runs' },
      { path: '/omf', heading: 'OMF inspector' },
      { path: '/settings', heading: 'Settings' },
    ]
    if (mapId !== null) {
      pages.push({ path: `/maps/${mapId}/studio`, heading: 'Map studio' })
    }
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

    const mapId = await firstMapId(page)
    test.skip(mapId === null, 'the library holds no map to draw on')

    await page.addInitScript(() => globalThis.localStorage.setItem('ourealis.locale', 'en'))
    await page.goto(`/maps/${mapId}/studio`)
    await expect(page.locator('h1')).toHaveText('Map studio')

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
    await page.goto('/simulations/does-not-exist')
    await expect(page.getByTestId('simulation-view')).toBeVisible()
    await expect(page.getByTestId('job-state')).toHaveCount(0)
  })
})

/** The renderer backend the studio reported, or null when it could not start. */
async function debugEngine(page: Page): Promise<string | null> {
  return page.evaluate(() => {
    const state = (globalThis as unknown as Record<string, { engine?: string | null } | undefined>)[
      '__ourealis_debug'
    ]
    return state?.engine ?? null
  })
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
  const mapId = await firstMapId(page)
  if (mapId === null) {
    return null
  }
  const name = `e2e comparison ${Date.now()}`
  const reply = await page.request
    .post('/api/v1/simulations', {
      failOnStatusCode: false,
      data: {
        name,
        map: { kind: 'id', id: mapId },
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
  const match = /\/simulations\/([^/?]+)/.exec(page.url())
  return match?.[1] ?? ''
}

/** Fills one number input of the form. */
async function fillNumber(page: Page, testId: string, value: number): Promise<void> {
  const input = page.getByTestId(testId).locator('input')
  await input.fill(String(value))
  await input.blur()
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
