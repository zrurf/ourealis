<script setup lang="ts">
/*
 * Audit: the run's own metrics report, and the same metrics against another run.
 *
 * Every number on this page comes from the service's `MetricsReport`
 * (`crates/core/src/eval/mod.rs`) or from the two-sample comparison endpoint; the
 * page computes nothing that the report already carries, and labels each value
 * with the unit the report uses. The two histograms are the exception and are
 * built from the truth timeline, which is why they are titled as distributions of
 * the samples rather than as report fields.
 */
import { computed, onMounted, ref } from 'vue'
import { useI18n } from 'vue-i18n'
import { useRoute, useRouter } from 'vue-router'
import {
  Alert as TAlert,
  Button as TButton,
  Card as TCard,
  Select as TSelect,
  Table as TTable,
  Tag as TTag,
} from 'tdesign-vue-next'
import type { TruthSample } from '@/types/result'
import { isTerminalState } from '@/types/simulation'
import EChart from '@/components/charts/EChart.vue'
import { acfOption, whiteNoiseBand } from '@/components/charts/options/acf'
import { histogramOption } from '@/components/charts/options/histogram'
import { compareOption } from '@/components/charts/options/series-compare'
import type { Point } from '@/components/charts/options/types'
import {
  acfPoints,
  compareRows,
  comparedValue,
  downsample,
  distributionRows,
  formatMetric,
  metricRows,
  reportOf,
  useSimulationsStore,
  type SpectrumSummary,
} from '@/stores/simulations'

/** Speed below which a sample belongs to a start or stop ramp, m/s. */
const RUNNING_SPEED_THRESHOLD = 0.5

const { t, locale } = useI18n({ useScope: 'global' })
const route = useRoute()
const router = useRouter()
const simulations = useSimulationsStore()

const truth = ref<TruthSample[]>([])
const status = ref<'idle' | 'loading' | 'ready' | 'failed'>('idle')
const failure = ref<string | null>(null)
const compareWith = ref<string | null>(null)

const jobId = computed(() => String(route.params.id ?? ''))

/** The report of the shown run, or `null` when it carries none. */
const report = computed(() => reportOf(simulations.currentSummary))

/** Rows of the primary-metric table. */
const primaryRows = computed(() =>
  (report.value === null ? [] : metricRows(report.value)).map((row) => ({
    key: row.key,
    label: t(row.labelKey),
    value: formatMetric(row.value, row.unit, locale.value, row.digits),
  })),
)

/** Speed quantiles of the report. */
const speedRows = computed(() =>
  (report.value === null ? [] : distributionRows(report.value.speed, 'm/s', 3)).map(toRow),
)

/** Turn-rate quantiles of the report. */
const turnRows = computed(() =>
  (report.value === null ? [] : distributionRows(report.value.turn_rate, 'rad/s', 4)).map(toRow),
)

/** GNSS error quantiles, when GNSS was simulated. */
const gnssRows = computed(() => {
  const gnss = report.value?.gnss
  if (gnss === null || gnss === undefined) {
    return []
  }
  const rows: Array<{ group: string; rows: ReturnType<typeof toRow>[] }> = [
    {
      group: t('simulation.audit.gnssHorizontal'),
      rows: distributionRows(gnss.horizontal, 'm', 3).map(toRow),
    },
    {
      group: t('simulation.audit.gnssVertical'),
      rows: distributionRows(gnss.vertical, 'm', 3).map(toRow),
    },
    {
      group: t('simulation.audit.gnssSpeed'),
      rows: distributionRows(gnss.speed, 'm/s', 3).map(toRow),
    },
  ]
  return rows
})

/** Lap times of a loop session, with the coefficient of variation. */
const lapRows = computed(() =>
  (report.value?.lap_times ?? []).map((time, index) => ({
    key: String(index),
    label: t('simulation.audit.lap', { index: index + 1 }),
    value: `${formatMetric(time, 's', locale.value, 3)}`,
  })),
)

