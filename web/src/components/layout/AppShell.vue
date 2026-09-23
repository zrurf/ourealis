<script setup lang="ts">
import { computed, ref } from 'vue'
import { useI18n } from 'vue-i18n'
import { useRoute } from 'vue-router'
import { Button as TButton, Select as TSelect } from 'tdesign-vue-next'
import AppIcon from '@/components/layout/AppIcon.vue'
import type { IconName } from '@/components/layout/icons'
import { isSupportedLocale } from '@/locales'
import { useLocaleStore } from '@/stores/locale'
import { useThemeStore } from '@/stores/theme'
import { readStored, writeStored } from '@/utils/storage'
import TaskModal from '@/components/common/TaskModal.vue'
import TaskTray from '@/components/layout/TaskTray.vue'
import TTooltip from '@/components/common/AppTooltip.vue'

/** A sidebar entry: a route a user reaches without an identifier in the path. */
interface NavItem {
  /** Target path, also the value the active check compares against. */
  path: string
  /** Catalog key of the entry's label. */
  labelKey: string
  /** Icon drawn beside the label, and the whole of it while the rail is collapsed. */
  icon: IconName
}

const NAV_ITEMS: readonly NavItem[] = [
  { path: '/', labelKey: 'nav.dashboard', icon: 'dashboard' },
  { path: '/maps', labelKey: 'nav.maps', icon: 'maps' },
  { path: '/run', labelKey: 'nav.run', icon: 'run' },
  { path: '/batch', labelKey: 'nav.batch', icon: 'batch' },
  { path: '/omf', labelKey: 'nav.omf', icon: 'inspect' },
  { path: '/settings', labelKey: 'nav.settings', icon: 'settings' },
]

const { t } = useI18n({ useScope: 'global' })
const route = useRoute()
const themeStore = useThemeStore()
const localeStore = useLocaleStore()

/** Sidebar collapse, remembered like the appearance: it is a working preference. */
const COLLAPSE_KEY = 'ourealis.nav.collapsed'

const collapsed = ref(readStored(COLLAPSE_KEY) === '1')

/** Folds the sidebar down to its icons, or back out. */
function toggleNav(): void {
  collapsed.value = !collapsed.value
  writeStored(COLLAPSE_KEY, collapsed.value ? '1' : '0')
}

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
    <aside
      class="flex shrink-0 flex-col border-r border-line bg-surface transition-[width] duration-150 ease-out"
      :class="collapsed ? 'w-14' : 'w-56'"
      data-testid="app-sidebar"
    >
      <div class="flex items-center gap-2 px-3 py-3">
        <RouterLink v-if="!collapsed" to="/" class="flex-1 text-base font-semibold text-ink">
          {{ t('app.title') }}
        </RouterLink>
        <TTooltip placement="right" :content="collapsed ? t('nav.expand') : t('nav.collapse')">
          <button
            type="button"
            class="rounded-control p-1.5 text-muted transition-colors hover:bg-page hover:text-ink"
            :aria-expanded="!collapsed"
            :aria-label="collapsed ? t('nav.expand') : t('nav.collapse')"
            data-testid="nav-toggle"
            @click="toggleNav()"
          >
            <AppIcon :name="collapsed ? 'chevron-right' : 'panel-left'" />
          </button>
        </TTooltip>
      </div>
      <nav class="flex flex-col gap-0.5 px-2 pb-4">
        <!-- Collapsed, the icon *is* the entry, and its tooltip is what names it. -->
        <TTooltip
          v-for="item in NAV_ITEMS"
          :key="item.path"
          placement="right"
          :content="collapsed ? t(item.labelKey) : ''"
        >
          <RouterLink
            :to="item.path"
            class="flex items-center gap-2.5 rounded-control py-2 text-sm transition-colors duration-150 ease-out"
            :class="[
              collapsed ? 'justify-center px-0' : 'px-3',
              isActive(item.path)
                ? 'bg-page font-medium text-brand'
                : 'text-muted hover:bg-page hover:text-ink',
            ]"
            :title="collapsed ? t(item.labelKey) : undefined"
          >
            <AppIcon :name="item.icon" />
            <span v-if="!collapsed">{{ t(item.labelKey) }}</span>
          </RouterLink>
        </TTooltip>
      </nav>
    </aside>

    <div class="flex min-w-0 flex-1 flex-col">
      <header class="flex h-14 shrink-0 items-center justify-end gap-2 border-b border-line px-6">
        <!-- Work that runs out of the way still has to be visible: a build that failed
             while the user was on another page must be discoverable without
             remembering that it was started. -->
        <TaskTray />
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

    <!-- One loader for the whole application: a blocking task is a property of the
         session, not of the page that started it. -->
    <TaskModal />
  </div>
</template>
