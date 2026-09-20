<script setup lang="ts">
/*
 * Population sweep: N individuals over one route.
 *
 * Each individual is a job of its own — the service has no batch endpoint — so
 * this page submits N requests, watches each one's own event stream, and collects
 * the summaries as they land. Nothing here polls: the state of a run arrives on
 * its SSE stream, and only a finished run is read once through `/summary`.
 *
 * The chosen-path frequency comes from one planning preview per individual. A
 * preview runs the same environment, the same configuration and the same seed as
 * the run and stops before the motion stage (doc §4.3 of the API), so the Logit
 * draw it reports is the draw the run makes; the summary the job endpoint returns
 * does not carry the chosen index at all.
 */
import { computed, onBeforeUnmount, onMounted, ref } from 'vue'
import { useI18n } from 'vue-i18n'
import { useRouter } from 'vue-router'
import {
  Alert as TAlert,
  Button as TButton,
  Card as TCard,
  InputNumber as TInputNumber,
  Table as TTable,
  Tag as TTag,
} from 'tdesign-vue-next'
import { api } from '@/api/client'
import { isApiError } from '@/api/errors'
import { cancelSimulation, getSummary, submitSimulation } from '@/api/simulations'
import { previewRoutes } from '@/api/routes'
import { subscribeJobEvents, type SseSubscription } from '@/api/sse'
import type { SimulationRequest, SubmitReply, Summary } from '@/api/simulations'
import { isTerminalState } from '@/types/simulation'
import type { JobEvent } from '@/types/events'
import type { JobState } from '@/types/simulation'
import EChart from '@/components/charts/EChart.vue'
import { scatterOption, linearFit } from '@/components/charts/options/scatter'
import { baseOption, barSeries } from '@/components/charts/options/types'
import SimulationForm from '@/components/forms/SimulationForm.vue'
import { useMapsStore } from '@/stores/maps'
import { useNotificationsStore } from '@/stores/notifications'
import { formatMetric, messageOf } from '@/stores/simulations'

/** One individual of the sweep. */
interface BatchRun {
  /** Index of the individual, which is also `individual` in its request. */
  individual: number
  /** Seed of this individual's run. */
  seed: number
  /** Identifier of the job, once the service accepted it. */
  id: string | null
  /** Lifecycle state, as last reported. */
  state: string
  /** Failure message, when the run failed. */
  error: string | null
  /** Candidate index the preview chose for this individual, when it ran. */
  chosen: number | null
  /** Candidate count of the preview. */
  candidateCount: number
  /** Summary of the finished run. */
  summary: Summary | null
}

/** How many previews are in flight at once. */
const PREVIEW_CONCURRENCY = 3

/** Attempts one individual's submission gets before its row is failed. */
const SUBMIT_ATTEMPTS = 2

const { t, locale } = useI18n({ useScope: 'global' })
const router = useRouter()
const maps = useMapsStore()
const notifications = useNotificationsStore()

const population = ref(8)
const runs = ref<BatchRun[]>([])
const started = ref(false)
const starting = ref(false)
const failure = ref<string | null>(null)
const streamProblems = ref(0)
let subscriptions: SseSubscription[] = []

/** Runs that finished, whatever the outcome. */
const finished = computed(() => runs.value.filter((run) => isTerminalState(run.state as JobState)))

/** Runs that succeeded and carry a metrics report. */
const succeeded = computed(() => runs.value.filter((run) => run.state === 'succeeded'))

/** Progress line of the sweep. */
const progressText = computed(() =>
  t('simulation.batch.running', { done: finished.value.length, total: runs.value.length }),
)

/** Path frequencies: the share of individuals whose preview chose each candidate. */
const frequencies = computed(() => {
  const counts = new Map<number, number>()
  let total = 0
  for (const run of runs.value) {
    if (run.chosen === null) {
      continue
    }
    counts.set(run.chosen, (counts.get(run.chosen) ?? 0) + 1)
    total += 1
  }
  if (total === 0) {
    return []
  }
  const entries = [...counts.entries()]
  // The array is this function's own copy, so sorting it reorders nothing else.
  // oxlint-disable-next-line unicorn/no-array-sort
  entries.sort((a, b) => a[0] - b[0])
  return entries.map(([index, count]) => ({
    index,
    share: count / total,
    count,
  }))
})

