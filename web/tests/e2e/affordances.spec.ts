/*
 * The interface's affordances, end to end: the things the eye needs to find its way.
 *
 * These are the interactions a reader reaches for without being told — the way back to the
 * library, folding the sidebar, the viewport rail, the mode switch that promises a shorter form,
 * and the middle button that pans every map they have ever used. Each is cheap to assert and each
 * was a real complaint, so each gets a test rather than a screenshot.
 */
import { expect, test } from '@playwright/test'
import { serviceGate } from '../support/service'

const service = serviceGate()

/**
 * Puts a start point on the map.
 *
 * The later stages are gated on a route: the workspace will not open the runner before it knows
 * where the run begins, so a test that wants that stage has to draw the first point.
 */
async function placeStart(page: import('@playwright/test').Page): Promise<void> {
  const box = await page.getByTestId('map-canvas').boundingBox()
  const cx = (box?.x ?? 0) + (box?.width ?? 0) / 2
  const cy = (box?.y ?? 0) + (box?.height ?? 0) / 2
  await page.mouse.click(cx - 60, cy)
  await page.waitForTimeout(500)
}

/** Path of the first map in the library, or a skipped test when there is none. */
async function anyMapPath(page: import('@playwright/test').Page): Promise<string> {
  const list = await page.request
    .get('/api/v1/maps', { failOnStatusCode: false })
    .then((response) => (response.ok() ? response.json() : null))
  const id = (list as { items?: Array<{ id?: string }> } | null)?.items?.[0]?.id
  test.skip(typeof id !== 'string', 'the library holds no map to open')
  return `/maps/${String(id)}`
}

test.describe('getting around', () => {
  test.beforeAll(async ({ request }) => {
    await service.probe(request)
  })

  test('a map opened from the library leads back to it', async ({ page }) => {
    service.skipUnlessAvailable()
    await page.addInitScript(() => globalThis.localStorage.setItem('ourealis.locale', 'en'))
    await page.goto(await anyMapPath(page))
    await expect(page.getByTestId('viewer-back')).toBeVisible()
    await page.getByTestId('viewer-back').click()
    await expect(page).toHaveURL(/\/maps$/)
  })

  test('the sidebar folds to its icons and remembers it', async ({ page }) => {
    service.skipUnlessAvailable()
    await page.addInitScript(() => globalThis.localStorage.setItem('ourealis.locale', 'en'))
    await page.goto('/')
    const sidebar = page.getByTestId('app-sidebar')
    const expanded = await sidebar.boundingBox()
    await page.getByTestId('nav-toggle').click()
    await expect
      .poll(async () => (await sidebar.boundingBox())?.width ?? 0)
      .toBeLessThan(expanded?.width ?? 0)
    // Folding is a working preference: it survives a reload.
    await page.reload()
    await expect
      .poll(async () => (await sidebar.boundingBox())?.width ?? 0)
      .toBeLessThan(expanded?.width ?? 0)
    await page.getByTestId('nav-toggle').click()
    await expect.poll(async () => (await sidebar.boundingBox())?.width ?? 0).toBe(expanded?.width)
  })

  test('the viewport rail opens a panel and shows what the camera is doing', async ({ page }) => {
    service.skipUnlessAvailable()
    await page.addInitScript(() => globalThis.localStorage.setItem('ourealis.locale', 'en'))
    await page.goto(await anyMapPath(page))
    await expect(page.getByTestId('engine-chip')).not.toHaveText('', { timeout: 30_000 })
    await page.getByTestId('viewport-camera').click()
    await expect(page.getByTestId('viewport-panel-camera')).toBeVisible()
    // The panel reads the camera: dragging the view changes the pitch it reports.
    const pitch = page.getByTestId('viewport-pitch-value')
    const before = await pitch.innerText()
    const box = await page.getByTestId('viewer-canvas').boundingBox()
    const cx = (box?.x ?? 0) + (box?.width ?? 0) / 2
    const cy = (box?.y ?? 0) + (box?.height ?? 0) / 2
    await page.mouse.move(cx, cy)
    await page.mouse.down()
    await page.mouse.move(cx, cy - 120, { steps: 8 })
    await page.mouse.up()
    await expect.poll(async () => pitch.innerText()).not.toBe(before)
  })

  test('the middle button pans the map, and the reset brings it back', async ({ page }) => {
    service.skipUnlessAvailable()
    await page.addInitScript(() => globalThis.localStorage.setItem('ourealis.locale', 'en'))
    await page.goto(await anyMapPath(page))
    await expect(page.getByTestId('engine-chip')).not.toHaveText('', { timeout: 30_000 })
    const box = await page.getByTestId('viewer-canvas').boundingBox()
    const cx = (box?.x ?? 0) + (box?.width ?? 0) / 2
    const cy = (box?.y ?? 0) + (box?.height ?? 0) / 2
    // The cell under one fixed pixel is the reading: panning moves the ground beneath it.
    await page.mouse.click(cx, cy)
    await expect(page.getByTestId('inspector-position')).toBeVisible()
    const before = await page.getByTestId('inspector-position').innerText()
    // A drag that leaves the map under the pointer: the middle button, which every map pans with.
    await page.mouse.move(cx, cy)
    await page.mouse.down({ button: 'middle' })
    await page.mouse.move(cx + 70, cy + 45, { steps: 10 })
    await page.mouse.up({ button: 'middle' })
    await page.mouse.click(cx, cy)
    await expect
      .poll(async () => page.getByTestId('inspector-position').innerText())
      .not.toBe(before)
    // And the reset puts the map's own framing back, so the same pixel reads the same cell.
    await page.getByTestId('viewport-reset-view').click()
    await page.mouse.click(cx, cy)
    await expect.poll(async () => page.getByTestId('inspector-position').innerText()).toBe(before)
  })
})

