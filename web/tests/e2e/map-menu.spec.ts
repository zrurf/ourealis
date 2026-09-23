import { expect, test } from '@playwright/test'
import { serviceGate } from '../support/service'
import { buildSyntheticMap } from '../support/tasks'

const service = serviceGate()

/**
 * The context menu is how a route is edited without hunting for a control: right-click
 * the ground to place an end, right-click a handle to remove it or to change what a
 * waypoint does when the runner reaches it.
 */
test('the map menu places the ends and dismisses', async ({ page, request }) => {
  await service.probe(request)
  service.skipUnlessAvailable()
  await page.addInitScript(() => globalThis.localStorage.setItem('ourealis.locale', 'en'))
  const name = `e2e menu ${Date.now()}`
  await buildSyntheticMap(request, { preset: 'compact', seed: 11, with_kpath_library: false }, name)
  await page.setViewportSize({ width: 1600, height: 950 })
  await page.goto('/run')
  await expect(page.getByTestId('run-ready')).toBeAttached({ timeout: 60_000 })
  await page.getByTestId('stage-map').click()
  await page.getByTestId('run-map').click()
  await page.locator('.t-select-option').filter({ hasText: name }).first().click()
  await page.getByTestId('stage-route').click()
  await expect(page.getByTestId('run-ready')).toBeAttached({ timeout: 60_000 })

  const box = await page.getByTestId('map-canvas').boundingBox()
  const cx = (box?.x ?? 0) + (box?.width ?? 0) / 2
  const cy = (box?.y ?? 0) + (box?.height ?? 0) / 2

  // Right-click on the ground offers the route's ends.
  await page.mouse.click(cx, cy, { button: 'right' })
  await expect(page.getByTestId('map-menu')).toBeVisible()
  await page.getByTestId('menu-set-start').click()
  await page.waitForTimeout(400)
  await page.mouse.click(cx + 80, cy + 60, { button: 'right' })
  await page.getByTestId('menu-set-goal').click()
  await page.waitForTimeout(1200)
  await expect(page.getByTestId('plan-summary')).toBeVisible({ timeout: 60_000 })
  await page.screenshot({ path: '.tmp/screens/menu-route.png' })

  // The placed ends are in the draft, which is the whole point of the gesture.
  await expect(page.getByTestId('start-x').locator('input')).not.toHaveValue('')
  await expect(page.getByTestId('goal-x').locator('input')).not.toHaveValue('')

  // A waypoint placed the same way, then retagged through its own menu.
  await page.mouse.click(cx - 90, cy + 40, { button: 'right' })
  await page.getByTestId('menu-add-waypoint').click()
  await expect(page.getByTestId('waypoint-0')).toBeVisible()
  await page.mouse.click(cx - 90, cy + 40, { button: 'right' })
  await expect(page.getByTestId('map-menu')).toBeVisible()
  await expect(page.getByTestId('menu-remove')).toBeVisible()
  await page.getByTestId('menu-semantics-dwell').click()
  // Choosing "dwell" is what makes the standing time appear, which is how the panel
  // shows that the behaviour changed.
  await expect(page.getByTestId('waypoint-0-duration')).toBeVisible()

  // Escape dismisses without acting.
  await page.mouse.click(cx + 80, cy + 60, { button: 'right' })
  await expect(page.getByTestId('map-menu')).toBeVisible()
  await page.keyboard.press('Escape')
  await expect(page.getByTestId('map-menu')).toHaveCount(0)

  // An end is cleared rather than removed: the field goes back to empty, to be placed
  // again, while a waypoint leaves the list.
  await page.mouse.click(cx + 80, cy + 60, { button: 'right' })
  await page.getByTestId('menu-clear').click()
  await expect(page.getByTestId('goal-x').locator('input')).toHaveValue('')
  await expect(page.getByTestId('waypoint-0')).toBeVisible()
})
