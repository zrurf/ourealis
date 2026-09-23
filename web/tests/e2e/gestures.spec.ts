/*
 * Touch gestures.
 *
 * One finger orbits the map, two fingers pan and pinch it — the same three gestures a
 * reader has on a phone. They are Babylon's own handling, enabled in `render/scene.ts`;
 * what this asserts is the part that can silently fail: that the browser does not take
 * the gesture for itself (scrolling the page instead of moving the map) and that a pinch
 * really changes what the map shows.
 *
 * A pinch is dispatched through the DevTools protocol because Playwright's own touch API
 * taps; two simultaneous touches are what a pinch is.
 */
import { expect, test } from '@playwright/test'
import { serviceGate } from '../support/service'
import { buildSyntheticMap } from '../support/tasks'

const service = serviceGate()

test.use({ hasTouch: true })

test('a pinch changes what the map shows without scrolling the page', async ({ page, request }) => {
  await service.probe(request)
  service.skipUnlessAvailable()
  await page.addInitScript(() => globalThis.localStorage.setItem('ourealis.locale', 'en'))
  const name = `e2e gestures ${Date.now()}`
  await buildSyntheticMap(request, { preset: 'compact', seed: 7, with_kpath_library: false }, name)
  await page.setViewportSize({ width: 1200, height: 800 })
  await page.goto('/run')
  await expect(page.getByTestId('run-ready')).toBeAttached({ timeout: 60_000 })
  await page.getByTestId('stage-map').click()
  await page.getByTestId('run-map').click()
  await page.locator('.t-select-option').filter({ hasText: name }).first().click()
  await page.getByTestId('stage-route').click()
  await expect(page.getByTestId('run-ready')).toBeAttached({ timeout: 60_000 })

  const box = await page.getByTestId('map-canvas').boundingBox()
  const cx = Math.round((box?.x ?? 0) + (box?.width ?? 0) / 2)
  const cy = Math.round((box?.y ?? 0) + (box?.height ?? 0) / 2)

  // What the middle of the canvas points at before the gesture: asked of the map itself
  // by placing the start there and reading the coordinate back.
  const before = await centrePoint(page, cx, cy)
  expect(before).not.toBeNull()

  // Pinch out: two fingers starting 100 px apart, ending 300 px apart.
  const cdp = await page.context().newCDPSession(page)
  await cdp.send('Input.dispatchTouchEvent', {
    type: 'touchStart',
    touchPoints: [touch(1, cx - 50, cy), touch(2, cx + 50, cy)],
  })
  for (const step of [100, 150, 200]) {
    await cdp.send('Input.dispatchTouchEvent', {
      type: 'touchMove',
      touchPoints: [touch(1, cx - step, cy), touch(2, cx + step, cy)],
    })
    await page.waitForTimeout(60)
  }
  await cdp.send('Input.dispatchTouchEvent', { type: 'touchEnd', touchPoints: [] })
  await page.waitForTimeout(600)
  await page.screenshot({ path: '.tmp/screens/gesture-pinch.png' })

  // The page did not consume the gesture: it moved the map, not the document.
  expect(await page.evaluate(() => window.scrollY)).toBe(0)

  // And the map is showing something else at that pixel: the centre now points somewhere
  // else, because the camera is closer.
  const after = await centrePoint(page, cx, cy)
  expect(after).not.toBeNull()
  expect(after).not.toEqual(before)
})

/** One touch point of a gesture. */
function touch(
  id: number,
  x: number,
  y: number,
): {
  id: number
  x: number
  y: number
  radiusX: number
  radiusY: number
  force: number
} {
  return { id, x, y, radiusX: 2, radiusY: 2, force: 1 }
}

/**
 * World position of a canvas pixel, read back through the start field.
 *
 * A plain click places the route's first point, and the panel reports where it landed —
 * the same path a reader takes, and independent of the context menu (which a
 * touch-capable context delivers differently). Clearing the field afterwards is what
 * makes the next reading use the same field: an emptied coordinate is "not set".
 */
async function centrePoint(
  page: import('@playwright/test').Page,
  x: number,
  y: number,
): Promise<{ x: number; y: number } | null> {
  await page.mouse.click(x, y)
  await page.waitForTimeout(400)
  const read = async (testId: string): Promise<number> =>
    Number(await page.getByTestId(testId).locator('input').inputValue())
  const point = { x: await read('start-x'), y: await read('start-y') }
  for (const field of ['start-x', 'start-y']) {
    const input = page.getByTestId(field).locator('input')
    await input.click()
    await input.press('ControlOrMeta+a')
    await input.press('Delete')
    await input.blur()
  }
  await page.waitForTimeout(200)
  return Number.isFinite(point.x) && Number.isFinite(point.y) ? point : null
}
