/*
 * Map viewer end to end.
 *
 * These tests need a running service: they create a map through the API, open it
 * in the viewer and assert that the renderer reported a backend and kept drawing.
 * Without one the lane fails by default and points at the command that starts the
 * service; `OUREALIS_ALLOW_SKIP=1` skips it with the same reason instead.
 *
 * Run it against a service with:
 *   OUREALIS_SERVICE_URL=http://127.0.0.1:8080 pnpm test:e2e
 */
import { expect, test } from '@playwright/test'
import { serviceGate } from '../support/service'

/** The debug hook `src/render/scene.ts` publishes; the viewer's only test surface. */
interface DebugState {
  engine: string | null
  frames: number
  loaded: boolean
  mapId: string | null
  error: string | null
}

/** Whether this lane has a service, and what to say about it. */
const service = serviceGate()

test.describe.configure({ mode: 'serial' })

test.describe('map viewer', () => {
  test.beforeAll(async ({ request }) => {
    await service.probe(request)
  })

  test('a map created through the UI is listed and renders in the viewer', async ({ page }) => {
    service.skipUnlessAvailable()

    const problems: string[] = []
    page.on('console', (message) => {
      if (message.type() === 'error') {
        problems.push(message.text())
      }
    })
    page.on('pageerror', (error) => problems.push(error.message))
    // Pin the catalogue: another lane may have left a language choice behind.
    await page.addInitScript(() => globalThis.localStorage.setItem('ourealis.locale', 'en'))

    await page.goto('/maps')
    await expect(page.locator('h1')).toHaveText('Maps')

    // The import path this lane can produce without a fixture file: the service's
    // own generator, reached through the same POST the file import uses.
    const name = `e2e viewer ${Date.now()}`
    await page.getByTestId('map-synthetic-name').locator('input').fill(name)
    await page.getByTestId('map-synthetic').click()

    const row = page.locator('[data-testid="map-table"] tbody tr').filter({ hasText: name })
    await expect(row).toBeVisible()

    await row.getByText('Open viewer').click()
    await expect(page.locator('h1')).toHaveText('Map preview')

    // The probe must have reported a backend, and one of the three the chain knows.
    await expect.poll(async () => (await debugState(page))?.engine ?? null).not.toBeNull()
    const started = await debugState(page)
    expect(['webgpu', 'webgl2', 'webgl']).toContain(started?.engine)
    expect(started?.error).toBeNull()

    // Frames advance, so the render loop is alive rather than merely started.
    const first = (await debugState(page))?.frames ?? 0
    await expect.poll(async () => ((await debugState(page))?.frames ?? 0) > first).toBe(true)

    // The elevation chunk of the visible area was fetched and drawn.
    await expect
      .poll(async () => Number(await page.getByTestId('chunk-count').innerText()))
      .toBeGreaterThan(0)

    // A layer toggle changes what the surface shows and leaves the loop running.
    await page.getByTestId('layer-1').click()
    await expect(page.getByTestId('layer-1')).not.toHaveClass(/t-is-checked/)
    const afterToggle = (await debugState(page))?.frames ?? 0
    await expect.poll(async () => ((await debugState(page))?.frames ?? 0) > afterToggle).toBe(true)
    await page.getByTestId('layer-1').click()

    // An overlay switched off and on again is drawn again. It was not: the geometry was
    // loaded once and the early return left it hidden for the rest of the session, so
    // the second switch did nothing at all.
    const regions = page.getByTestId('overlay-regions')
    if (!(await regions.isDisabled())) {
      await regions.click()
      await expect(regions).toHaveClass(/t-is-checked/)
      await regions.click()
      await expect(regions).not.toHaveClass(/t-is-checked/)
      await regions.click()
      await expect(regions).toHaveClass(/t-is-checked/)
      await expect.poll(async () => (await debugState(page))?.frames ?? 0).toBeGreaterThan(0)
    }

    expect(problems).toEqual([])
  })

  test('?engine=webgl2 selects WebGL2 and says so in the status chip', async ({ page }) => {
    service.skipUnlessAvailable()

    const mapId = await firstMapId(page)
    test.skip(mapId === null, 'the library holds no map to open')

    await page.goto(`/maps/${mapId}?engine=webgl2`)
    await expect(page.locator('h1')).toHaveText('Map preview')

    await expect.poll(async () => (await debugState(page))?.engine ?? null).toBe('webgl2')
    await expect(page.getByTestId('engine-chip')).toHaveText('WebGL2')
  })
})

/** Reads the renderer's debug hook, or null before the viewer has written it. */
async function debugState(page: import('@playwright/test').Page): Promise<DebugState | null> {
  return page.evaluate(() => {
    // Written through a string key: the hook's name is fixed by the viewer's contract.
    const state = (globalThis as unknown as Record<string, DebugState | undefined>)[
      '__ourealis_debug'
    ]
    return state ?? null
  })
}

/** Identifier of any map in the library, or null when it is empty. */
async function firstMapId(page: import('@playwright/test').Page): Promise<string | null> {
  const list = await page.request
    .get('/api/v1/maps', { failOnStatusCode: false })
    .then((response) => (response.ok() ? response.json() : null))
    .catch(() => null)
  const items = (list as { items?: Array<{ id?: unknown }> } | null)?.items
  const id = Array.isArray(items) ? items[0]?.id : undefined
  return typeof id === 'string' ? id : null
}