/** Speeds of the moving samples, in m/s. */
const movingSpeeds = computed(() =>
  downsample(truth.value, 40_000)
    .filter((sample) => !sample.standing && sample.speed >= RUNNING_SPEED_THRESHOLD)
    .map((sample) => sample.speed),
)

/** Turn rates of the moving samples, `speed * kappa_eff`, in rad/s. */
const turnRates = computed(() =>
  downsample(truth.value, 40_000)
    .filter((sample) => !sample.standing && sample.speed >= RUNNING_SPEED_THRESHOLD)
    .map((sample) => sample.speed * sample.kappa_eff),
)

/** Histogram of the running speeds. */
const speedHistogram = computed(() =>
  histogramOption({
    x: t('simulation.audit.meanSpeed'),
    y: t('simulation.audit.count'),
    title: t('simulation.audit.speedDistribution'),
    values: movingSpeeds.value,
    bins: 32,
    decimals: 2,
  }),
)

/** Histogram of the turn rates. */
const turnHistogram = computed(() =>
  histogramOption({
    x: t('simulation.audit.turnRateP95'),
    y: t('simulation.audit.count'),
    title: t('simulation.audit.turnRateDistribution'),
    values: turnRates.value,
    bins: 32,
    decimals: 3,
  }),
)

/** Autocorrelation of the speed residual, with the white-noise band. */
const speedAcfOption = computed(() => acfChart('speed'))
/** Autocorrelation of the lateral position residual. */
const positionAcfOption = computed(() => acfChart('position'))

/** Builds one ACF chart from the report. */
function acfChart(which: 'speed' | 'position') {
  const current = report.value
  if (current === null) {
    return {}
  }
  const values = which === 'speed' ? current.speed_acf : current.position_acf
  const count = Math.max(current.speed.count, current.turn_rate.count)
  return acfOption({
    x: t('simulation.audit.acfLag'),
    y: t('simulation.audit.acfCoefficient'),
    title: which === 'speed' ? t('simulation.audit.acfSpeed') : t('simulation.audit.acfPosition'),
    lags: acfPoints(values, current.acf_lag_s).map((point) => point.x),
    values,
    name: which === 'speed' ? t('simulation.audit.acfSpeed') : t('simulation.audit.acfPosition'),
    confidence: whiteNoiseBand(count),
  })
}

/** Rows of one spectrum summary. */
function spectrumRows(spectrum: SpectrumSummary | null | undefined) {
  if (spectrum === null || spectrum === undefined) {
    return []
  }
  const peak = (value: { frequency_hz: number; magnitude: number } | null | undefined): string =>
    value === null || value === undefined
      ? '—'
      : `${formatMetric(value.frequency_hz, 'Hz', locale.value, 3)} @ ${formatMetric(value.magnitude, '', locale.value, 4)}`
  return [
    {
      key: 'resolution',
      label: t('simulation.audit.resolution'),
      value: formatMetric(spectrum.resolution_hz, 'Hz', locale.value, 5),
    },
    { key: 'dominant', label: t('simulation.audit.dominantPeak'), value: peak(spectrum.dominant) },
    { key: 'step', label: t('simulation.audit.stepPeak'), value: peak(spectrum.step_peak) },
    {
      key: 'harmonics',
      label: t('simulation.audit.harmonicRatios'),
      value:
        spectrum.harmonic_ratios.length === 0
          ? '—'
          : spectrum.harmonic_ratios
              .map(
                (ratio, index) =>
                  `${t('simulation.audit.harmonicRatio', { index: index + 2 })} ${ratio.toFixed(3)}`,
              )
              .join('  '),
    },
  ]
}

const accelSpectrumRows = computed(() => spectrumRows(report.value?.accel_spectrum))
const baroSpectrumRows = computed(() => spectrumRows(report.value?.baro_spectrum))

