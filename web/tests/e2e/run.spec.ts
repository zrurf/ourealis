import { expect, test } from '@playwright/test'
import { serviceGate } from '../support/service'

const service = serviceGate()

test('the workspace draws a route, plans it and starts the run', async ({ page, request }) => {
  await service.probe(request)
  service.skipUnlessAvailable()
  await page.addInitScript(() => globalThis.localStorage.setItem('ourealis.locale', 'en'))
  await page.setViewportSize({ width: 1600, height: 950 })
  await page.goto('/run')
  await expect(page.locator('h1')).toHaveText('Run workspace')
  // The stage rail is on the page and the route stage is open.
  await expect(page.getByTestId('stage-rail')).toBeVisible()
  await expect(page.getByTestId('stage-panel-route')).toBeVisible()
  // The default 300x200 compact map is loaded; place a start and a goal by clicking.
  // The surface has to be loaded before a click means anything: the extent a point is
  // checked against comes with it.
  await expect(page.getByTestId('run-ready')).toBeAttached({ timeout: 60_000 })
  // Click the middle of the canvas: `frameBounds` aimed the camera at the middle of the
  // map, so a correct picker reports the map's centre.
  const box = await page.getByTestId('map-canvas').boundingBox()
  const cx = (box?.x ?? 0) + (box?.width ?? 0) / 2
  const cy = (box?.y ?? 0) + (box?.height ?? 0) / 2
  await page.mouse.click(cx - 60, cy)
  await page.waitForTimeout(400)
  await page.mouse.click(cx + 90, cy + 90)
  await page.waitForTimeout(600)
  // The plan arrives on its own: no button, and the summary is the service's numbers.
  await expect(page.getByTestId('plan-summary')).toBeVisible({ timeout: 90_000 })
  await expect(page.getByTestId('summary-length')).not.toHaveText('0 m')
  await expect(page.getByTestId('plan-status')).toContainText('planned in')

  // The keyboard edits the same route: Tab cycles the handles and an arrow key moves
  // the selected one, which is what makes the workspace usable without a pointer.
  const lengthBefore = await page.getByTestId('summary-length').innerText()
  await page.keyboard.press('Tab')
  await expect(page.getByTestId('handle-selection')).not.toHaveText('')
  for (let press = 0; press < 12; press += 1) {
    await page.keyboard.press('ArrowUp')
  }
  await expect
    .poll(async () => page.getByTestId('summary-length').innerText(), { timeout: 30_000 })
    .not.toBe(lengthBefore)

  // The candidate table is the planner's own answer, and its choice is marked.
  await expect(page.getByTestId('candidate-0')).toBeVisible()
  await expect(page.getByTestId('candidate-0')).toContainText('chosen')

  // A stage other than the route keeps the same draft: the runner's preset is the one
  // the recipe set, which is what "one draft" means in practice.
  await page.getByTestId('stage-runner').click()
  await expect(page.getByTestId('person-form')).toBeVisible()
  await page.getByTestId('stage-run').click()
  await page.getByTestId('run-name').locator('input').fill('e2e workspace run')

  // Starting the run from here submits the route that was drawn on this page.
  await page.getByTestId('run-submit').click()
  await expect(page.getByTestId('simulation-view')).toBeVisible({ timeout: 60_000 })
  await expect(page.getByTestId('job-state')).not.toHaveText('')
})