test.describe('the two levels of detail', () => {
  test.beforeAll(async ({ request }) => {
    await service.probe(request)
  })

  test('expert mode brings the per-individual parameters back', async ({ page }) => {
    service.skipUnlessAvailable()
    await page.addInitScript(() => {
      globalThis.localStorage.setItem('ourealis.locale', 'en')
      globalThis.localStorage.setItem('ourealis.mode', 'simple')
    })
    await page.goto('/run')
    await expect(page.getByTestId('run-ready')).toBeAttached({ timeout: 60_000 })
    await placeStart(page)
    await page.getByTestId('stage-runner').click()
    // Simple mode: the preset and the individual, and none of the twenty-odd overrides.
    await expect(page.getByTestId('person-form')).toBeVisible()
    expect(await page.locator('[data-testid^="override-"]').count()).toBe(0)
    await page.getByTestId('mode-toggle').click()
    await expect
      .poll(async () => page.locator('[data-testid^="override-"]').count())
      .toBeGreaterThan(10)
  })

  test('the runner stage keeps its controls in one column', async ({ page }) => {
    service.skipUnlessAvailable()
    await page.addInitScript(() => globalThis.localStorage.setItem('ourealis.locale', 'en'))
    await page.goto('/run')
    await expect(page.getByTestId('run-ready')).toBeAttached({ timeout: 60_000 })
    await placeStart(page)
    await page.getByTestId('stage-runner').click()
    const panel = await page.getByTestId('stage-panel-runner').boundingBox()
    for (const testid of ['run-individual', 'run-seed']) {
      const field = await page.getByTestId(testid).boundingBox()
      // Inside the panel, and not the same row as the field before it: overlapping and
      // overflowing in x was the defect this asserts against.
      expect((field?.x ?? 0) + (field?.width ?? 0)).toBeLessThanOrEqual(
        (panel?.x ?? 0) + (panel?.width ?? 0) + 1,
      )
    }
    const individual = await page.getByTestId('run-individual').boundingBox()
    const seed = await page.getByTestId('run-seed').boundingBox()
    expect(seed?.y ?? 0).toBeGreaterThan((individual?.y ?? 0) + (individual?.height ?? 0))
  })
})

test.describe('the workspace viewport', () => {
  test.beforeAll(async ({ request }) => {
    await service.probe(request)
  })

  test('the workspace can switch the map sections on, like the preview can', async ({ page }) => {
    service.skipUnlessAvailable()
    await page.addInitScript(() => globalThis.localStorage.setItem('ourealis.locale', 'en'))
    const problems: string[] = []
    page.on('pageerror', (error) => problems.push(error.message))
    await page.goto('/run')
    await expect(page.getByTestId('run-ready')).toBeAttached({ timeout: 60_000 })
    // The rail's display panel is where a 3D tool keeps the overlay switches, in the workspace too.
    await page.getByTestId('viewport-display').click()
    await expect(page.getByTestId('viewport-panel-display')).toBeVisible()
    const regions = page.getByTestId('viewport-overlay-regions')
    await expect(regions).toBeVisible()
    // A section the map does not carry is offered greyed out rather than missing. tdesign draws a
    // switch as a div, so the state is its class rather than a native `disabled`.
    await expect(page.getByTestId('viewport-overlay-prm')).toHaveClass(/t-is-disabled/)
    await regions.click()
    await expect(regions).toHaveClass(/t-is-checked/)
    // The family was read and drawn: the canvas keeps rendering and nothing threw.
    await expect.poll(async () => problems.length).toBe(0)
    await expect(page.getByTestId('map-canvas').locator('canvas')).toBeVisible()
    await regions.click()
    await expect(regions).not.toHaveClass(/t-is-checked/)
    // Switched on again after off: the geometry is already loaded and visibility is re-applied.
    await regions.click()
    await expect(regions).toHaveClass(/t-is-checked/)
    expect(problems).toEqual([])
  })
})