/** Finished runs the comparison may use. */
const candidates = computed(() =>
  simulations.jobs
    .filter((job) => job.state === 'succeeded' && job.id !== jobId.value)
    .map((job) => ({
      value: job.id,
      label: job.name ?? t('simulation.list.unnamed', { id: job.id }),
    })),
)

/** The comparison reply, when one has been run. */
const comparison = computed(() => simulations.compareResult)

/** Rows of the comparison table: both runs and the difference. */
const compareTable = computed(() => {
  const result = comparison.value
  if (result === null) {
    return []
  }
  return compareRows(result).map((row) => {
    const values = comparedValue(result, row.key)
    return {
      key: row.key,
      label: t(row.labelKey),
      a: formatMetric(values.a, row.unit, locale.value, row.digits),
      b: formatMetric(values.b, row.unit, locale.value, row.digits),
      delta: formatMetric(row.value, row.unit, locale.value, row.digits),
    }
  })
})

/**
 * One run's position on the cadence-against-pace plane.
 *
 * The comparison carries each run's mean speed and step frequency and nothing per
 * sample, so the point is that pair. The KS rows above the chart describe the speed
 * samples; this is the pair of headline numbers those samples produced.
 */
function cadencePacePoint(run: { mean_speed_mps: number; step_frequency_hz: number }): Point[] {
  return [{ x: run.mean_speed_mps, y: run.step_frequency_hz }]
}

const compareChart = computed(() => {
  const result = comparison.value
  if (result === null) {
    return {}
  }
  return compareOption({
    x: t('simulation.audit.meanSpeed'),
    y: t('simulation.audit.stepFrequency'),
    title: t('simulation.audit.compareChart'),
    differenceLabel: t('simulation.audit.delta'),
    series: [
      { name: t('simulation.audit.baseline'), data: cadencePacePoint(result.a) },
      { name: t('simulation.audit.other'), data: cadencePacePoint(result.b) },
    ],
  })
})

/** One table row from a metric row. */
function toRow(row: {
  key: string
  labelKey: string
  value: number | null
  unit: string
  digits: number
}) {
  return {
    key: row.key,
    label: t(row.labelKey),
    value: formatMetric(row.value, row.unit, locale.value, row.digits),
  }
}

/** Loads the run's report and the truth timeline the histograms need. */
async function load(): Promise<void> {
  status.value = 'loading'
  failure.value = null
  simulations.select(jobId.value)
  const state = await simulations.refreshJob(jobId.value)
  if (state !== null && !isTerminalState(state.state)) {
    failure.value = t('simulation.result.noSummary')
    status.value = 'failed'
    return
  }
  await simulations.loadSummary(jobId.value)
  truth.value = await simulations.loadAllTruth()
  status.value = 'ready'
}

/** Runs the comparison against the selected run. */
async function compare(): Promise<void> {
  const other = compareWith.value
  if (other === null || other === jobId.value) {
    return
  }
  await simulations.compare(jobId.value, other)
}

onMounted(() => {
  void simulations.loadJobs()
  void load()
})
</script>

