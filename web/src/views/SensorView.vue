<script setup lang="ts">
/*
 * Sensors: the recorded channels, the GNSS error cloud, the map-plane track, a
 * spectrum and the exports.
 *
 * The page reads the pages the store cached and never the network twice: the
 * charts draw a downsampled view of a channel while the export asks the service
 * for the file, so a 200 Hz inertial stream neither blocks the render nor has to
 * fit in a chart. The spectrum is computed in the browser from the accelerometer
 * channel — the service's own spectral summary lives in the metrics report and is
 * rendered by the audit page.
 */
import { computed, onMounted, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import { useRoute, useRouter } from 'vue-router'
import {
  Alert as TAlert,
  Button as TButton,
  Card as TCard,
  RadioGroup as TRadioGroup,
  Tag as TTag,
} from 'tdesign-vue-next'
import type { ExportFormat } from '@/api/simulations'
import type { SensorSample, TruthSample } from '@/types/result'
import EChart from '@/components/charts/EChart.vue'
import { lineOption } from '@/components/charts/options/line'
import { scatterOption } from '@/components/charts/options/scatter'
import { spectrumOption } from '@/components/charts/options/spectrum'
import { useNotificationsStore } from '@/stores/notifications'
import {
  CHANNELS,
  downsample,
  downloadBytes,
  reportOf,
  useSimulationsStore,
} from '@/stores/simulations'

/** Channel names as query values, in the order the picker lists them. */
type Channel = (typeof CHANNELS)[number]

/** Samples a spectrum is computed from. */
const SPECTRUM_SAMPLES = 2_048

const { t, locale } = useI18n({ useScope: 'global' })
const route = useRoute()
const router = useRouter()
const simulations = useSimulationsStore()
const notifications = useNotificationsStore()

const channel = ref<Channel>('accel')
const truth = ref<TruthSample[]>([])
const samples = ref<SensorSample[]>([])
const accelSamples = ref<SensorSample[]>([])
const gnssSamples = ref<SensorSample[]>([])
const status = ref<'idle' | 'loading' | 'ready' | 'failed'>('idle')
const failure = ref<string | null>(null)
const exporting = ref<ExportFormat | null>(null)

const jobId = computed(() => String(route.params.id ?? ''))

/** Channel options of the picker. */
const channelOptions = computed(() =>
  CHANNELS.map((name) => ({ value: name, label: t(`simulation.sensors.channels.${name}`) })),
)

/** The metrics report, which carries the service's own spectral summary. */
const report = computed(() => reportOf(simulations.currentSummary))

/** Rows of the GNSS usability summary. */
const gnssRows = computed(() => {
  const fixes = gnssSamples.value.filter((sample) => sample.channel === 'gnss')
  const valid = fixes.filter((sample) => sample.valid === true).length
  return {
    valid,
    dropped: fixes.length - valid,
    availability: fixes.length === 0 ? 0 : valid / fixes.length,
  }
})

/** Series of the selected channel, one per component. */
const channelSeries = computed(() => {
  const reduced = downsample(samples.value)
  const names = componentNames()
  const series = names.map((name, component) => ({
    name,
    data: reduced.map((sample) => ({
      x: sample.time_s,
      y: componentValue(sample, component),
    })),
  }))
  return lineOption({
    x: t('simulation.sensors.axes.time'),
    y: axisLabel(),
    title: t(`simulation.sensors.channels.${channel.value}`),
    series,
  })
})

/** GNSS fixes against the truth, in the map plane. */
const trackOption = computed(() => {
  const fixes = downsample(
    gnssSamples.value.filter((entry) => entry.channel === 'gnss'),
    3_000,
  )
  const fixesPoints = fixes
    .filter((fix) => fix.v !== null && fix.v !== undefined)
    .map((fix) => ({ x: fix.v?.[0] ?? 0, y: fix.v?.[1] ?? 0 }))
  const truthPoints = downsample(truth.value, 3_000).map((sample) => ({
    x: sample.position.x,
    y: sample.position.y,
  }))
  return lineOption({
    x: t('simulation.sensors.trackX'),
    y: t('simulation.sensors.trackY'),
    title: t('simulation.sensors.gnssTrack'),
    legend: true,
    series: [
      { name: t('simulation.sensors.truthTrack'), data: truthPoints },
      { name: t('simulation.sensors.gnssTrackSeries'), data: fixesPoints },
    ],
  })
})

/** GNSS error cloud: each fix's offset from the truth at its time. */
const errorOption = computed(() => {
  const points = errorPoints().map((error) => ({ x: error.east, y: error.north }))
  return scatterOption({
    x: t('simulation.sensors.errorEast'),
    y: t('simulation.sensors.errorNorth'),
    title: t('simulation.sensors.gnssErrorCloud'),
    points,
    name: t('simulation.sensors.gnssErrorCloud'),
    symbolSize: 5,
  })
})

/** Quantiles of the horizontal error, in metres. */
const errorSummary = computed(() => {
  const distances = errorPoints().map((error) => Math.hypot(error.east, error.north))
  if (distances.length === 0) {
    return []
  }
  distances.sort((a, b) => a - b)
  const at = (quantile: number): number =>
    distances[Math.min(distances.length - 1, Math.floor(quantile * distances.length))] ?? 0
  return [
    { key: 'p50', label: t('simulation.audit.p50'), value: at(0.5) },
    { key: 'p95', label: t('simulation.audit.p95'), value: at(0.95) },
    { key: 'max', label: t('simulation.audit.max'), value: distances[distances.length - 1] ?? 0 },
  ]
})

/** Spectrum of the vertical accelerometer channel, from the samples themselves. */
const spectrum = computed(() => {
  const accel = accelSamples.value.map((sample) => sample.v?.[2] ?? 0)
  if (accel.length < 32) {
    return null
  }
  const rate = sampleRate()
  const window = accel.slice(0, Math.min(SPECTRUM_SAMPLES, accel.length))
  const { frequencies, magnitudes } = periodogram(window, rate)
  return spectrumOption({
    x: t('simulation.sensors.spectrumAxis'),
    y: t('simulation.sensors.magnitudeAxis'),
    title: t('simulation.sensors.spectrum'),
    frequencies,
    magnitudes,
    name: t('simulation.sensors.channels.accel'),
  })
})

/** Names of the components a channel carries. */
function componentNames(): string[] {
  switch (channel.value) {
    case 'gnss':
      return [t('simulation.trajectory.channelSpeed')]
    case 'baro':
      return [t('simulation.sensors.axes.pressure')]
    case 'accel':
      return ['x', 'y', 'z']
    case 'gyro':
      return ['x', 'y', 'z']
    case 'mag':
      return ['x', 'y', 'z']
  }
}

/** Reads one component of one sample. */
function componentValue(sample: SensorSample, component: number): number {
  if (sample.channel === 'baro') {
    return sample.pressure_pa ?? 0
  }
  if (sample.channel === 'gnss') {
    return sample.speed_mps ?? 0
  }
  return sample.v?.[component] ?? 0
}

/** Y-axis label of the selected channel. */
function axisLabel(): string {
  switch (channel.value) {
    case 'accel':
      return t('simulation.sensors.axes.acceleration')
    case 'gyro':
      return t('simulation.sensors.axes.angularRate')
    case 'mag':
      return t('simulation.sensors.axes.magneticField')
    case 'baro':
      return t('simulation.sensors.axes.pressure')
    case 'gnss':
      return t('simulation.trajectory.axisSpeed')
  }
}

/** GNSS error of every fix, matched to the truth sample nearest its time. */
function errorPoints(): Array<{ east: number; north: number }> {
  const fixes = gnssSamples.value.filter((entry) => entry.channel === 'gnss')
  const timeline = truth.value
  if (fixes.length === 0 || timeline.length === 0) {
    return []
  }
  const out: Array<{ east: number; north: number }> = []
  // Both series ascend in time, so one cursor walks the timeline across all fixes
  // instead of rescanning it per fix. The fix list is short but the timeline runs at
  // the inertial rate, and the per-fix scan over it dominated this page's render.
  let cursor = 0
  const distanceAt = (index: number, time_s: number): number =>
    Math.abs((timeline[index]?.time_s ?? 0) - time_s)
  for (const fix of downsample(fixes, 5_000)) {
    const v = fix.v
    if (v === null || v === undefined) {
      continue
    }
    while (
      cursor + 1 < timeline.length &&
      distanceAt(cursor + 1, fix.time_s) <= distanceAt(cursor, fix.time_s)
    ) {
      cursor += 1
    }
    const reference = timeline[cursor]
    if (reference === undefined || Math.abs(reference.time_s - fix.time_s) > 0.5) {
      continue
    }
    out.push({ east: v[0] - reference.position.x, north: v[1] - reference.position.y })
  }
  return out
}

/** Sample rate of the accelerometer channel, hertz, from its own time stamps. */
function sampleRate(): number {
  const list = accelSamples.value
  if (list.length < 2) {
    return 100
  }
  const first = list[0]?.time_s ?? 0
  const last = list[list.length - 1]?.time_s ?? 0
  const span = last - first
  return span > 0 ? (list.length - 1) / span : 100
}

/**
 * One-sided periodogram of a signal.
 *
 * A direct transform over a window is enough here: the window is a couple of
 * thousand samples, the result only feeds a chart, and a dependency-free
 * implementation keeps the sensor page independent of the Rust side's own FFT.
 */
function periodogram(
  values: readonly number[],
  rateHz: number,
): { frequencies: number[]; magnitudes: number[] } {
  const count = values.length
  const mean = values.reduce((sum, value) => sum + value, 0) / count
  const bins = Math.floor(count / 2)
  const frequencies: number[] = []
  const magnitudes: number[] = []
  for (let bin = 1; bin <= bins; bin += 1) {
    let real = 0
    let imaginary = 0
    for (let index = 0; index < count; index += 1) {
      const value = (values[index] ?? 0) - mean
      const angle = (2 * Math.PI * bin * index) / count
      real += value * Math.cos(angle)
      imaginary -= value * Math.sin(angle)
    }
    frequencies.push((bin * rateHz) / count)
    magnitudes.push((2 * Math.sqrt(real * real + imaginary * imaginary)) / count)
  }
  return { frequencies, magnitudes }
}

/** Loads the truth timeline and the selected channel. */
async function load(): Promise<void> {
  status.value = 'loading'
  failure.value = null
  simulations.select(jobId.value)
  await simulations.refreshJob(jobId.value)
  await simulations.loadSummary(jobId.value)
  truth.value = await simulations.loadAllTruth()
  // The accelerometer channel feeds the spectrum panel and the GNSS channel feeds the
  // error card and the track chart. Both are read whatever channel the picker shows,
  // because those panels are always on the page.
  accelSamples.value = await simulations.loadAllSensor('accel')
  gnssSamples.value = await simulations.loadAllSensor('gnss')
  samples.value = await selectChannel(channel.value)
  status.value = 'ready'
  if (simulations.resultStatus === 'failed') {
    failure.value = simulations.resultError
  }
}

/** Samples of one channel, reusing the two already loaded. */
async function selectChannel(name: Channel): Promise<SensorSample[]> {
  if (name === 'accel') {
    return accelSamples.value
  }
  if (name === 'gnss') {
    return gnssSamples.value
  }
  return simulations.loadAllSensor(name)
}

/** Downloads one export in the requested format. */
async function exportAs(exportFormat: ExportFormat): Promise<void> {
  exporting.value = exportFormat
  try {
    const { bytes, fileName } = await simulations.exportJob(jobId.value, exportFormat)
    downloadBytes(bytes, fileName ?? `${jobId.value}.${exportFormat}`, contentType(exportFormat))
    notifications.push({ kind: 'success', message: t('simulation.sensors.exportDone') })
  } catch (error) {
    notifications.pushError(t('simulation.sensors.exportFailed'), error)
  } finally {
    exporting.value = null
  }
}

/** Content type of an export format. */
function contentType(exportFormat: ExportFormat): string {
  switch (exportFormat) {
    case 'json':
      return 'application/json'
    case 'csv':
      return 'text/csv; charset=utf-8'
    case 'geojson':
      return 'application/geo+json'
  }
}

/** Formats a number in the interface locale. */
function format(value: number, digits = 2): string {
  return new Intl.NumberFormat(locale.value, { maximumFractionDigits: digits }).format(value)
}

watch(channel, async () => {
  samples.value = await selectChannel(channel.value)
})

onMounted(() => {
  void load()
})
</script>

<template>
  <section class="mx-auto max-w-7xl px-8 py-8" data-testid="sensor-view">
    <div class="flex items-center justify-between gap-4">
      <div class="flex items-center gap-3">
        <h1 class="font-semibold text-ink">{{ t('views.sensor.title') }}</h1>
        <span class="font-mono text-xs text-muted">{{ jobId }}</span>
      </div>
      <div class="flex items-center gap-3">
        <span class="text-sm text-muted">{{ t('simulation.sensors.exportHint') }}</span>
        <TButton
          v-for="format in ['json', 'csv', 'geojson'] as ExportFormat[]"
          :key="format"
          variant="outline"
          :loading="exporting === format"
          :data-testid="`export-${format}`"
          @click="exportAs(format)"
        >
          {{ t(`simulation.export.${format}`) }}
        </TButton>
        <TButton variant="outline" @click="router.push(`/simulations/${jobId}`)">
          {{ t('common.back') }}
        </TButton>
      </div>
    </div>

    <TAlert
      v-if="failure !== null"
      class="mt-4"
      theme="error"
      :message="t('simulation.sensors.loadFailed')"
      data-testid="sensor-error"
    >
      <p class="text-sm text-muted">
        <TTag size="small" variant="light">{{ t('simulation.live.fromService') }}</TTag>
        <span class="ml-2">{{ failure }}</span>
      </p>
    </TAlert>
    <p v-else-if="status === 'loading'" class="mt-4 text-sm text-muted">
      {{ t('common.loading') }}
    </p>

    <div class="mt-4 flex items-center gap-4">
      <span class="text-sm text-muted">{{ t('simulation.sensors.channel') }}</span>
      <TRadioGroup
        variant="default-filled"
        :value="channel"
        :options="channelOptions"
        data-testid="channel-select"
        @change="(value) => (channel = String(value) as Channel)"
      />
      <span class="text-sm text-muted" data-testid="sample-count">
        {{ format(simulations.totalOf(channel), 0) }} {{ t('simulation.sensors.sampleCount') }}
      </span>
    </div>

    <TCard class="mt-4" size="small">
      <TAlert
        v-if="samples.length === 0"
        theme="info"
        :message="t('simulation.sensors.noSamples')"
        data-testid="sensor-empty"
      />
      <EChart v-else :option="channelSeries" :height="280" data-testid="channel-chart" />
    </TCard>

    <div class="mt-6 grid grid-cols-2 gap-6">
      <TCard :title="t('simulation.sensors.gnssErrorCloud')" size="small">
        <EChart :option="errorOption" :height="280" data-testid="gnss-error-chart" />
        <dl class="mt-2 flex flex-col gap-1 text-sm">
          <div v-for="row in errorSummary" :key="row.key" class="flex justify-between gap-3">
            <dt class="text-muted">{{ row.label }}</dt>
            <dd class="font-mono text-ink">{{ format(row.value, 3) }} m</dd>
          </div>
          <div class="flex justify-between gap-3">
            <dt class="text-muted">{{ t('simulation.sensors.availability') }}</dt>
            <dd class="font-mono text-ink" data-testid="gnss-availability">
              {{ format(gnssRows.availability * 100, 1) }} %
            </dd>
          </div>
          <div class="flex justify-between gap-3">
            <dt class="text-muted">{{ t('simulation.sensors.validFixes') }}</dt>
            <dd class="font-mono text-ink">{{ format(gnssRows.valid, 0) }}</dd>
          </div>
          <div class="flex justify-between gap-3">
            <dt class="text-muted">{{ t('simulation.sensors.droppedFixes') }}</dt>
            <dd class="font-mono text-ink">{{ format(gnssRows.dropped, 0) }}</dd>
          </div>
        </dl>
      </TCard>

      <TCard :title="t('simulation.sensors.gnssTrack')" size="small">
        <EChart :option="trackOption" :height="280" data-testid="gnss-track-chart" />
      </TCard>

      <TCard class="col-span-2" :title="t('simulation.sensors.spectrum')" size="small">
        <p v-if="spectrum === null" class="text-sm text-muted">
          {{ t('simulation.sensors.noSamples') }}
        </p>
        <EChart v-else :option="spectrum" :height="260" data-testid="spectrum-chart" />
        <p v-if="report?.accel_spectrum" class="mt-2 text-sm text-muted">
          {{ t('simulation.audit.dominantPeak') }}:
          <span class="font-mono text-ink">
            {{
              report.accel_spectrum.dominant === null ||
              report.accel_spectrum.dominant === undefined
                ? '—'
                : `${format(report.accel_spectrum.dominant.frequency_hz, 3)} Hz @ ${format(report.accel_spectrum.dominant.magnitude, 4)}`
            }}
          </span>
        </p>
      </TCard>
    </div>
  </section>
</template>