/** Bar chart of the chosen-path frequencies. */
const frequencyOption = computed(() => {
  const entries = frequencies.value
  if (entries.length === 0) {
    return {}
  }
  const base = baseOption(
    { x: t('simulation.batch.candidate', { index: '' }), y: t('simulation.batch.applied') },
    { categoryAxis: true, tooltip: { trigger: 'item' } },
  )
  return {
    ...base,
    xAxis: {
      ...(base.xAxis as Record<string, unknown>),
      data: entries.map((entry) => `#${entry.index}`),
    },
    series: [
      barSeries(
        t('simulation.batch.share'),
        entries.map((entry) => entry.share),
      ),
    ],
  }
})

/** Cadence against mean speed, one point per finished run. */
const cadencePoints = computed(() =>
  succeeded.value
    .map((run) => {
      const metrics = run.summary?.metrics
      if (metrics === null || metrics === undefined) {
        return null
      }
      return { x: metrics.mean_speed_mps ?? 0, y: metrics.step_frequency_hz ?? 0 }
    })
    .filter((point): point is { x: number; y: number } => point !== null),
)

/** Scatter of the cadence-speed relation with its least-squares line. */
const cadenceOption = computed(() =>
  scatterOption({
    x: t('simulation.batch.speedAxis'),
    y: t('simulation.batch.cadenceAxis'),
    title: t('simulation.batch.cadence'),
    points: cadencePoints.value,
    name: t('simulation.batch.cadenceAxis'),
    fit: true,
  }),
)

/** Text of the fitted line, or an empty string before there are two points. */
const fitText = computed(() => {
  if (cadencePoints.value.length < 2) {
    return ''
  }
  const fit = linearFit(cadencePoints.value)
  return t('simulation.batch.fitSummary', {
    slope: formatMetric(fit.slope, '', locale.value, 4),
    r2: formatMetric(fit.r2, '', locale.value, 4),
  })
})

/** Rows of the per-run table. */
const tableRows = computed(() =>
  runs.value.map((run) => ({
    key: String(run.individual),
    individual: run.individual,
    seed: run.seed,
    state: t(`simulation.job.${run.state}`),
    stateTheme: stateTheme(run.state),
    chosen: run.chosen === null ? '—' : `#${run.chosen}`,
    pathRatio: formatMetric(run.summary?.metrics?.path_ratio ?? null, '−', locale.value, 4),
    meanSpeed: formatMetric(run.summary?.metrics?.mean_speed_mps ?? null, 'm/s', locale.value, 3),
    stepFrequency: formatMetric(
      run.summary?.metrics?.step_frequency_hz ?? null,
      'Hz',
      locale.value,
      3,
    ),
    duration: formatMetric(run.summary?.duration_s ?? null, 's', locale.value, 2),
    id: run.id,
  })),
)

const columns = computed(() => [
  { colKey: 'individual', title: t('simulation.batch.columnIndividual'), width: 90 },
  { colKey: 'state', title: t('simulation.batch.columnState'), width: 120 },
  { colKey: 'seed', title: t('simulation.batch.columnSeed'), width: 120 },
  { colKey: 'chosen', title: t('simulation.batch.columnChosen'), width: 110 },
  { colKey: 'pathRatio', title: t('simulation.batch.columnPathRatio'), width: 110 },
  { colKey: 'meanSpeed', title: t('simulation.batch.columnMeanSpeed'), width: 140 },
  { colKey: 'stepFrequency', title: t('simulation.batch.columnStepFrequency'), width: 140 },
  { colKey: 'duration', title: t('simulation.batch.columnDuration'), width: 110 },
  { colKey: 'open', title: ' ', width: 90 },
])

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

/** Builds the request of one individual from the form's request. */
function requestFor(base: SimulationRequest, individual: number, seed: number): SimulationRequest {
  const request: SimulationRequest = {
    ...base,
    individual,
    seed,
    name:
      base.name === null || base.name === undefined
        ? `batch #${individual}`
        : `${base.name} #${individual}`,
  }
  return request
}

/** Starts the sweep: one preview and one job per individual. */
async function start(base: SimulationRequest): Promise<void> {
  if (starting.value) {
    // A sweep is still submitting: replacing `runs` would abandon rows whose jobs
    // already exist and whose streams are already open.
    return
  }
  starting.value = true
  failure.value = null
  closeStreams()
  const count = Math.max(1, Math.trunc(population.value))
  const seed = base.seed ?? 1
  runs.value = Array.from({ length: count }, (_, individual) => ({
    individual,
    seed: seed + individual,
    id: null,
    state: 'queued',
    error: null,
    chosen: null,
    candidateCount: 0,
    summary: null,
  }))
  started.value = true
  try {
    await deriveFrequencies(base, seed, count)
    await submitAll(base, seed, count)
  } catch (error) {
    failure.value = messageOf(error)
    notifications.pushError(t('simulation.batch.submitFailed'), error)
  } finally {
    starting.value = false
  }
}

