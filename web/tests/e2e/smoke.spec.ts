import { test, expect } from '@playwright/test'
import { serviceGate } from '../support/service'

/**
 * The lane runs against a real service: the shell renders without one, but the
 * views it opens do not, and a lane that skipped its way to a green run would be
 * reporting a stack it never touched. An absent service fails it by default; see
 * `tests/support/service.ts` for the reason and for `OUREALIS_ALLOW_SKIP=1`.
 */
const service = serviceGate()

test.describe('application shell', () => {
  test.beforeAll(async ({ request }) => {
    await service.probe(request)
  })

  test('the root route renders the shell and the default view', async ({ page }) => {
    service.skipUnlessAvailable()

    await page.goto('/')

    await expect(page).toHaveTitle('Ourealis')
    await expect(page.locator('h1')).toHaveText('Overview')
    await expect(page.locator('nav a')).toHaveCount(6)
    await expect(page.locator('html')).toHaveAttribute('lang', 'en')
  })

  test('rendering logs no console error and raises no page error', async ({ page }) => {
    service.skipUnlessAvailable()

    const problems: string[] = []
    page.on('console', (message) => {
      if (message.type() === 'error') {
        problems.push(message.text())
      }
    })
    page.on('pageerror', (error) => problems.push(error.message))

    await page.goto('/')
    await expect(page.locator('h1')).toHaveText('Overview')
    await page.waitForLoadState('networkidle')

    expect(problems).toEqual([])
  })

  test('the theme toggle reaches both dark selectors and survives a reload', async ({ page }) => {
    service.skipUnlessAvailable()

    await page.goto('/')
    const root = page.locator('html')
    await expect(root).toHaveAttribute('theme-mode', 'light')

    await page.getByRole('button', { name: 'Dark' }).click()

    // Tailwind reads the class, tdesign reads the attribute, and the token must be the dark green.
    await expect(root).toHaveClass(/(^|\s)dark(\s|$)/)
    await expect(root).toHaveAttribute('theme-mode', 'dark')
    const brand = await root.evaluate((element) =>
      getComputedStyle(element).getPropertyValue('--td-brand-color').trim(),
    )
    expect(brand).toBe('#5aa46a')

    await page.reload()
    await expect(root).toHaveAttribute('theme-mode', 'dark')
  })

  test('switching the language changes the shell text and survives a reload', async ({ page }) => {
    service.skipUnlessAvailable()

    await page.goto('/')
    await expect(page.locator('h1')).toHaveText('Overview')

    await page.locator('.t-select-input').click()
    await page.getByText('简体中文', { exact: true }).click()

    await expect(page.locator('h1')).toHaveText('总览')
    await expect(page.locator('html')).toHaveAttribute('lang', 'zh-CN')

    await page.reload()
    await expect(page.locator('h1')).toHaveText('总览')
  })
})
