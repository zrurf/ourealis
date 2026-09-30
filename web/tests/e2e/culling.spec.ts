/*
 * Frustum culling, against a real map in a real browser.
 *
 * The unit lane proves the arithmetic; this proves the thing that actually matters, which
 * is that a map drawn with culling is indistinguishable from a map drawn without it. A
 * culler that drops a visible chunk produces a hole, and a hole looks exactly like a
 * rendering fault — so these assertions are about what is *still on screen* after the
 * camera has moved, not about counters.
 */
import { expect, test } from '@playwright/test'
import { serviceGate } from '../support/service'

/** The debug hook the viewer publishes. */
interface DebugState {
  engine: string | null
  frames: number
  loaded: boolean
  error: string | null
  culling: {
    active: boolean
    culled: number
    tracked: number
    passes: number
    rescues: number
    triangles: number
  }
}

const service = serviceGate()

/** The debug hook, read through its fixed name. */
async function debugState(page: import('@playwright/test').Page): Promise<DebugState | null> {
  return page.evaluate(() => {
    const state = (globalThis as unknown as Record<string, DebugState | undefined>)[
      '__ourealis_debug'
    ]
    return state ?? null
  })
}

/** What the library says about one map, as far as chunk count can be read off it. */
interface MapSummary {
  id: string
  bounds: { min_x: number; min_y: number; max_x: number; max_y: number }
  base_res_m: number
  chunk_size: number
}

/** Chunks the finest level of a map would hold, from its summary alone. */
function estimatedChunks(map: MapSummary): number {
  const width = (map.bounds.max_x - map.bounds.min_x) / Math.max(1e-6, map.base_res_m)
  const depth = (map.bounds.max_y - map.bounds.min_y) / Math.max(1e-6, map.base_res_m)
  const across = Math.ceil(width / Math.max(1, map.chunk_size))
  const down = Math.ceil(depth / Math.max(1, map.chunk_size))
  return across * down
}

/**
 * Opens the largest map in the library, or skips when there is not one.
 *
 * Largest rather than any: a culler that never has anything to decide is a culler whose
 * every assertion passes for the wrong reason. The synthetic maps the other lanes leave
 * behind hold a handful of chunks each, far below the threshold at which culling engages,
 * so a test built on one of those would prove nothing. The candidate is chosen from the
 * library listing rather than by scanning every map, because the listing grows with every
 * test run and scanning it one request at a time is a minute of waiting.
 */
async function openLargestMap(page: import('@playwright/test').Page): Promise<string> {
  const list = await page.request
    .get('/api/v1/maps?limit=500', { failOnStatusCode: false })
    .then((response) => (response.ok() ? response.json() : null))
  const items = ((list as { items?: MapSummary[] } | null)?.items ?? []).filter(
    (entry) => typeof entry.id === 'string' && entry.chunk_size > 0,
  )
  test.skip(items.length === 0, 'the library holds no map to open')
  const best = items.toSorted((a, b) => estimatedChunks(b) - estimatedChunks(a))[0]
  const chunks = best === undefined ? 0 : estimatedChunks(best)
  test.skip(
    chunks < 64,
    `no map in the library is large enough to cull (largest holds ${String(chunks)} chunks)`,
  )
  return String(best?.id)
}

/**
 * How much of the canvas is ground rather than sky.
 *
 * The cheapest available proxy for "the surface is drawn": a culler that emptied the view
 * would leave the background, and a culler that punched a hole would take a bite out of
 * the middle. It is a coarse measure on purpose — the point is that a large drop fails
 * loudly, not that it is measured exactly.
 */
async function paintedFraction(page: import('@playwright/test').Page): Promise<number> {
  return page.evaluate(() => {
    // The test id is on the element the canvas is *mounted into* — the canvas itself is
    // created by the engine and lives inside it, which is what makes lending one engine
    // between views possible at all.
    const canvas = document.querySelector<HTMLCanvasElement>('[data-testid="viewer-canvas"] canvas')
    if (canvas === null) {
      return 0
    }
    const scratch = document.createElement('canvas')
    const width = 240
    const height = 150
    scratch.width = width
    scratch.height = height
    const context = scratch.getContext('2d')
    if (context === null) {
      return 0
    }
    context.drawImage(canvas, 0, 0, width, height)
    const { data } = context.getImageData(0, 0, width, height)
    let painted = 0
    for (let index = 0; index < data.length; index += 4) {
      // The background is a near-neutral dark; anything with colour or a different
      // luminance band is ground, a drape or an overlay.
      const spread =
        Math.max(data[index] ?? 0, data[index + 1] ?? 0, data[index + 2] ?? 0) -
        Math.min(data[index] ?? 0, data[index + 1] ?? 0, data[index + 2] ?? 0)
      if (spread > 8 || (data[index] ?? 0) > 90) {
        painted += 1
      }
    }
    return painted / (width * height)
  })
}

