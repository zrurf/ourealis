<script setup lang="ts">
/*
 * Settings: the interface's own preferences and the service's effective config.
 *
 * The two halves are deliberately different kinds of thing. Language, appearance
 * and units live in this browser and are applied immediately; the configuration
 * below is read-only and comes from `/system/config`, which is the service's own
 * merged defaults — the only truthful answer to "what is this instance running
 * with", as opposed to what a file on disk says.
 */
import { computed, onMounted, ref } from 'vue'
import { useI18n } from 'vue-i18n'
import {
  Alert as TAlert,
  Button as TButton,
  Card as TCard,
  RadioGroup as TRadioGroup,
  Select as TSelect,
  Tag as TTag,
} from 'tdesign-vue-next'
import { fetchSystemConfig } from '@/api/system'
import { isApiError } from '@/api/errors'
import { isSupportedLocale, type LocaleName } from '@/locales'
import { useLocaleStore } from '@/stores/locale'
import { useThemeStore } from '@/stores/theme'
import { useSystemStore } from '@/stores/system'
import { useSimulationsStore } from '@/stores/simulations'

const { t, locale } = useI18n({ useScope: 'global' })
const localeStore = useLocaleStore()
const themeStore = useThemeStore()
const system = useSystemStore()
const simulations = useSimulationsStore()

const config = ref<Record<string, unknown> | null>(null)
const configStatus = ref<'idle' | 'loading' | 'ready' | 'failed'>('idle')
const configError = ref<string | null>(null)
const copied = ref(false)

/** Facade switches the service reported. */
const facades = computed(() => [
  { key: 'rpc', enabled: system.info?.rpc_enabled ?? false, labelKey: 'dashboard.facadeRpc' },
  { key: 'http', enabled: system.info?.http_enabled ?? false, labelKey: 'dashboard.facadeHttp' },
  { key: 'web', enabled: system.info?.web_enabled ?? false, labelKey: 'dashboard.facadeWeb' },
])

/** Facts the service card lists as key/value rows. */
const facts = computed(() => {
  const info = system.info
  if (info === null) {
    return []
  }
  return [
    { key: 'version', labelKey: 'dashboard.fieldVersion', value: info.version },
    { key: 'workspace', labelKey: 'dashboard.fieldWorkspace', value: info.workspace_version },
    { key: 'core', labelKey: 'dashboard.fieldCore', value: info.core_version },
    { key: 'mapFormat', labelKey: 'dashboard.fieldMapFormat', value: info.map_format_version },
    {
      key: 'api',
      labelKey: 'dashboard.fieldApi',
      value: `${info.api_version} (${info.api_prefix})`,
    },
    { key: 'build', labelKey: 'dashboard.fieldBuild', value: formatTimestamp(info.build_time) },
    { key: 'backend', labelKey: 'dashboard.fieldBackend', value: info.backend },
    { key: 'threads', labelKey: 'dashboard.fieldThreads', value: String(info.worker_threads) },
    { key: 'storage', labelKey: 'dashboard.fieldStorage', value: info.storage_mode },
    { key: 'assets', labelKey: 'dashboard.fieldAssets', value: assetsText() },
    { key: 'presets', labelKey: 'dashboard.fieldPresets', value: info.presets.join(', ') || '—' },
    { key: 'maps', labelKey: 'dashboard.fieldMaps', value: String(info.maps) },
    {
      key: 'inflight',
      labelKey: 'dashboard.fieldInFlight',
      value: String(info.simulations_in_flight),
    },
  ]
})

/** Formats a timestamp in the interface locale. */
function formatTimestamp(value: string): string {
  const parsed = new Date(value)
  return Number.isNaN(parsed.getTime()) ? value : parsed.toLocaleString(locale.value)
}

