<script setup lang="ts">
/*
 * Overview: which service is answering, what it can do, and the way into every
 * other page.
 *
 * The page is deliberately read-only — everything that changes state lives behind
 * the cards — and it keeps its failure states visible: a service that does not
 * answer says so here rather than in a toast nobody kept.
 */
import { computed, onMounted, ref } from 'vue'
import { useI18n } from 'vue-i18n'
import { useRouter } from 'vue-router'
import {
  Alert as TAlert,
  Button as TButton,
  Card as TCard,
  Table as TTable,
  Tag as TTag,
} from 'tdesign-vue-next'
import type { SimulationState } from '@/types/simulation'
import { useSimulationsStore } from '@/stores/simulations'
import { useSystemStore } from '@/stores/system'
import { useNotificationsStore } from '@/stores/notifications'

/** A shortcut into another page. */
interface Shortcut {
  /** Path the card opens. */
  path: string
  /** Catalog key of the title. */
  titleKey: string
  /** Catalog key of the one-line description. */
  bodyKey: string
}

const SHORTCUTS: readonly Shortcut[] = [
  { path: '/maps', titleKey: 'nav.maps', bodyKey: 'dashboard.mapsBody' },
  { path: '/run', titleKey: 'nav.run', bodyKey: 'dashboard.routesBody' },
  { path: '/batch', titleKey: 'nav.batch', bodyKey: 'dashboard.batchBody' },
  { path: '/omf', titleKey: 'nav.omf', bodyKey: 'dashboard.omfBody' },
  { path: '/settings', titleKey: 'nav.settings', bodyKey: 'dashboard.settingsBody' },
]

const { t, locale } = useI18n({ useScope: 'global' })
const router = useRouter()
const system = useSystemStore()
const simulations = useSimulationsStore()
const notifications = useNotificationsStore()

const refreshing = ref(false)

/** Facade switches the service reported. */
const facades = computed(() => [
  { key: 'rpc', enabled: system.info?.rpc_enabled ?? false, labelKey: 'dashboard.facadeRpc' },
  { key: 'http', enabled: system.info?.http_enabled ?? false, labelKey: 'dashboard.facadeHttp' },
  { key: 'web', enabled: system.info?.web_enabled ?? false, labelKey: 'dashboard.facadeWeb' },
])

/** Facts the overview lists as key/value rows. */
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

/** Jobs shown in the recent list. */
const recent = computed(() => simulations.jobs.slice(0, 12))

const columns = computed(() => [
  { colKey: 'name', title: t('simulation.list.columnName'), width: 240 },
  { colKey: 'state', title: t('simulation.list.columnState'), width: 120 },
  { colKey: 'mode', title: t('simulation.list.columnMode'), width: 110 },
  { colKey: 'created', title: t('simulation.list.columnCreated'), width: 190 },
  { colKey: 'open', title: ' ', width: 120 },
])

/** Rows of the recent-jobs table, already formatted. */
const rows = computed(() =>
  recent.value.map((job) => ({
    id: job.id,
    name: job.name ?? t('simulation.list.unnamed', { id: job.id }),
    stateTheme: stateTheme(job.state),
    stateLabel: t(`simulation.job.${job.state}`),
    mode: t(`simulation.mode.${job.mode}`),
    created: formatTimestamp(job.created_at),
  })),
)

/** Tag appearance of a job state. */
function stateTheme(state: SimulationState['state']): 'success' | 'warning' | 'danger' | 'default' {
  switch (state) {
    case 'succeeded':
      return 'success'
    case 'failed':
      return 'danger'
    case 'queued':
    case 'running':
      return 'warning'
    default:
      return 'default'
  }
}

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

/** Reads the service facts and the job table. */
async function refresh(): Promise<void> {
  refreshing.value = true
  try {
    await Promise.all([system.load(), simulations.loadJobs()])
  } finally {
    refreshing.value = false
  }
}

/** Opens one job. */
function open(row: { id: string }): void {
  void router.push({ name: 'simulation', params: { id: row.id } })
}

onMounted(() => {
  void refresh()
})
</script>

