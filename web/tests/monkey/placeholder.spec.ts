import { createPinia, setActivePinia } from 'pinia'
import { expect, test } from '@playwright/test'
import { nextTick } from 'vue'
import { i18n, SUPPORTED_LOCALES } from '../../src/locales'
import { useLocaleStore } from '../../src/stores/locale'
import { SUPPORTED_THEMES, useThemeStore } from '../../src/stores/theme'

test.describe.configure({ mode: 'serial' })

/** Deterministic 32-bit generator, so a failing operation sequence replays from the seed alone. */
function mulberry32(seed: number): () => number {
  let state = seed >>> 0
  return () => {
    state = (state + 0x6d2b79f5) >>> 0
    let t = state
    t = Math.imul(t ^ (t >>> 15), t | 1)
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61)
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296
  }
}

test.describe('store monkey', () => {
  test('a random theme and locale operation sequence keeps the invariants', async () => {
    setActivePinia(createPinia())
    const theme = useThemeStore()
    const locale = useLocaleStore()
    const random = mulberry32(0x5eed_1234)
    const dashboardTitles = new Set<string>()

    for (let step = 0; step < 500; step += 1) {
      const action = random()
      if (action < 0.4) {
        theme.toggleTheme()
      } else if (action < 0.6) {
        theme.setTheme(random() < 0.5 ? 'light' : 'dark')
      } else if (action < 0.9) {
        locale.setLocale(random() < 0.5 ? 'en' : 'zh-CN')
      }

      // Both stores publish through watchers, which Vue flushes on the microtask queue.
      await nextTick()

      expect(SUPPORTED_THEMES).toContain(theme.theme)
      expect(theme.isDark).toBe(theme.theme === 'dark')
      expect(SUPPORTED_LOCALES).toContain(locale.locale)
      expect(i18n.global.locale.value).toBe(locale.locale)

      const title = i18n.global.t('nav.dashboard')
      expect(title.trim()).not.toBe('')
      dashboardTitles.add(title)
    }

    // Both catalogs answered over the run, which means the locale switch reached the composer.
    expect(dashboardTitles.size).toBe(2)
  })

  test('language options follow the active locale', async () => {
    setActivePinia(createPinia())
    const locale = useLocaleStore()

    locale.setLocale('en')
    await nextTick()
    expect(locale.options.map((option) => option.label)).toEqual(['English', '简体中文'])

    locale.setLocale('zh-CN')
    await nextTick()
    expect(locale.options.map((option) => option.value)).toEqual([...SUPPORTED_LOCALES])
  })
})
