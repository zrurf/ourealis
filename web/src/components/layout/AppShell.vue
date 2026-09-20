<script setup lang="ts">
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'
import { useRoute } from 'vue-router'
import { Button as TButton, Select as TSelect, Tooltip as TTooltip } from 'tdesign-vue-next'
import { isSupportedLocale } from '@/locales'
import { useLocaleStore } from '@/stores/locale'
import { useThemeStore } from '@/stores/theme'

/** A sidebar entry: a route a user reaches without an identifier in the path. */
interface NavItem {
  /** Target path, also the value the active check compares against. */
  path: string
  /** Catalog key of the entry's label. */
  labelKey: string
}

const NAV_ITEMS: readonly NavItem[] = [
  { path: '/', labelKey: 'nav.dashboard' },
  { path: '/maps', labelKey: 'nav.maps' },
  { path: '/routes', labelKey: 'nav.routes' },
  { path: '/batch', labelKey: 'nav.batch' },
  { path: '/omf', labelKey: 'nav.omf' },
  { path: '/settings', labelKey: 'nav.settings' },
]

const { t } = useI18n({ useScope: 'global' })
const route = useRoute()
const themeStore = useThemeStore()
const localeStore = useLocaleStore()

const themeAction = computed(() => (themeStore.isDark ? t('theme.light') : t('theme.dark')))

/** True when `path` is the current route or an ancestor of it. */
function isActive(path: string): boolean {
  return path === '/'
    ? route.path === '/'
    : route.path === path || route.path.startsWith(`${path}/`)
}

/** The select emits the raw option value; anything outside the catalogs is ignored. */
function onLocaleChange(value: unknown): void {
  if (isSupportedLocale(value)) {
    localeStore.setLocale(value)
  }
}
</script>

<template>
  <div class="flex min-h-screen bg-page text-ink">
    <aside class="flex w-56 shrink-0 flex-col border-r border-line bg-surface">
      <RouterLink to="/" class="px-5 py-4 text-base font-semibold text-ink">
        {{ t('app.title') }}
      </RouterLink>
      <nav class="flex flex-col gap-0.5 px-2 pb-4">
        <RouterLink
          v-for="item in NAV_ITEMS"
          :key="item.path"
          :to="item.path"
          class="rounded-control px-3 py-2 text-sm transition-colors duration-150 ease-out"
          :class="
            isActive(item.path)
              ? 'bg-page font-medium text-brand'
              : 'text-muted hover:bg-page hover:text-ink'
          "
        >
          {{ t(item.labelKey) }}
        </RouterLink>
      </nav>
    </aside>

    <div class="flex min-w-0 flex-1 flex-col">
      <header class="flex h-14 shrink-0 items-center justify-end gap-2 border-b border-line px-6">
        <!-- The icon is decorative: the button's accessible name is the action it
             performs, which is also what the tooltip shows. -->
        <TTooltip :content="themeAction">
          <TButton
            variant="text"
            shape="square"
            :aria-label="themeAction"
            @click="themeStore.toggleTheme()"
          >
            <svg
              v-if="themeStore.isDark"
              class="h-[18px] w-[18px]"
              viewBox="0 0 24 24"
              fill="none"
              stroke="currentColor"
              stroke-width="1.6"
              stroke-linecap="round"
              aria-hidden="true"
            >
              <circle cx="12" cy="12" r="4.1" />
              <path
                d="M12 2.8v2.1M12 19.1v2.1M2.8 12h2.1M19.1 12h2.1M5.5 5.5l1.5 1.5M17 17l1.5 1.5M18.5 5.5L17 7M7 17l-1.5 1.5"
              />
            </svg>
            <svg
              v-else
              class="h-[18px] w-[18px]"
              viewBox="0 0 24 24"
              fill="none"
              stroke="currentColor"
              stroke-width="1.6"
              stroke-linecap="round"
              stroke-linejoin="round"
              aria-hidden="true"
            >
              <path d="M20.5 14.4A8.7 8.7 0 0 1 9.6 3.5a8.7 8.7 0 1 0 10.9 10.9Z" />
            </svg>
          </TButton>
        </TTooltip>
        <!-- The width belongs on this wrapper: tdesign's own `.t-select__wrap`
             declares `width: 100%` and its sheet is loaded after the utility layer,
             so a width class on the select itself never takes effect. -->
        <label class="block w-32 shrink-0">
          <span class="sr-only">{{ t('language.label') }}</span>
          <TSelect
            :value="localeStore.locale"
            :options="localeStore.options"
            borderless
            @change="onLocaleChange"
          />
        </label>
      </header>

      <main class="min-w-0 flex-1">
        <RouterView />
      </main>
    </div>
  </div>
</template>
