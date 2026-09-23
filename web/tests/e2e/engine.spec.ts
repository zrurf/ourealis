import { expect, test } from '@playwright/test'
import { serviceGate } from '../support/service'

const service = serviceGate()

/**
 * One engine for the session.
 *
 * Every map-bearing view used to build its own engine, which meant a fresh graphics
 * device — and its hundreds of milliseconds — on each move between the preview, the
 * workspace and a trajectory. The host owns the canvas and moves it between views, so
 * this asserts the thing that makes that true: the canvas found in each view is the
 * same element, and the loop keeps advancing in both.
 */
test('one engine serves the map views', async ({ page, request }) => {
  await service.probe(request)
  service.skipUnlessAvailable()
  await page.addInitScript(() => globalThis.localStorage.setItem('ourealis.locale', 'en'))
  await page.setViewportSize({ width: 1600, height: 950 })
  const list = await request.get('/api/v1/maps')
  const items = (await list.json()) as { items: Array<{ id: string }> }
  const id = items.items[0]?.id
  expect(id).toBeTruthy()

  // Through the app's own navigation, not `page.goto`: a full load legitimately builds
  // a new engine, and the claim here is about moving between views in one session.
  await page.goto('/maps')
  const name = `e2e engine ${Date.now()}`
  await page.getByTestId('map-synthetic-name').locator('input').fill(name)
  await page.getByTestId('map-synthetic').click()
  const row = page.locator('[data-testid="map-table"] tbody tr').filter({ hasText: name })
  await expect(row).toBeVisible({ timeout: 60_000 })
  await row.getByText('Open viewer').click()
  await expect
    .poll(async () => Number(await page.getByTestId('chunk-count').innerText()))
    .toBeGreaterThan(0)
  const fromViewer = await canvasId(page)
  const framesInViewer = await frames(page)
  await page.waitForTimeout(500)
  expect(await frames(page)).toBeGreaterThan(framesInViewer)

  await page.getByRole('link', { name: 'New run' }).click()
  await expect(page.getByTestId('run-ready')).toBeAttached({ timeout: 60_000 })
  await expect(page.getByTestId('map-canvas').locator('canvas')).toBeVisible()
  const framesInWorkspace = await frames(page)
  await page.waitForTimeout(500)
  expect(await frames(page)).toBeGreaterThan(framesInWorkspace)

  // The same canvas in both views: the engine was lent, not rebuilt.
  expect(await canvasId(page)).toBe(fromViewer)

  // And the identity means something: a reload builds a new engine with a new canvas.
  await page.reload()
  await expect(page.getByTestId('run-ready')).toBeAttached({ timeout: 60_000 })
  expect(await canvasId(page)).not.toBe(fromViewer)
})

/** Identity of the canvas the renderer is drawing into. */
async function canvasId(page: import('@playwright/test').Page): Promise<string | null> {
  return page.evaluate(() => {
    // Read through a string key: the property name is part of the renderer's contract.
    const state = (
      globalThis as unknown as Record<string, { canvasId?: string | null } | undefined>
    )['__ourealis_debug']
    return state?.canvasId ?? null
  })
}

async function frames(page: import('@playwright/test').Page): Promise<number> {
  return page.evaluate(() => {
    // As above: the hook's name is fixed by the renderer's contract.
    const state = (globalThis as unknown as Record<string, { frames?: number } | undefined>)[
      '__ourealis_debug'
    ]
    return state?.frames ?? 0
  })
}
