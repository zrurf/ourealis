<script setup lang="ts">
/*
 * Runs: the history at `/runs` and one job's live state, result and detail tabs
 * at `/runs/:id`.
 *
 * The live panel is built around what the service actually reports: a state, the
 * pipeline stage the run is in, an elapsed time and a log. `core` reports its stages
 * through a run observer, so a run of several minutes says what it is doing rather
 * than only that it started. The fraction that comes with a stage counts the named
 * steps of the pipeline, not a share of the work — a route search can outlast every
 * other stage combined — so the stage name is what this panel leads with. The elapsed
 * timer is advanced by an interval and never allowed to overtake a value an event
 * carried.
 *
 * The detail tabs are held in the URL query, so a reload and a deep link open the
 * same one; only the selected tab is mounted, which disposes the 3D scene and the
 * charts of the others.
 */
import { computed, onBeforeUnmount, onMounted, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import { useRoute, useRouter } from 'vue-router'
import {
  Alert as TAlert,
  Button as TButton,
  Card as TCard,
  Divider as TDivider,
  Table as TTable,
  Tag as TTag,
} from 'tdesign-vue-next'
import { isTerminalState } from '@/types/simulation'
import { useNotificationsStore } from '@/stores/notifications'
import { manifestOf, metricRows, reportOf, useSimulationsStore } from '@/stores/simulations'
import TrajectoryPanel from '@/components/run/TrajectoryPanel.vue'
import SensorPanel from '@/components/run/SensorPanel.vue'
import AuditPanel from '@/components/run/AuditPanel.vue'

/** Detail tabs, in order, with the catalog key of each label. */
const TABS: ReadonlyArray<{
  id: 'overview' | 'trajectory' | 'sensors' | 'audit'
  labelKey: string
}> = [
  { id: 'overview', labelKey: 'simulation.tabs.overview' },
  { id: 'trajectory', labelKey: 'simulation.tabs.trajectory' },
  { id: 'sensors', labelKey: 'simulation.tabs.sensors' },
  { id: 'audit', labelKey: 'simulation.tabs.audit' },
]

/** One detail tab of the run page. */
type Tab = (typeof TABS)[number]['id']

const { t, locale } = useI18n({ useScope: 'global' })
const route = useRoute()
const router = useRouter()
const notifications = useNotificationsStore()
const simulations = useSimulationsStore()

let timer: ReturnType<typeof setInterval> | null = null

/** Identifier from the path; empty on `/runs`, which is the history list. */
const jobId = computed(() => String(route.params.id ?? ''))

/** True on the history list, where no single job is shown. */
const isList = computed(() => jobId.value === '')

/** The job being shown, when there is one. */
const job = computed(() => (isList.value ? null : simulations.current))

/** True once the shown job cannot change state again. */
const finished = computed(() => job.value !== null && isTerminalState(job.value.state))

/** Selected detail tab, read from the query; an unknown value falls back to overview. */
const tab = computed<Tab>(() => {
  const value = route.query.tab
  if (typeof value === 'string' && TABS.some((entry) => entry.id === value)) {
    return value as Tab
  }
  return 'overview'
})

/** Summary of the shown job, once read. */
const summary = computed(() => simulations.currentSummary)

/** Metrics report of the summary, when the run evaluated one. */
const report = computed(() => reportOf(summary.value))

/** Manifest rows of the summary. */
const manifestRows = computed(() => {
  const manifest = manifestOf(summary.value)
  if (manifest === null) {
    return []
  }
  return [
    {
      key: 'generator',
      labelKey: 'simulation.result.generator',
      value: manifest.generator ?? '—',
    },
    { key: 'seed', labelKey: 'simulation.result.seed', value: String(manifest.seed ?? '—') },
    {
      key: 'individual',
      labelKey: 'simulation.result.individual',
      value: String(manifest.individual ?? '—'),
    },
    { key: 'mode', labelKey: 'simulation.list.columnMode', value: manifest.mode ?? '—' },
    { key: 'map', labelKey: 'simulation.result.mapName', value: manifest.map_name ?? '—' },
    { key: 'backend', labelKey: 'simulation.result.backend', value: manifest.backend ?? '—' },
    { key: 'mount', labelKey: 'simulation.result.mount', value: manifest.mount ?? '—' },
    {
      key: 'rates',
      labelKey: 'simulation.result.rates',
      value: (manifest.rates_hz ?? []).map((rate) => formatNumber(rate, 2)).join(' / ') || '—',
    },
  ]
})

/** Rows of the headline-metric table. */
const metricTable = computed(() =>
  (report.value === null ? [] : metricRows(report.value)).map((row) => ({
    key: row.key,
    label: t(row.labelKey),
    value: formatNumber(row.value, row.digits),
    unit: row.unit,
  })),
)

/** Sample counts of the result, in the order the DTO lists them. */
const sampleRows = computed(() => {
  const samples = summary.value?.samples
  if (samples === undefined) {
    return []
  }
  return [
    { key: 'truth', labelKey: 'simulation.result.truth', value: samples.truth },
    { key: 'gnss', labelKey: 'simulation.result.gnss', value: samples.gnss },
    { key: 'accel', labelKey: 'simulation.result.accel', value: samples.accel },
    { key: 'gyro', labelKey: 'simulation.result.gyro', value: samples.gyro },
    { key: 'mag', labelKey: 'simulation.result.mag', value: samples.mag },
    { key: 'baro', labelKey: 'simulation.result.baro', value: samples.baro },
  ]
})

/** Recent jobs of the aside and of the history table. */
const recent = computed(() => simulations.jobs.slice(0, 15))

const columns = computed(() => [
  { colKey: 'name', title: t('simulation.list.columnName'), width: 200 },
  { colKey: 'state', title: t('simulation.list.columnState'), width: 110 },
  { colKey: 'created', title: t('simulation.list.columnCreated'), width: 180 },
  { colKey: 'open', title: ' ', width: 90 },
])

const recentRows = computed(() =>
  recent.value.map((entry) => ({
    id: entry.id,
    name: entry.name ?? t('simulation.list.unnamed', { id: entry.id }),
    stateTheme: stateTheme(entry.state),
    stateLabel: t(`simulation.job.${entry.state}`),
    created: formatTimestamp(entry.created_at),
  })),
)

/** Elapsed time as `m:ss`, which is how the service reports it (seconds). */
const elapsedText = computed(() => {
  const total = Math.max(0, simulations.elapsed_s)
  const minutes = Math.floor(total / 60)
  const seconds = Math.floor(total - minutes * 60)
  return `${String(minutes).padStart(2, '0')}:${String(seconds).padStart(2, '0')}`
})

/** Tag appearance of a job state. */
function stateTheme(state: string): 'success' | 'warning' | 'danger' | 'default' {
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

/** Formats a number in the interface locale; a missing value prints as a dash. */
function formatNumber(value: number | null | undefined, digits = 2): string {
  if (value === null || value === undefined || !Number.isFinite(value)) {
    return '—'
  }
  return new Intl.NumberFormat(locale.value, { maximumFractionDigits: digits }).format(value)
}

/** Formats a timestamp in the interface locale. */
function formatTimestamp(value: string): string {
  const parsed = new Date(value)
  return Number.isNaN(parsed.getTime()) ? value : parsed.toLocaleString(locale.value)
}

/** Opens one job. */
function open(id: string): void {
  void router.push({ name: 'simulation', params: { id } })
}

/** Switches the detail tab, in the URL so a reload lands on the same one. */
function selectTab(next: Tab): void {
  void router.replace({ query: { ...route.query, tab: next } })
}

/** Reads the job's state, its stream and — once it finished — its summary. */
async function loadJob(): Promise<void> {
  const id = jobId.value
  if (id === '') {
    return
  }
  simulations.select(id)
  const state = await simulations.refreshJob(id)
  if (state === null) {
    return
  }
  if (isTerminalState(state.state)) {
    await simulations.loadSummary(id)
    return
  }
  simulations.openStream(id)
}

/** Reads the job list again. */
function refresh(): void {
  void simulations.loadJobs()
}

/** Cancels the shown job. */
async function cancel(): Promise<void> {
  try {
    await simulations.cancel(jobId.value)
    notifications.push({ kind: 'info', message: t('simulation.job.cancelled') })
  } catch {
    // The failure notice is pushed by the store.
  }
}

watch(jobId, () => {
  void loadJob()
})

onMounted(() => {
  void simulations.loadJobs()
  if (!isList.value) {
    void loadJob()
  }
  timer = setInterval(() => simulations.tick(), 1_000)
})

onBeforeUnmount(() => {
  if (timer !== null) {
    clearInterval(timer)
    timer = null
  }
  simulations.closeStream()
})
</script>

<template>
  <section class="mx-auto max-w-7xl px-8 py-8" data-testid="simulation-view">
    <template v-if="isList">
      <div class="flex items-center justify-between gap-4">
        <h1 class="font-semibold text-ink">{{ t('views.runs.title') }}</h1>
        <div class="flex items-center gap-2">
          <TButton
            variant="outline"
            :loading="simulations.listStatus === 'loading'"
            data-testid="refresh-runs"
            @click="refresh()"
          >
            {{ t('common.refresh') }}
          </TButton>
          <TButton theme="primary" data-testid="new-run" @click="router.push('/run')">
            {{ t('simulation.form.submit') }}
          </TButton>
        </div>
      </div>

      <div class="mt-6">
        <p v-if="recentRows.length === 0" class="text-sm text-muted">
          {{ t('simulation.list.empty') }}
        </p>
        <TTable
          v-else
          :data="recentRows"
          :columns="columns"
          row-key="id"
          size="small"
          data-testid="job-list"
        >
          <template #state="{ row }">
            <TTag size="small" variant="light" :theme="row.stateTheme">
              {{ row.stateLabel }}
            </TTag>
          </template>
          <template #open="{ row }">
            <TButton variant="text" @click="open(row.id)">
              {{ t('simulation.list.open') }}
            </TButton>
          </template>
        </TTable>
      </div>
    </template>

    <template v-else>
      <div class="flex items-center justify-between gap-4">
        <div class="flex items-center gap-3">
          <h1 class="font-semibold text-ink">{{ t('views.simulation.title') }}</h1>
          <span v-if="job !== null" class="font-mono text-xs text-muted" data-testid="job-id">
            {{ job.id }}
          </span>
          <TTag
            v-if="job !== null"
            variant="light"
            :theme="stateTheme(job.state)"
            data-testid="job-state"
          >
            {{ t(`simulation.job.${job.state}`) }}
          </TTag>
          <TTag
            v-if="job !== null && !finished"
            size="small"
            variant="outline"
            data-testid="job-stage"
          >
            {{ t(`simulation.stage.${job.stage}`) }}
          </TTag>
        </div>
        <div class="flex items-center gap-2">
          <TButton variant="outline" data-testid="new-run" @click="router.push('/run')">
            {{ t('simulation.form.submit') }}
          </TButton>
          <TButton
            v-if="job !== null && !finished"
            theme="danger"
            variant="outline"
            data-testid="cancel-run"
            @click="cancel()"
          >
            {{ t('simulation.live.cancelRun') }}
          </TButton>
        </div>
      </div>

      <div
        class="mt-6 flex flex-wrap items-center gap-1"
        role="tablist"
        :aria-label="t('views.simulation.title')"
      >
        <button
          v-for="entry in TABS"
          :key="entry.id"
          type="button"
          role="tab"
          class="rounded-control px-2 py-1 text-sm transition-colors"
          :class="
            tab === entry.id ? 'bg-surface font-medium text-ink' : 'text-muted hover:text-ink'
          "
          :aria-selected="tab === entry.id"
          :data-testid="`simulation-tab-${entry.id}`"
          @click="selectTab(entry.id)"
        >
          {{ t(entry.labelKey) }}
        </button>
      </div>

      <template v-if="tab === 'overview'">
        <div class="mt-6 grid grid-cols-4 gap-6">
          <div class="col-span-3 flex flex-col gap-6">
            <TAlert
              v-if="job === null && simulations.listStatus === 'ready'"
              theme="error"
              :message="t('simulation.result.noSummary')"
              data-testid="job-missing"
            />

            <TCard v-if="job !== null" :title="t('simulation.live.title')" size="small">
              <div class="grid grid-cols-4 gap-4">
                <div>
                  <p class="text-xs text-muted">{{ t('simulation.live.state') }}</p>
                  <p class="text-sm text-ink">{{ t(`simulation.job.${job.state}`) }}</p>
                </div>
                <div>
                  <p class="text-xs text-muted">{{ t('simulation.live.stage') }}</p>
                  <p class="text-sm text-ink">{{ t(`simulation.stage.${job.stage}`) }}</p>
                </div>
                <div>
                  <p class="text-xs text-muted">{{ t('simulation.live.elapsed') }}</p>
                  <p class="font-mono text-sm text-ink" data-testid="elapsed">{{ elapsedText }}</p>
                </div>
                <div>
                  <p class="text-xs text-muted">{{ t('simulation.list.columnMode') }}</p>
                  <p class="text-sm text-ink">{{ t(`simulation.mode.${job.mode}`) }}</p>
                </div>
              </div>
              <p class="mt-2 text-xs text-muted">{{ t('simulation.live.progressUnknown') }}</p>
              <p class="mt-2 text-xs text-muted">
                {{
                  simulations.streamStatus === 'live'
                    ? t('simulation.live.streamLive')
                    : simulations.streamStatus === 'connecting'
                      ? t('simulation.live.streamConnecting')
                      : t('simulation.live.streamClosed')
                }}
              </p>

              <TAlert
                v-if="job.error !== null && job.error !== undefined"
                class="mt-3"
                theme="error"
                :message="t('simulation.job.failed')"
                data-testid="job-error"
              >
                <p class="text-sm text-muted">
                  <TTag size="small" variant="light">{{ t('simulation.live.fromService') }}</TTag>
                  <span class="ml-2">{{ job.error }}</span>
                </p>
              </TAlert>
              <TAlert
                v-else-if="simulations.streamError !== null && !finished"
                class="mt-3"
                theme="warning"
                :message="t('simulation.live.streamFailed')"
              >
                <p class="text-sm text-muted">{{ simulations.streamError }}</p>
              </TAlert>

              <TDivider class="my-3" />
              <p class="text-sm text-muted">{{ t('simulation.live.log') }}</p>
              <p v-if="simulations.logLines.length === 0" class="mt-1 text-sm text-muted">
                {{ t('simulation.live.logEmpty') }}
              </p>
              <ul
                v-else
                class="mt-1 max-h-64 overflow-y-auto rounded-control bg-page px-3 py-2 font-mono text-xs"
                data-testid="log-stream"
              >
                <li
                  v-for="line in simulations.logLines.slice(-300)"
                  :key="line.id"
                  class="flex gap-2"
                >
                  <span class="text-muted">{{ line.elapsed_s.toFixed(2) }}s</span>
                  <span :class="line.level === 'error' ? 'text-danger' : 'text-ink'">
                    {{ line.message }}
                  </span>
                </li>
              </ul>
            </TCard>

            <TCard v-if="finished" :title="t('simulation.result.title')" size="small">
              <TAlert
                v-if="simulations.summaryStatus === 'failed'"
                theme="error"
                :message="t('simulation.result.summaryFailed')"
                data-testid="summary-error"
              >
                <p class="text-sm text-muted">{{ simulations.summaryError }}</p>
              </TAlert>
              <p v-else-if="summary === null" class="text-sm text-muted">
                {{ t('common.loading') }}
              </p>
              <template v-else>
                <div class="grid grid-cols-3 gap-4">
                  <div>
                    <p class="text-xs text-muted">{{ t('simulation.result.routeLength') }}</p>
                    <p class="text-sm text-ink" data-testid="route-length">
                      {{ formatNumber(summary.route_length_m, 1) }} m
                    </p>
                  </div>
                  <div>
                    <p class="text-xs text-muted">{{ t('simulation.result.duration') }}</p>
                    <p class="text-sm text-ink">{{ formatNumber(summary.duration_s, 2) }} s</p>
                  </div>
                  <div>
                    <p class="text-xs text-muted">{{ t('simulation.result.backend') }}</p>
                    <p class="text-sm text-ink">{{ summary.backend }}</p>
                  </div>
                </div>

                <TDivider class="my-3" />
                <p class="text-sm text-muted">{{ t('simulation.result.samples') }}</p>
                <dl class="mt-1 grid grid-cols-3 gap-x-6 gap-y-1 text-sm">
                  <div v-for="row in sampleRows" :key="row.key" class="flex justify-between gap-3">
                    <dt class="text-muted">{{ t(row.labelKey) }}</dt>
                    <dd class="text-ink">{{ formatNumber(row.value, 0) }}</dd>
                  </div>
                </dl>

                <TDivider class="my-3" />
                <p class="text-sm text-muted">{{ t('simulation.result.metrics') }}</p>
                <p v-if="metricTable.length === 0" class="mt-1 text-sm text-muted">
                  {{ t('simulation.result.noMetrics') }}
                </p>
                <TTable
                  v-else
                  class="mt-1"
                  :data="metricTable"
                  :columns="[
                    { colKey: 'label', title: t('simulation.audit.metric') },
                    { colKey: 'value', title: t('simulation.audit.value') },
                    { colKey: 'unit', title: t('simulation.audit.unit'), width: 90 },
                  ]"
                  row-key="key"
                  size="small"
                  data-testid="headline-metrics"
                />

                <TDivider class="my-3" />
                <p class="text-sm text-muted">{{ t('simulation.result.manifest') }}</p>
                <dl class="mt-1 grid grid-cols-2 gap-x-6 gap-y-1 text-sm">
                  <div
                    v-for="row in manifestRows"
                    :key="row.key"
                    class="flex justify-between gap-3"
                  >
                    <dt class="text-muted">{{ t(row.labelKey) }}</dt>
                    <dd class="font-mono text-xs text-ink">{{ row.value }}</dd>
                  </div>
                </dl>

                <div class="mt-4 flex items-center gap-3">
                  <TButton
                    theme="primary"
                    variant="outline"
                    data-testid="open-trajectory"
                    @click="selectTab('trajectory')"
                  >
                    {{ t('simulation.result.openTrajectory') }}
                  </TButton>
                  <TButton
                    variant="outline"
                    data-testid="open-sensors"
                    @click="selectTab('sensors')"
                  >
                    {{ t('simulation.result.openSensors') }}
                  </TButton>
                  <TButton variant="outline" data-testid="open-audit" @click="selectTab('audit')">
                    {{ t('simulation.result.openAudit') }}
                  </TButton>
                </div>
              </template>
            </TCard>
          </div>

          <aside class="flex flex-col gap-4">
            <TCard :title="t('dashboard.recentJobs')" size="small">
              <p v-if="recentRows.length === 0" class="text-sm text-muted">
                {{ t('simulation.list.empty') }}
              </p>
              <TTable
                v-else
                :data="recentRows"
                :columns="columns"
                row-key="id"
                size="small"
                data-testid="job-list"
              >
                <template #state="{ row }">
                  <TTag size="small" variant="light" :theme="row.stateTheme">
                    {{ row.stateLabel }}
                  </TTag>
                </template>
                <template #open="{ row }">
                  <TButton variant="text" @click="open(row.id)">
                    {{ t('simulation.list.open') }}
                  </TButton>
                </template>
              </TTable>
            </TCard>

            <div
              v-for="notice in notifications.notices"
              :key="notice.id"
              class="rounded-card border border-line bg-surface px-4 py-3"
              :data-testid="`notice-${notice.kind}`"
            >
              <p class="text-sm font-medium text-ink">{{ notice.message }}</p>
              <p v-if="notice.fromService !== undefined" class="mt-1 text-sm text-muted">
                <TTag size="small" variant="light">{{ t('error.fromService') }}</TTag>
                <span class="ml-2">{{ notice.fromService }}</span>
              </p>
            </div>
          </aside>
        </div>
      </template>
      <TrajectoryPanel v-else-if="tab === 'trajectory'" class="mt-6" :job-id="jobId" />
      <SensorPanel v-else-if="tab === 'sensors'" class="mt-6" :job-id="jobId" />
      <AuditPanel v-else class="mt-6" :job-id="jobId" />
    </template>
  </section>
</template>