/**
 * Derives the chosen candidate of every individual.
 *
 * The previews run with a small concurrency: they are CPU-bound planning calls on
 * the same machine that is about to run the simulations, and firing all of them at
 * once would starve the jobs themselves.
 */
async function deriveFrequencies(
  base: SimulationRequest,
  seed: number,
  count: number,
): Promise<void> {
  let next = 0
  const workers = Array.from({ length: Math.min(PREVIEW_CONCURRENCY, count) }, async () => {
    for (;;) {
      const individual = next
      next += 1
      if (individual >= count) {
        return
      }
      try {
        // The previews are issued one per worker: they are CPU-bound planning
        // calls, and the concurrency of the worker pool is what bounds them.
        // oxlint-disable-next-line no-await-in-loop
        const preview = await previewRoutes(requestFor(base, individual, seed + individual))
        const index = runs.value.findIndex((run) => run.individual === individual)
        if (index >= 0) {
          const copy = [...runs.value]
          const run = copy[index]
          if (run !== undefined) {
            copy[index] = {
              ...run,
              chosen: preview.chosen,
              candidateCount: preview.candidates.length,
            }
            runs.value = copy
          }
        }
      } catch (error) {
        notifications.pushError(t('simulation.batch.previewFailed'), error)
        return
      }
    }
  })
  await Promise.all(workers)
}

/** Submits one job per individual and watches each one's stream. */
async function submitAll(base: SimulationRequest, seed: number, count: number): Promise<void> {
  for (let individual = 0; individual < count; individual += 1) {
    const request = requestFor(base, individual, seed + individual)
    // Submissions go one at a time so the queue order matches the individual order;
    // the service's own queue decides when each one actually runs.
    // oxlint-disable-next-line no-await-in-loop
    const reply = await submitOne(request, individual)
    if (reply === null) {
      // One individual the service would not accept does not end the sweep: the
      // remaining ones are still worth submitting.
      continue
    }
    updateRun(individual, { id: reply.id, state: reply.state })
    subscriptions.push(
      subscribeJobEvents(api.url(`simulations/${encodeURIComponent(reply.id)}/events`), {
        onEvent: (event) => applyEvent(individual, event),
        onError: () => {
          streamProblems.value += 1
        },
      }),
    )
  }
}

/**
 * Submits one individual, retrying once while the service is busy.
 *
 * A busy service refuses with `503` for a reason that passes, so the individual is
 * retried; any other refusal fails that row and is left behind. `null` means the
 * row was failed here, with the service's own message on it.
 */
async function submitOne(
  request: SimulationRequest,
  individual: number,
): Promise<SubmitReply | null> {
  for (let attempt = 1; attempt <= SUBMIT_ATTEMPTS; attempt += 1) {
    try {
      // oxlint-disable-next-line no-await-in-loop
      return await submitSimulation(request)
    } catch (error) {
      if (attempt < SUBMIT_ATTEMPTS && isBusy(error)) {
        continue
      }
      updateRun(individual, { state: 'failed', error: messageOf(error) })
      notifications.pushError(t('simulation.batch.submitFailed'), error)
      return null
    }
  }
  return null
}

/** True when the service refused the request because it is busy, which passes. */
function isBusy(error: unknown): boolean {
  return isApiError(error) && (error.kind === 'busy' || error.status === 503)
}

/** Applies one event of one individual's stream. */
function applyEvent(individual: number, event: JobEvent): void {
  if (event.type === 'state') {
    updateRun(individual, { state: event.state })
    return
  }
  if (event.type === 'done') {
    updateRun(individual, { state: event.state === '' ? 'succeeded' : event.state })
    void collect(individual)
    return
  }
  if (event.type === 'error') {
    updateRun(individual, { state: 'failed', error: event.message })
  }
}

/** Reads one finished run's summary and stores it on its row. */
async function collect(individual: number): Promise<void> {
  const run = runs.value.find((entry) => entry.individual === individual)
  if (run === undefined || run.id === null) {
    return
  }
  try {
    const summary = await getSummary(run.id)
    updateRun(individual, { summary })
  } catch (error) {
    updateRun(individual, { error: messageOf(error) })
  }
}

/** Replaces one row of the sweep. */
function updateRun(individual: number, patch: Partial<BatchRun>): void {
  const index = runs.value.findIndex((entry) => entry.individual === individual)
  if (index < 0) {
    return
  }
  const copy = [...runs.value]
  const run = copy[index]
  if (run === undefined) {
    return
  }
  copy[index] = { ...run, ...patch }
  runs.value = copy
}