/** Embedded-asset line, which tells a developer whether the bundle is real. */
function assetsText(): string {
  const info = system.info
  if (info === null) {
    return '—'
  }
  const size = new Intl.NumberFormat(locale.value, { maximumFractionDigits: 0 }).format(
    info.web_asset_bytes / 1024,
  )
  return info.web_assets_built
    ? `${info.web_asset_files} files, ${size} KiB`
    : t('dashboard.assetsPlaceholder')
}

/** Language options the catalogs cover. */
const localeOptions = computed(() =>
  localeStore.options.map((option) => ({ value: option.value, label: option.label })),
)

/** Appearance options; the switch that flips them is in the shell's header. */
const themeOptions = computed(() => [
  { value: 'light', label: t('theme.light') },
  { value: 'dark', label: t('theme.dark') },
])

/** Unit options; the wire format stays metric either way. */
const unitOptions = computed(() => [
  { value: 'metric', label: t('settings.unitsMetric') },
  { value: 'imperial', label: t('settings.unitsImperial') },
])

/** The configuration as flat key/value rows, one level deep. */
const configRows = computed(() => {
  const source = config.value
  if (source === null) {
    return []
  }
  const rows: Array<{ key: string; value: string }> = []
  for (const [key, value] of Object.entries(source)) {
    if (typeof value === 'object' && value !== null) {
      for (const [nested, inner] of Object.entries(value as Record<string, unknown>)) {
        rows.push({ key: `${key}.${nested}`, value: formatValue(inner) })
      }
      continue
    }
    rows.push({ key, value: formatValue(value) })
  }
  return rows
})

/** Renders one configuration value as text. */
function formatValue(value: unknown): string {
  if (value === null) {
    return 'null'
  }
  if (Array.isArray(value)) {
    return value.length === 0 ? '[]' : JSON.stringify(value)
  }
  if (typeof value === 'object') {
    return JSON.stringify(value)
  }
  return String(value)
}

/** Reads the service's effective configuration. */
async function loadConfig(): Promise<void> {
  configStatus.value = 'loading'
  configError.value = null
  copied.value = false
  try {
    config.value = await fetchSystemConfig()
    configStatus.value = 'ready'
  } catch (error) {
    configError.value = isApiError(error) ? error.message : String(error)
    configStatus.value = 'failed'
  }
}

/** Copies the configuration to the clipboard, reporting a refusal. */
async function copyConfig(): Promise<void> {
  const text = JSON.stringify(config.value ?? {}, null, 2)
  try {
    await navigator.clipboard.writeText(text)
    copied.value = true
  } catch {
    configError.value = t('settings.copyFailed')
  }
}

onMounted(() => {
  void system.load()
  void loadConfig()
})
</script>

