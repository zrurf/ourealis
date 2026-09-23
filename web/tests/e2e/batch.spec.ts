/**
 * Population sweep, end to end.
 *
 * The sweep is the only feature that submits several runs at once and watches them
 * concurrently, so it is the place where the job queue, the event streams and the
 * aggregate charts meet. The test asks for two individuals — enough to exercise the
 * fan-out and the regression chart, small enough to finish inside a test run — and
 * checks that the table, the progress tag and both charts end up populated.
 *
 * An absent service fails the lane by default with the command that starts one;
 * `OUREALIS_ALLOW_SKIP=1` skips it with the same reason instead.
 */

import { expect, test, type Page } from '@playwright/test'
import en from '../../src/locales/en'
import { SERVICE_URL, serviceGate } from '../support/service'
import { buildSyntheticMap } from '../support/tasks'

/** Comfortably above two sequential runs on the compact map. */
const SWEEP_TIMEOUT_MS = 240_000

/** Whether this lane has a service, and what to say about it. */
const service = serviceGate()

test.beforeAll(async ({ request }) => {
  await service.probe(request)
})

/** Makes sure the library holds a map, so the form has something to select. */
async function ensureMap(request: import('@playwright/test').APIRequestContext): Promise<void> {
  const list = await request.get(`${SERVICE_URL}/api/v1/maps`)
  const body = (await list.json()) as { total: number }
  if (body.total > 0) {
    return
  }
  await buildSyntheticMap(request, { preset: 'compact', seed: 4242, with_kpath_library: false })
}

/** Fills a numeric field of the route form. */
async function fillNumber(page: Page, testId: string, value: number): Promise<void> {
  const input = page.getByTestId(testId).locator('input')
  await input.fill(String(value))
  await input.blur()
}

/** Reads the section text without depending on the exact translation. */
async function sectionText(page: Page, testId: string): Promise<string> {
  return (await page.getByTestId(testId).textContent()) ?? ''
}

test('a two-individual sweep fills the table and both charts', async ({ page, request }) => {
  service.skipUnlessAvailable()
  test.setTimeout(SWEEP_TIMEOUT_MS + 60_000)
  await ensureMap(request)

  const consoleErrors: string[] = []
  const serverErrors: string[] = []
  page.on('console', (message) => {
    if (message.type() === 'error') {
      consoleErrors.push(message.text())
    }
  })
  page.on('pageerror', (error) => consoleErrors.push(error.message))
  page.on('response', (response) => {
    if (response.status() >= 500) {
      serverErrors.push(`${response.status()} ${response.url()}`)
    }
  })

  // The header lookup below compares against the English catalog, so the page is
  // opened in English whatever an earlier lane left behind.
  await page.addInitScript(() => globalThis.localStorage.setItem('ourealis.locale', 'en'))
  await page.goto('/batch')
  await expect(page.getByTestId('batch-view')).toBeVisible()

  // Two individuals: the fan-out is exercised without a long run.
  const population = page.getByTestId('batch-population').locator('input')
  await population.fill('2')
  await population.blur()

  // The route is required, and the form deliberately has no default: a batch of
  // runs on an arbitrary pair of points would be a silently wrong result.
  await fillNumber(page, 'start-x', 20)
  await fillNumber(page, 'start-y', 20)
  await fillNumber(page, 'goal-x', 200)
  await fillNumber(page, 'goal-y', 120)

  // The form's own submit button, not whichever button happens to be first.
  await page.getByTestId('form-submit').click()

  // Rows appear as soon as the runs are created, so the wait has to be for the
  // sweep to *finish*: the charts below are derived from finished runs only.
  await expect(async () => {
    const rows = page.getByTestId('batch-table').locator('tbody tr')
    expect(await rows.count()).toBeGreaterThanOrEqual(2)
    const terminal = await page
      .getByTestId('batch-table')
      .locator('tbody tr')
      .evaluateAll(
        (trs) =>
          trs.filter((tr) => /succeeded|failed|cancelled/i.test(tr.textContent ?? '')).length,
      )
    expect(terminal, 'both runs must reach a terminal state').toBeGreaterThanOrEqual(2)
  }).toPass({ timeout: SWEEP_TIMEOUT_MS, intervals: [2_000] })

  const table = page.getByTestId('batch-table')
  // The table must carry the metrics of every finished run, not merely rows: the
  // column titles come from the catalog, so the check follows a renamed column, and
  // a cell that shows the placeholder instead of a number means the summary never
  // reached the row.
  const headers = await table.locator('thead th').allInnerTexts()
  const ratioColumn = headers.findIndex(
    (title) => title.trim() === en.simulation.batch.columnPathRatio,
  )
  const stateColumn = headers.findIndex((title) => title.trim() === en.simulation.batch.columnState)
  expect(
    ratioColumn,
    `the sweep table must have a path-ratio column: ${headers.join(' | ')}`,
  ).toBeGreaterThanOrEqual(0)
  expect(
    stateColumn,
    `the sweep table must have a state column: ${headers.join(' | ')}`,
  ).toBeGreaterThanOrEqual(0)
  const rows = table.locator('tbody tr')
  const rowCount = await rows.count()
  let finished = 0
  for (let index = 0; index < rowCount; index += 1) {
    const cells = rows.nth(index).locator('td')
    const state = ((await cells.nth(stateColumn).innerText()) ?? '').trim()
    if (!/succeeded/i.test(state)) {
      continue
    }
    finished += 1
    const ratio = ((await cells.nth(ratioColumn).innerText()) ?? '').trim()
    expect(
      ratio,
      `run ${index + 1} must report a numeric path ratio, got ${JSON.stringify(ratio)}`,
    ).toMatch(/^-?\d+([.,]\d+)?$/)
  }
  expect(finished, 'both finished runs must be in the table').toBe(2)

  // Both aggregate charts are rendered by echarts, which needs a canvas per chart.
  await expect(page.getByTestId('batch-frequency').locator('canvas')).toBeVisible()
  await expect(page.getByTestId('batch-cadence').locator('canvas')).toBeVisible()
  const fit = await sectionText(page, 'batch-fit')
  // The fit reports the slope and the R² of the least-squares line. A fit that could
  // not be computed renders the placeholder instead, which carries no number at all,
  // so requiring two numbers is what makes this an assertion about the values.
  const fitValues = fit.match(/-?\d+(?:[.,]\d+)?/g) ?? []
  expect(
    fitValues.length,
    `the cadence-speed fit must report a slope and an R², got ${JSON.stringify(fit)}`,
  ).toBeGreaterThanOrEqual(2)
  for (const value of fitValues) {
    expect(Number.isFinite(Number(value.replace(',', '.'))), `${value} must not be NaN`).toBe(true)
  }

  expect(serverErrors, 'the sweep must not produce server faults').toEqual([])
  expect(consoleErrors, 'the sweep must not log errors').toEqual([])
})
