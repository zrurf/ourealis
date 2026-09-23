import { expect, test } from '@playwright/test'
import { serviceGate } from '../support/service'
import { buildSyntheticMap } from '../support/tasks'

const service = serviceGate()

/**
 * A point the map refuses is reported at once.
 *
 * The page cannot know that a click landed inside a building — the hard mask lives in
 * the map and the page only draws the surface — so the workspace asks the service as the
 * point is placed. Without this the reader sees a failed plan a minute later and has no
 * way to tell which point caused it.
 */
test('a point the map refuses is reported where it was placed', async ({ page, request }) => {
  await service.probe(request)
  service.skipUnlessAvailable()
  await page.addInitScript(() => globalThis.localStorage.setItem('ourealis.locale', 'en'))
  const name = `e2e feasibility ${Date.now()}`
  const mapId = await buildSyntheticMap(
    request,
    { preset: 'compact', seed: 4242, with_kpath_library: false },
    name,
  )
  await page.setViewportSize({ width: 1600, height: 950 })
  await page.goto('/run')
  await expect(page.getByTestId('run-ready')).toBeAttached({ timeout: 60_000 })

  // The workspace defaults to the library's first map, which is not necessarily the one
  // this test built; the map stage is where that is chosen.
  await page.getByTestId('stage-map').click()
  await page.getByTestId('run-map').click()
  await page.locator('.t-select-option').filter({ hasText: name }).first().click()
  await page.getByTestId('stage-route').click()
  await expect(page.getByTestId('run-ready')).toBeAttached({ timeout: 60_000 })

  // The fixture's lake, typed into the start fields: the numeric path is a first-class
  // way to place a point, and it lands exactly where the test says.
  await fillNumber(page, 'start-x', 234)
  await fillNumber(page, 'start-y', 156)
  await page.waitForTimeout(1200)
  await expect(page.getByTestId('route-refused')).toBeVisible({ timeout: 30_000 })
  await expect(page.getByTestId('route-refused')).toContainText('blocked')
  await page.screenshot({ path: '.tmp/screens/feasibility.png' })

  // Moving it onto ground the map allows clears the report. Which point that is depends
  // on the seed the buildings were drawn from, so it is asked of the service rather than
  // assumed.
  const open = await legalPoint(request, mapId)
  expect(open, 'the fixture map must have a passable point').not.toBeNull()
  await fillNumber(page, 'start-x', open?.x ?? 0)
  await fillNumber(page, 'start-y', open?.y ?? 0)
  await expect(page.getByTestId('route-refused')).toHaveCount(0, { timeout: 30_000 })
})

/** A point of the map the service calls passable, or `null` if there is none. */
async function legalPoint(
  request: import('@playwright/test').APIRequestContext,
  id: string,
): Promise<{ x: number; y: number } | null> {
  const reply = await request.post(`/api/v1/maps/${id}/feasibility`, {
    data: {
      points: [
        { x: 36, y: 100 },
        { x: 60, y: 60 },
        { x: 150, y: 100 },
        { x: 264, y: 100 },
      ],
    },
  })
  const items = (
    (await reply.json()) as {
      items: Array<{ point: { x: number; y: number }; legal: boolean }>
    }
  ).items
  return items.find((item) => item.legal)?.point ?? null
}

/**
 * Sets a numeric field.
 *
 * The field is selected before it is filled: tdesign's number input keeps its own value
 * and appends what is typed to it, so `fill` alone turned "234" into "23436" — a
 * coordinate far outside the map, which is exactly what the test then reported.
 */
async function fillNumber(
  page: import('@playwright/test').Page,
  testId: string,
  value: number,
): Promise<void> {
  const input = page.getByTestId(testId).locator('input')
  await input.click()
  await input.press('ControlOrMeta+a')
  await input.press('Delete')
  await input.type(String(value))
  await input.blur()
}