<template>
  <section class="mx-auto max-w-5xl px-8 py-8" data-testid="settings">
    <h1 class="font-semibold text-ink">{{ t('views.settings.title') }}</h1>

    <TCard class="mt-6" :title="t('settings.interface')" size="small">
      <div class="grid grid-cols-3 gap-6">
        <label>
          <span class="text-sm text-muted">{{ t('settings.language') }}</span>
          <TSelect
            :value="localeStore.locale"
            :options="localeOptions"
            data-testid="settings-locale"
            @change="
              (value) => isSupportedLocale(value) && localeStore.setLocale(value as LocaleName)
            "
          />
        </label>
        <div>
          <span class="text-sm text-muted">{{ t('settings.theme') }}</span>
          <TRadioGroup
            class="mt-1"
            variant="default-filled"
            :value="themeStore.isDark ? 'dark' : 'light'"
            :options="themeOptions"
            data-testid="settings-theme"
            @change="(value) => themeStore.setTheme(value === 'dark' ? 'dark' : 'light')"
          />
        </div>
        <div>
          <span class="text-sm text-muted">{{ t('settings.units') }}</span>
          <TSelect
            class="mt-1"
            :value="simulations.units"
            :options="unitOptions"
            data-testid="settings-units"
            @change="(value) => simulations.setUnits(value === 'imperial' ? 'imperial' : 'metric')"
          />
        </div>
      </div>
      <p class="mt-3 text-sm text-muted">{{ t('settings.unitsHint') }}</p>
    </TCard>

    <TCard class="mt-6" :title="t('settings.service')" size="small">
      <p class="text-sm text-muted">{{ t('settings.serviceHint') }}</p>

      <div class="mt-3 flex items-center gap-3">
        <TTag :theme="system.isAlive ? 'success' : 'danger'" variant="light">
          {{ system.isAlive ? t('dashboard.connected') : t('dashboard.disconnected') }}
        </TTag>
        <span v-if="system.info !== null" class="text-sm text-muted">
          {{ system.info.version }} · {{ system.info.backend }} · {{ system.info.storage_mode }}
        </span>
        <TButton
          variant="outline"
          :loading="configStatus === 'loading'"
          data-testid="settings-reload"
          @click="loadConfig()"
        >
          {{ t('settings.reload') }}
        </TButton>
        <TButton
          v-if="configRows.length > 0"
          variant="outline"
          data-testid="settings-copy"
          @click="copyConfig()"
        >
          {{ t('settings.copyConfig') }}
        </TButton>
        <span v-if="copied" class="text-sm text-muted" data-testid="settings-copied">
          {{ t('settings.copied') }}
        </span>
      </div>

      <TAlert
        v-if="system.status === 'failed'"
        class="mt-3"
        theme="error"
        :message="t('dashboard.systemFailed')"
        data-testid="settings-health-error"
      >
        <p class="text-sm text-muted">
          <TTag size="small" variant="light">{{ t('error.fromService') }}</TTag>
          <span class="ml-2">{{ system.error }}</span>
        </p>
      </TAlert>

      <ul
        v-if="system.info !== null"
        class="mt-3 flex flex-col gap-2"
        data-testid="settings-facades"
      >
        <li v-for="facade in facades" :key="facade.key" class="flex items-center justify-between">
          <span class="text-sm text-ink">{{ t(facade.labelKey) }}</span>
          <TTag size="small" variant="light" :theme="facade.enabled ? 'success' : 'default'">
            {{ facade.enabled ? t('dashboard.enabled') : t('dashboard.disabled') }}
          </TTag>
        </li>
      </ul>

      <dl
        v-if="facts.length > 0"
        class="mt-3 grid grid-cols-2 gap-x-6 gap-y-1 text-sm"
        data-testid="settings-facts"
      >
        <div v-for="fact in facts" :key="fact.key" class="flex justify-between gap-3">
          <dt class="text-muted">{{ t(fact.labelKey) }}</dt>
          <dd class="text-right font-mono text-xs text-ink">{{ fact.value }}</dd>
        </div>
      </dl>

      <TAlert
        v-if="configStatus === 'failed'"
        class="mt-3"
        theme="error"
        :message="t('settings.configFailed')"
        data-testid="settings-error"
      >
        <p class="text-sm text-muted">
          <TTag size="small" variant="light">{{ t('error.fromService') }}</TTag>
          <span class="ml-2">{{ configError }}</span>
        </p>
      </TAlert>
      <p v-else-if="configStatus === 'loading'" class="mt-3 text-sm text-muted">
        {{ t('common.loading') }}
      </p>
      <TAlert
        v-else-if="!system.isAlive && configRows.length === 0"
        class="mt-3"
        theme="info"
        :message="t('settings.noService')"
      />
      <TAlert
        v-else-if="configRows.length === 0"
        class="mt-3"
        theme="info"
        :message="t('settings.configEmpty')"
      />
      <dl
        v-else
        class="mt-3 grid grid-cols-2 gap-x-8 gap-y-1 text-sm"
        data-testid="settings-config"
      >
        <div v-for="row in configRows" :key="row.key" class="flex justify-between gap-4">
          <dt class="font-mono text-xs text-muted">{{ row.key }}</dt>
          <dd class="text-right font-mono text-xs text-ink">{{ row.value }}</dd>
        </div>
      </dl>
    </TCard>
  </section>
</template>
