import { expect, test } from '@playwright/test'
import { isSupportedLocale, SUPPORTED_LOCALES } from '../../src/locales'
import { isThemeName, SUPPORTED_THEMES } from '../../src/stores/theme'

test.describe.configure({ mode: 'serial' })

/** Deterministic 32-bit generator, so a failing case is reproducible from the seed alone. */
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

/** Random string biased towards the characters the persisted values are built from. */
function randomToken(random: () => number): string {
  const alphabet = 'enNzh-Cc_UUI0123456789 .:'
  const length = Math.floor(random() * 12)
  let token = ''
  for (let index = 0; index < length; index += 1) {
    const position = Math.floor(random() * alphabet.length)
    token += alphabet[position] ?? ''
  }
  return token
}

/** A value drawn from the shapes localStorage, a DOM attribute or a select payload can hold. */
function randomValue(random: () => number): unknown {
  const roll = random()
  if (roll < 0.6) {
    return randomToken(random)
  }
  if (roll < 0.7) {
    return null
  }
  if (roll < 0.8) {
    return undefined
  }
  if (roll < 0.9) {
    return Math.floor(random() * 1000) - 500
  }
  if (roll < 0.95) {
    return random() < 0.5
  }
  return { toString: () => randomToken(random) }
}

test.describe('persisted state guards', () => {
  test('the guards accept exactly their closed sets and never throw', () => {
    const random = mulberry32(0x0f0f_0f0f)

    for (let step = 0; step < 5000; step += 1) {
      const value = randomValue(random)
      const locale = isSupportedLocale(value)
      const theme = isThemeName(value)

      expect(locale).toBe(
        typeof value === 'string' && (SUPPORTED_LOCALES as readonly string[]).includes(value),
      )
      expect(theme).toBe(
        typeof value === 'string' && (SUPPORTED_THEMES as readonly string[]).includes(value),
      )
    }
  })

  test('near misses of the supported values are rejected', () => {
    for (const value of [
      'EN',
      'En',
      'en-US',
      'zh',
      'zh_CN',
      ' zh-CN',
      'zh-CN ',
      'Dark',
      'light ',
    ]) {
      expect(isSupportedLocale(value), value).toBe(false)
      expect(isThemeName(value), value).toBe(false)
    }
    for (const value of SUPPORTED_LOCALES) {
      expect(isSupportedLocale(value), value).toBe(true)
    }
    for (const value of SUPPORTED_THEMES) {
      expect(isThemeName(value), value).toBe(true)
    }
  })
})
