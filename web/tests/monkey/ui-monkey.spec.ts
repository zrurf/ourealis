/**
 * Monkey lane: a random walk through the interface must not break it.
 *
 * The store-level monkey test in `placeholder.spec.ts` exercises store transitions;
 * this one drives a real browser: it visits routes in a random order, clicks, types,
 * scrolls, toggles the theme and switches the language, and after every step checks
 * the invariants a user would notice — the shell is still there, nothing was logged
 * as an error, no request failed unexpectedly, and the app never navigated away.
 *
 * The walk is seeded, and the seed plus the action log are printed on failure, so a
 * broken sequence replays exactly. Actions that fail because the element moved or
 * disappeared are counted as skipped rather than as failures: a random click racing
 * a re-render is not a defect.
 */

import { expect, test, type ConsoleMessage, type Page, type Response } from '@playwright/test'

/** Seed of the walk; override with `OUREALIS_MONKEY_SEED` to explore. */
const MONKEY_SEED = Number(process.env.OUREALIS_MONKEY_SEED ?? 0x5eed_2024)

/** Steps per route. Kept small: the lane runs on every acceptance pass. */
const STEPS_PER_ROUTE = 8

/**
 * Routes the walk may visit.
 *
 * Deliberately only routes whose data exists: a route that asks for an unknown
 * identifier answers with a 404, the browser logs "Failed to load resource…" for it,
 * and that notice would be indistinguishable here from a real console error. The
 * error states of unknown identifiers are asserted directly instead, in
 * `tests/e2e/simulation.spec.ts`.
 */
const ROUTES = ['/', '/maps', '/routes', '/batch', '/omf', '/settings']

/** Deterministic 32-bit generator, so the walk replays from the seed alone. */
function mulberry32(seed: number): () => number {
  let state = seed >>> 0
  return () => {
    state = (state + 0x6d2b79f5) >>> 0
    let value = state
    value = Math.imul(value ^ (value >>> 15), value | 1)
    value ^= value + Math.imul(value ^ (value >>> 7), value | 61)
    return ((value ^ (value >>> 14)) >>> 0) / 4294967296
  }
}

/** Everything a broken step is allowed to be, so an assertion failure is legible. */
interface Observation {
  consoleErrors: string[]
  pageErrors: string[]
  failedRequests: string[]
  serverErrors: string[]
}

function watch(page: Page): Observation {
  const seen: Observation = {
    consoleErrors: [],
    pageErrors: [],
    failedRequests: [],
    serverErrors: [],
  }
  page.on('console', (message: ConsoleMessage) => {
    if (message.type() === 'error') {
      seen.consoleErrors.push(message.text())
    }
  })
  page.on('pageerror', (error: Error) => {
    seen.pageErrors.push(error.message)
  })
  page.on('requestfailed', (request) => {
    // A cancelled request is the app aborting its own fetch on navigation, which is
    // correct behaviour, not a failure.
    const reason = request.failure()?.errorText ?? ''
    if (!reason.includes('ERR_ABORTED')) {
      seen.failedRequests.push(`${request.method()} ${request.url()} (${reason})`)
    }
  })
  page.on('response', (response: Response) => {
    if (response.status() >= 500) {
      seen.serverErrors.push(`${response.status()} ${response.url()}`)
    }
  })
  return seen
}

/** Selectors that are safe to click at random. */
const CLICKABLE = [
  'main button:visible:not([disabled])',
  'main a:visible',
  'main [role="button"]:visible',
  'header button:visible:not([disabled])',
  '.t-switch:visible',
  '.t-tabs__nav-item:visible',
  '.t-radio-button:visible',
]

/** Performs one random action and describes it for the log. */
async function act(page: Page, random: () => number): Promise<string> {
  const choice = random()
  if (choice < 0.7) {
    const selector = CLICKABLE[Math.floor(random() * CLICKABLE.length)]!
    const candidates = page.locator(selector)
    const count = await candidates.count()
    if (count === 0) {
      return `click: no candidate for ${selector}`
    }
    const index = Math.floor(random() * count)
    const target = candidates.nth(index)
    const label = ((await target.textContent()) ?? '').trim().slice(0, 40)
    try {
      await target.click({ timeout: 2_000, noWaitAfter: true })
      return `click ${selector}[${index}] ${JSON.stringify(label)}`
    } catch (error) {
      return `skipped click ${selector}[${index}]: ${(error as Error).message.split('\n')[0]}`
    }
  }
  if (choice < 0.85) {
    const inputs = page.locator('main input:visible:not([type="checkbox"]), main textarea:visible')
    const count = await inputs.count()
    if (count === 0) {
      return 'type: no input on this page'
    }
    const index = Math.floor(random() * count)
    const value = ['7', 'banana', '-3', '999999', '', '1e9'][Math.floor(random() * 6)]!
    try {
      await inputs.nth(index).fill(value, { timeout: 2_000 })
      return `type ${JSON.stringify(value)} into input[${index}]`
    } catch (error) {
      return `skipped type into input[${index}]: ${(error as Error).message.split('\n')[0]}`
    }
  }
  if (choice < 0.95) {
    await page.mouse.wheel(0, random() < 0.5 ? 400 : -400)
    return 'wheel'
  }
  await page.keyboard.press('Escape')
  return 'escape'
}

test.describe.configure({ mode: 'serial' })

test('a random walk through the interface keeps its invariants', async ({ page }) => {
  const random = mulberry32(MONKEY_SEED)
  const log: string[] = []
  const seen = watch(page)

  const order = [...ROUTES]
  // Fisher-Yates with the seeded generator: the visit order varies per seed and is
  // still reproducible.
  for (let index = order.length - 1; index > 0; index -= 1) {
    const swap = Math.floor(random() * (index + 1))
    ;[order[index], order[swap]] = [order[swap]!, order[index]!]
  }

  // The origin comes from the first navigation rather than from the configuration,
  // so the walk follows whatever base URL the lane was started with (the embedded
  // page, a preview server, or the service's own origin).
  await page.goto('/', { waitUntil: 'domcontentloaded' })
  const origin = new URL(page.url()).origin
  for (const route of order.slice(0, 4)) {
    await page.goto(new URL(route, origin).href, { waitUntil: 'domcontentloaded' })
    await expect(page.locator('#app, main').first()).toBeVisible({ timeout: 15_000 })
    log.push(`--- ${route}`)
    for (let step = 0; step < STEPS_PER_ROUTE; step += 1) {
      log.push(`  ${step}: ${await act(page, random)}`)
      // The shell must survive every step: a blank page after a click is a crash the
      // console listeners may not have caught (for example a Vue render error).
      await expect(
        page.locator('main'),
        `the shell disappeared after step ${step} on ${route} (seed ${MONKEY_SEED})\n${log.join('\n')}`,
      ).toBeVisible()
    }
    // Still inside the application: the walk must not have followed a link away.
    expect(new URL(page.url()).origin, 'the walk left the application').toBe(origin)
    expect(new URL(page.url()).pathname, 'the walk ended outside the router').not.toBe('')
  }

  const report = `seed ${MONKEY_SEED}\n${log.join('\n')}`
  expect(seen.pageErrors, `uncaught errors during the walk\n${report}`).toEqual([])
  expect(seen.consoleErrors, `console errors during the walk\n${report}`).toEqual([])
  expect(seen.failedRequests, `failed requests during the walk\n${report}`).toEqual([])
  expect(seen.serverErrors, `server faults during the walk\n${report}`).toEqual([])
})