<template>
  <section class="mx-auto max-w-7xl px-8 py-8" data-testid="dashboard">
    <div class="flex items-center justify-between gap-4">
      <h1 class="font-semibold text-ink">{{ t('views.dashboard.title') }}</h1>
      <div class="flex items-center gap-3">
        <TTag :theme="system.isAlive ? 'success' : 'danger'" variant="light">
          <span data-testid="dashboard-health">
            {{ system.isAlive ? t('dashboard.connected') : t('dashboard.disconnected') }}
          </span>
        </TTag>
        <TButton variant="outline" :loading="refreshing" @click="refresh()">
          {{ t('common.refresh') }}
        </TButton>
      </div>
    </div>

    <TAlert
      v-if="system.status === 'failed'"
      class="mt-4"
      theme="error"
      :message="t('dashboard.systemFailed')"
      data-testid="dashboard-error"
    >
      <p class="text-sm text-muted">
        <TTag size="small" variant="light">{{ t('error.fromService') }}</TTag>
        <span class="ml-2">{{ system.error }}</span>
      </p>
    </TAlert>

    <p v-else-if="system.status === 'loading'" class="mt-4 text-sm text-muted">
      {{ t('common.loading') }}
    </p>

    <div class="mt-6 grid grid-cols-3 gap-6">
      <TCard :title="t('dashboard.system')" size="small" class="col-span-2">
        <dl class="grid grid-cols-2 gap-x-6 gap-y-1 text-sm">
          <div v-for="fact in facts" :key="fact.key" class="flex justify-between gap-3">
            <dt class="text-muted">{{ t(fact.labelKey) }}</dt>
            <dd class="text-right font-mono text-xs text-ink">{{ fact.value }}</dd>
          </div>
        </dl>
      </TCard>

      <TCard :title="t('dashboard.facades')" size="small">
        <ul class="flex flex-col gap-2">
          <li v-for="facade in facades" :key="facade.key" class="flex items-center justify-between">
            <span class="text-sm text-ink">{{ t(facade.labelKey) }}</span>
            <TTag size="small" variant="light" :theme="facade.enabled ? 'success' : 'default'">
              {{ facade.enabled ? t('dashboard.enabled') : t('dashboard.disabled') }}
            </TTag>
          </li>
        </ul>
      </TCard>
    </div>

    <div class="mt-6 grid grid-cols-5 gap-4">
      <button
        v-for="shortcut in SHORTCUTS"
        :key="shortcut.path"
        type="button"
        class="rounded-card border border-line bg-surface px-4 py-3 text-left transition-colors duration-150 ease-out hover:bg-page"
        @click="router.push(shortcut.path)"
      >
        <span class="text-sm font-medium text-ink">{{ t(shortcut.titleKey) }}</span>
        <span class="mt-1 block text-xs text-muted">{{ t(shortcut.bodyKey) }}</span>
      </button>
    </div>

    <div class="mt-8">
      <div class="flex items-center justify-between">
        <h2 class="text-base font-medium text-ink">{{ t('dashboard.recentJobs') }}</h2>
        <span class="text-sm text-muted">
          {{ t('simulation.list.active', { count: simulations.activeJobs.length }) }}
        </span>
      </div>

      <p v-if="simulations.listStatus === 'loading'" class="mt-2 text-sm text-muted">
        {{ t('common.loading') }}
      </p>
      <TAlert
        v-else-if="simulations.listStatus === 'failed'"
        class="mt-2"
        theme="error"
        :message="t('simulation.list.loadFailed')"
      >
        <p class="text-sm text-muted">{{ simulations.listError }}</p>
      </TAlert>
      <TAlert
        v-else-if="rows.length === 0"
        class="mt-2"
        theme="info"
        :message="t('simulation.list.empty')"
      />
      <TTable
        v-else
        class="mt-2"
        :data="rows"
        :columns="columns"
        row-key="id"
        hover
        data-testid="dashboard-jobs"
      >
        <template #state="{ row }">
          <TTag size="small" variant="light" :theme="row.stateTheme">{{ row.stateLabel }}</TTag>
        </template>
        <template #open="{ row }">
          <TButton variant="text" @click="open(row)">{{ t('simulation.list.open') }}</TButton>
        </template>
      </TTable>
    </div>

    <div
      v-for="notice in notifications.notices"
      :key="notice.id"
      class="mt-3 rounded-card border border-line bg-surface px-4 py-3"
      :data-testid="`notice-${notice.kind}`"
    >
      <p class="text-sm font-medium text-ink">{{ notice.message }}</p>
      <p v-if="notice.fromService !== undefined" class="mt-1 text-sm text-muted">
        <TTag size="small" variant="light">{{ t('error.fromService') }}</TTag>
        <span class="ml-2">{{ notice.fromService }}</span>
      </p>
    </div>
  </section>
</template>
