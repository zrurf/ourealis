import { defineStore } from 'pinia'
import { computed, ref, watch } from 'vue'
import { readStored, writeStored } from '@/utils/storage'

/** Appearance values the app can render; everything else is rejected. */
export const SUPPORTED_THEMES = ['light', 'dark'] as const

/** An appearance value the app has tokens for. */
export type ThemeName = (typeof SUPPORTED_THEMES)[number]

const STORAGE_KEY = 'ourealis.theme'

/** Narrows an untrusted value — a persisted setting, a DOM attribute — to a theme name. */
export function isThemeName(value: unknown): value is ThemeName {
  return typeof value === 'string' && (SUPPORTED_THEMES as readonly string[]).includes(value)
}

/**
 * Resolves the appearance to start in: the stored user choice first, then the
 * operating system preference, then light.
 */
export function detectTheme(): ThemeName {
  const stored = readStored(STORAGE_KEY)
  if (isThemeName(stored)) {
    return stored
  }
  if (typeof window !== 'undefined' && window.matchMedia('(prefers-color-scheme: dark)').matches) {
    return 'dark'
  }
  return 'light'
}

/**
 * Mirrors the theme onto `<html>`: the `dark` class drives the Tailwind variant,
 * `theme-mode` drives the tdesign tokens. Absent in the node-side test lanes.
 */
function applyToDocument(name: ThemeName): void {
  if (typeof document === 'undefined') {
    return
  }
  const root = document.documentElement
  root.classList.toggle('dark', name === 'dark')
  root.setAttribute('theme-mode', name)
}

/**
 * Light/dark appearance. The initial value follows the system preference unless
 * the user has chosen before, and only an explicit choice is persisted, so an
 * untouched installation keeps tracking the system.
 */
export const useThemeStore = defineStore('theme', () => {
  const theme = ref<ThemeName>(detectTheme())
  const isDark = computed(() => theme.value === 'dark')

  watch(theme, applyToDocument, { immediate: true })

  /** Switches to the given appearance and remembers it. */
  function setTheme(name: ThemeName): void {
    theme.value = name
    writeStored(STORAGE_KEY, name)
  }

  /** Switches between light and dark and remembers the result. */
  function toggleTheme(): void {
    setTheme(theme.value === 'dark' ? 'light' : 'dark')
  }

  return { theme, isDark, setTheme, toggleTheme }
})
