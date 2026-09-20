import { defineStore } from 'pinia'
import { computed, ref, watch } from 'vue'
import { detectLocale, i18n, SUPPORTED_LOCALES, type LocaleName } from '@/locales'
import { writeStored } from '@/utils/storage'

const STORAGE_KEY = 'ourealis.locale'

/** Sets the `lang` attribute so screen readers and hyphenation follow the catalog in use. */
function applyToDocument(name: LocaleName): void {
  if (typeof document === 'undefined') {
    return
  }
  document.documentElement.lang = name
}

/**
 * Interface language. English is the default and the fallback, so a missing key
 * in the Chinese catalog renders English rather than an empty string.
 */
export const useLocaleStore = defineStore('locale', () => {
  const locale = ref<LocaleName>(detectLocale())
  const options = computed(() =>
    SUPPORTED_LOCALES.map((name) => ({
      value: name,
      label: i18n.global.t(name === 'zh-CN' ? 'language.zh-CN' : 'language.en'),
    })),
  )

  watch(
    locale,
    (name) => {
      i18n.global.locale.value = name
      applyToDocument(name)
    },
    { immediate: true },
  )

  /** Switches to the given catalog and remembers it. */
  function setLocale(name: LocaleName): void {
    locale.value = name
    writeStored(STORAGE_KEY, name)
  }

  return { locale, options, setLocale }
})