test.describe('frustum culling', () => {
  test.beforeAll(async ({ request }) => {
    await service.probe(request)
  })

  test('a culled map draws the same ground as an unculled one', async ({ page }) => {
    await page.addInitScript(() => globalThis.localStorage.setItem('ourealis.locale', 'en'))

    const id = await openLargestMap(page)
    await page.goto(`/maps/${id}`)
    await expect(page.getByTestId('engine-chip')).not.toHaveText('', { timeout: 30_000 })
    await expect
      .poll(async () => Number(await page.getByTestId('chunk-count').innerText()))
      .toBeGreaterThan(0)
    // Let the surface finish streaming before either reading is taken.
    await page.waitForTimeout(2500)
    const withCulling = await paintedFraction(page)

    // The same map, the same camera, with the culler switched off by the query flag.
    await page.goto(`${page.url()}${page.url().includes('?') ? '&' : '?'}nocull=1`)
    await expect(page.getByTestId('engine-chip')).not.toHaveText('', { timeout: 30_000 })
    await expect
      .poll(async () => Number(await page.getByTestId('chunk-count').innerText()))
      .toBeGreaterThan(0)
    await page.waitForTimeout(2500)
    const withoutCulling = await paintedFraction(page)

    expect(withCulling).toBeGreaterThan(0.05)
    // The two renderings are the same scene, so the amount of ground on screen has to
    // match. A culler that dropped a visible chunk would show here as a smaller number.
    expect(Math.abs(withCulling - withoutCulling)).toBeLessThan(0.06)
  })

  test('panning to a corner never leaves the reader looking at nothing', async ({ page }) => {
    await page.addInitScript(() => globalThis.localStorage.setItem('ourealis.locale', 'en'))
    const id = await openLargestMap(page)
    await page.goto(`/maps/${id}`)
    await expect(page.getByTestId('engine-chip')).not.toHaveText('', { timeout: 30_000 })
    await page.waitForTimeout(2500)

    const box = await page.getByTestId('viewer-canvas').boundingBox()
    const cx = (box?.x ?? 0) + (box?.width ?? 0) / 2
    const cy = (box?.y ?? 0) + (box?.height ?? 0) / 2

    // A drag in each direction, asserting the surface is still there after each. The
    // ground under the camera is protected explicitly, so this is the case that a
    // culler protecting only its own bounds would get wrong.
    const drags: Array<[number, number]> = [
      [400, 260],
      [-800, -520],
      [600, -400],
    ]
    for (const [dx, dy] of drags) {
      await page.mouse.move(cx, cy)
      await page.mouse.down({ button: 'middle' })
      await page.mouse.move(cx + dx, cy + dy, { steps: 12 })
      await page.mouse.up({ button: 'middle' })
      await page.waitForTimeout(1200)
      expect(await paintedFraction(page)).toBeGreaterThan(0.05)
    }
  })

  test('culling engages on a real map and culls only what is out of view', async ({ page }) => {
    await page.addInitScript(() => globalThis.localStorage.setItem('ourealis.locale', 'en'))
    const id = await openLargestMap(page)
    await page.goto(`/maps/${id}`)
    await expect(page.getByTestId('engine-chip')).not.toHaveText('', { timeout: 30_000 })
    await page.waitForTimeout(2500)

    const box = await page.getByTestId('viewer-canvas').boundingBox()
    const cx = (box?.x ?? 0) + (box?.width ?? 0) / 2
    const cy = (box?.y ?? 0) + (box?.height ?? 0) / 2

    // Sampled over the pan rather than read once at the end, because the surface is
    // rebuilt when the level changes and a reading taken afterwards says nothing about
    // whether the culler ever did its work.
    let bestCulled = 0
    let bestTracked = 0
    const sample = async (): Promise<void> => {
      const culling = (await debugState(page))?.culling
      if (culling === undefined) {
        return
      }
      bestCulled = Math.max(bestCulled, culling.culled)
      bestTracked = Math.max(bestTracked, culling.tracked)
    }

    const swings: Array<[number, number]> = [
      [300, 200],
      [-700, -460],
      [500, 300],
    ]
    for (const [dx, dy] of swings) {
      await page.mouse.move(cx, cy)
      await page.mouse.down({ button: 'middle' })
      await page.mouse.move(cx + dx, cy + dy, { steps: 10 })
      await page.mouse.up({ button: 'middle' })
      for (let step = 0; step < 4; step += 1) {
        await page.waitForTimeout(650)
        await sample()
        // Ground has to be on screen at every sample, not only at the end: a culler that
        // empties the view and refills it would pass a single reading at the end.
        expect(await paintedFraction(page)).toBeGreaterThan(0.05)
      }
    }

    // The whole point of the test: the culler has to have engaged and dropped something.
    // Without this every other assertion here would pass on a culler that never ran.
    expect(bestTracked).toBeGreaterThan(0)
    expect(bestCulled).toBeGreaterThan(0)
  })

  test('the escape hatch puts every chunk back', async ({ page }) => {
    await page.addInitScript(() => globalThis.localStorage.setItem('ourealis.locale', 'en'))
    const id = await openLargestMap(page)
    await page.goto(`/maps/${id}?nocull=1`)
    await expect(page.getByTestId('engine-chip')).not.toHaveText('', { timeout: 30_000 })
    await page.waitForTimeout(3000)
    const state = await debugState(page)
    // With the flag set the culler is not merely inactive by accident: nothing is off.
    expect(state?.culling.culled ?? 0).toBe(0)
  })
})