<template>
  <section class="mx-auto max-w-7xl px-8 py-8" data-testid="audit-view">
    <div class="flex items-center justify-between gap-4">
      <div class="flex items-center gap-3">
        <h1 class="font-semibold text-ink">{{ t('views.audit.title') }}</h1>
        <span class="font-mono text-xs text-muted">{{ jobId }}</span>
        <!--
          No plausibility badge: the service reports measured quantities and never
          decides whether a run is plausible, so a badge here could only assert
          something nothing evaluated. The report below is the evidence.
        -->
      </div>
      <TButton variant="outline" @click="router.push(`/simulations/${jobId}`)">
        {{ t('common.back') }}
      </TButton>
    </div>

    <TAlert
      v-if="failure !== null"
      class="mt-4"
      theme="error"
      :message="t('simulation.audit.loadFailed')"
      data-testid="audit-error"
    >
      <p class="text-sm text-muted">{{ failure }}</p>
    </TAlert>
    <p v-else-if="status === 'loading'" class="mt-4 text-sm text-muted">
      {{ t('common.loading') }}
    </p>
    <TAlert
      v-else-if="report === null"
      class="mt-4"
      theme="info"
      :message="t('simulation.audit.noReport')"
      data-testid="audit-empty"
    />

    <template v-if="report !== null">
      <div class="mt-6 grid grid-cols-3 gap-6">
        <TCard :title="t('simulation.audit.primary')" size="small">
          <TTable
            :data="primaryRows"
            :columns="[
              { colKey: 'label', title: t('simulation.audit.metric') },
              { colKey: 'value', title: t('simulation.audit.value') },
            ]"
            row-key="key"
            size="small"
            data-testid="audit-primary"
          />
        </TCard>

        <TCard :title="t('simulation.audit.speedDistribution')" size="small">
          <TTable
            :data="speedRows"
            :columns="[
              { colKey: 'label', title: t('simulation.audit.metric') },
              { colKey: 'value', title: t('simulation.audit.value') },
            ]"
            row-key="key"
            size="small"
            data-testid="audit-speed"
          />
        </TCard>

        <TCard :title="t('simulation.audit.turnRateDistribution')" size="small">
          <TTable
            :data="turnRows"
            :columns="[
              { colKey: 'label', title: t('simulation.audit.metric') },
              { colKey: 'value', title: t('simulation.audit.value') },
            ]"
            row-key="key"
            size="small"
            data-testid="audit-turn-rate"
          />
        </TCard>
      </div>

      <div class="mt-6 grid grid-cols-2 gap-6">
        <TCard :title="t('simulation.audit.speedDistribution')" size="small">
          <EChart :option="speedHistogram" :height="240" data-testid="audit-speed-histogram" />
        </TCard>
        <TCard :title="t('simulation.audit.turnRateDistribution')" size="small">
          <EChart :option="turnHistogram" :height="240" data-testid="audit-turn-histogram" />
        </TCard>
        <TCard :title="t('simulation.audit.acfSpeed')" size="small">
          <EChart :option="speedAcfOption" :height="240" data-testid="audit-acf-speed" />
          <p class="mt-1 text-xs text-muted">
            {{
              t('simulation.audit.acfConfidence', {
                band: whiteNoiseBand(report.speed.count).toFixed(4),
              })
            }}
          </p>
        </TCard>
        <TCard :title="t('simulation.audit.acfPosition')" size="small">
          <EChart :option="positionAcfOption" :height="240" data-testid="audit-acf-position" />
        </TCard>
      </div>

      <div class="mt-6 grid grid-cols-3 gap-6">
        <TCard :title="t('simulation.audit.accelSpectrum')" size="small">
          <TTable
            :data="accelSpectrumRows"
            :columns="[
              { colKey: 'label', title: t('simulation.audit.metric') },
              { colKey: 'value', title: t('simulation.audit.value') },
            ]"
            row-key="key"
            size="small"
            data-testid="audit-accel-spectrum"
          />
        </TCard>
        <TCard :title="t('simulation.audit.baroSpectrum')" size="small">
          <TTable
            :data="baroSpectrumRows"
            :columns="[
              { colKey: 'label', title: t('simulation.audit.metric') },
              { colKey: 'value', title: t('simulation.audit.value') },
            ]"
            row-key="key"
            size="small"
            data-testid="audit-baro-spectrum"
          />
        </TCard>
        <TCard :title="t('simulation.audit.laps')" size="small">
          <p v-if="lapRows.length === 0" class="text-sm text-muted">
            {{ t('simulation.audit.noReport') }}
          </p>
          <TTable
            v-else
            :data="lapRows"
            :columns="[
              { colKey: 'label', title: t('simulation.audit.metric') },
              { colKey: 'value', title: t('simulation.audit.value') },
            ]"
            row-key="key"
            size="small"
            data-testid="audit-laps"
          />
        </TCard>
      </div>

      <TCard class="mt-6" :title="t('simulation.audit.gnss')" size="small">
        <p v-if="gnssRows.length === 0" class="text-sm text-muted">
          {{ t('simulation.audit.noReport') }}
        </p>
        <div v-else class="grid grid-cols-3 gap-6">
          <div v-for="group in gnssRows" :key="group.group">
            <p class="text-sm font-medium text-ink">{{ group.group }}</p>
            <TTable
              :data="group.rows"
              :columns="[
                { colKey: 'label', title: t('simulation.audit.metric') },
                { colKey: 'value', title: t('simulation.audit.value') },
              ]"
              row-key="key"
              size="small"
            />
          </div>
        </div>
      </TCard>

      <TCard class="mt-6" :title="t('simulation.audit.compare')" size="small">
        <p class="text-sm text-muted">{{ t('simulation.audit.compareHint') }}</p>
        <div class="mt-3 flex items-end gap-3">
          <label class="w-80">
            <span class="text-sm text-muted">{{ t('simulation.audit.compareWith') }}</span>
            <TSelect
              :value="compareWith ?? ''"
              :options="candidates"
              :placeholder="t('simulation.audit.noCandidates')"
              data-testid="compare-select"
              @change="(value) => (compareWith = String(value))"
            />
          </label>
          <TButton
            theme="primary"
            :loading="simulations.compareStatus === 'loading'"
            :disabled="compareWith === null"
            data-testid="compare-run"
            @click="compare()"
          >
            {{ t('simulation.audit.compareAction') }}
          </TButton>
        </div>

        <TAlert
          v-if="simulations.compareStatus === 'failed'"
          class="mt-3"
          theme="error"
          :message="t('simulation.audit.compareFailed')"
          data-testid="compare-error"
        >
          <p class="text-sm text-muted">
            <TTag size="small" variant="light">{{ t('simulation.live.fromService') }}</TTag>
            <span class="ml-2">{{ simulations.compareError }}</span>
          </p>
        </TAlert>

        <template v-if="comparison !== null">
          <div class="mt-3 grid grid-cols-2 gap-6">
            <TTable
              :data="compareTable"
              :columns="[
                { colKey: 'label', title: t('simulation.audit.metric') },
                { colKey: 'a', title: t('simulation.audit.baseline') },
                { colKey: 'b', title: t('simulation.audit.other') },
                { colKey: 'delta', title: t('simulation.audit.delta') },
              ]"
              row-key="key"
              size="small"
              data-testid="compare-table"
            />
            <div>
              <p class="text-sm font-medium text-ink">{{ t('simulation.audit.speedKs') }}</p>
              <dl class="mt-1 flex flex-col gap-1 text-sm">
                <div class="flex justify-between gap-3">
                  <dt class="text-muted">{{ t('simulation.audit.ksStatistic') }}</dt>
                  <dd class="font-mono text-ink" data-testid="ks-statistic">
                    {{ formatMetric(comparison.speed_ks.statistic, '−', locale, 4) }}
                  </dd>
                </div>
                <div class="flex justify-between gap-3">
                  <dt class="text-muted">{{ t('simulation.audit.ksPValue') }}</dt>
                  <dd class="font-mono text-ink">
                    {{ formatMetric(comparison.speed_ks.p_value, '−', locale, 4) }}
                  </dd>
                </div>
                <div class="flex justify-between gap-3">
                  <dt class="text-muted">{{ t('simulation.audit.ksSamples') }}</dt>
                  <dd class="font-mono text-ink">
                    {{ comparison.speed_ks.samples_a }} / {{ comparison.speed_ks.samples_b }}
                  </dd>
                </div>
              </dl>
              <EChart class="mt-3" :option="compareChart" :height="200" />
            </div>
          </div>
        </template>
      </TCard>
    </template>
  </section>
</template>