/** Cancels every job that has not finished. */
async function cancelAll(): Promise<void> {
  const active = runs.value.filter(
    (run) => run.id !== null && !isTerminalState(run.state as JobState),
  )
  for (const run of active) {
    if (run.id === null) {
      continue
    }
    try {
      // oxlint-disable-next-line no-await-in-loop
      await cancelSimulation(run.id)
      updateRun(run.individual, { state: 'cancelled' })
    } catch (error) {
      notifications.pushError(t('simulation.live.cancelFailed'), error)
    }
  }
  closeStreams()
  notifications.push({ kind: 'info', message: t('simulation.batch.cancelled') })
}

/** Closes every subscription the sweep opened. */
function closeStreams(): void {
  for (const subscription of subscriptions) {
    subscription.close()
  }
  subscriptions = []
}

/** Keeps a failure of one run visible in the page's own banner. */
function errorText(error: string | null): string {
  return error ?? ''
}

onMounted(() => {
  void maps.loadMaps()
})

onBeforeUnmount(() => {
  closeStreams()
})
</script>

<template>
  <section class="mx-auto max-w-7xl px-8 py-8" data-testid="batch-view">
    <div class="flex items-center justify-between gap-4">
      <div class="flex items-center gap-3">
        <h1 class="font-semibold text-ink">{{ t('views.batch.title') }}</h1>
        <span class="text-sm text-muted">{{ t('simulation.batch.hint') }}</span>
      </div>
      <div class="flex items-center gap-3">
        <label class="flex items-center gap-2">
          <span class="text-sm text-muted">{{ t('simulation.batch.individuals') }}</span>
          <TInputNumber
            v-model="population"
            :min="1"
            :max="64"
            class="w-24"
            data-testid="batch-population"
          />
        </label>
        <TButton
          v-if="runs.length > 0 && finished.length < runs.length"
          theme="danger"
          variant="outline"
          data-testid="batch-cancel"
          @click="cancelAll()"
        >
          {{ t('simulation.batch.cancelAll') }}
        </TButton>
      </div>
    </div>

    <TAlert
      v-if="failure !== null"
      class="mt-4"
      theme="error"
      :message="t('simulation.batch.submitFailed')"
      data-testid="batch-error"
    >
      <p class="text-sm text-muted">{{ errorText(failure) }}</p>
    </TAlert>

    <TCard class="mt-6" :title="t('simulation.form.title')" size="small">
      <SimulationForm
        :maps="maps.summaries"
        :busy="starting"
        :submit-label="t('simulation.batch.start')"
        @submit="start"
      />
    </TCard>

    <template v-if="started">
      <div class="mt-6 flex items-center gap-3">
        <TTag variant="light" theme="warning" data-testid="batch-progress">{{ progressText }}</TTag>
        <span v-if="streamProblems > 0" class="text-sm text-muted">
          {{ t('simulation.live.streamFailed') }} ({{ streamProblems }})
        </span>
      </div>

      <div class="mt-4">
        <TTable
          :data="tableRows"
          :columns="columns"
          row-key="key"
          size="small"
          data-testid="batch-table"
        >
          <template #state="{ row }">
            <TTag size="small" variant="light" :theme="row.stateTheme">{{ row.state }}</TTag>
          </template>
          <template #open="{ row }">
            <TButton
              v-if="row.id !== null"
              variant="text"
              @click="router.push(`/simulations/${row.id}`)"
            >
              {{ t('simulation.list.open') }}
            </TButton>
          </template>
        </TTable>
      </div>

      <div class="mt-6 grid grid-cols-2 gap-6">
        <TCard :title="t('simulation.batch.frequency')" size="small">
          <p class="text-sm text-muted">{{ t('simulation.batch.frequencyHint') }}</p>
          <p v-if="frequencies.length === 0" class="mt-2 text-sm text-muted">
            {{ t('simulation.batch.empty') }}
          </p>
          <EChart
            v-else
            class="mt-2"
            :option="frequencyOption"
            :height="240"
            data-testid="batch-frequency"
          />
        </TCard>

        <TCard :title="t('simulation.batch.cadence')" size="small">
          <p v-if="cadencePoints.length < 2" class="text-sm text-muted">
            {{ t('simulation.batch.empty') }}
          </p>
          <template v-else>
            <EChart :option="cadenceOption" :height="240" data-testid="batch-cadence" />
            <p class="mt-1 text-sm text-muted" data-testid="batch-fit">
              {{ t('simulation.batch.fit') }}: {{ fitText }}
            </p>
          </template>
        </TCard>
      </div>
    </template>
  </section>
</template>
