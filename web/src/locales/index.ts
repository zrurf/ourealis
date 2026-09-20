import { createI18n } from 'vue-i18n'
import en from './en'
import zhCN from './zh-CN'

/** Locales the interface ships; `en` is both the default and the fallback. */
export const SUPPORTED_LOCALES = ['en', 'zh-CN'] as const

/** A locale the interface has a catalog for. */
export type LocaleName = (typeof SUPPORTED_LOCALES)[number]

/** Browser language tags that select the Chinese catalog: `zh`, `zh-CN`, `zh-Hans`, `zh-TW`. */
const CHINESE_TAG = /^zh\b/i

/**
 * Narrows an untrusted value — a persisted setting, a `navigator.language` tag, a
 * select payload — to a locale this app can render.
 */
export function isSupportedLocale(value: unknown): value is LocaleName {
  return typeof value === 'string' && (SUPPORTED_LOCALES as readonly string[]).includes(value)
}

/**
 * Resolves the locale to start in: the stored user choice first, then the browser
 * language (`zh*` selects Chinese, everything else English), then English.
 *
 * Storage access is wrapped because private-mode browsers and the node-side test
 * lanes have no `localStorage`, and a missing preference is not a failure.
 */
export function detectLocale(): LocaleName {
  try {
    const stored = globalThis.localStorage?.getItem('ourealis.locale')
    if (isSupportedLocale(stored)) {
      return stored
    }
  } catch {
    // Storage is unavailable; fall through to the browser language.
  }
  const tag = typeof navigator === 'undefined' ? undefined : navigator.language
  return typeof tag === 'string' && CHINESE_TAG.test(tag) ? 'zh-CN' : 'en'
}

export const i18n = createI18n({
  legacy: false,
  globalInjection: true,
  locale: detectLocale(),
  fallbackLocale: 'en',
  messages: { en, 'zh-CN': zhCN },
})
