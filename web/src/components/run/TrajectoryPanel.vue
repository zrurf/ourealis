<script setup lang="ts">
/*
 * Trajectory playback: the truth timeline over the map surface, the runner marker
 * at the playhead, and the channels drawn against time.
 *
 * The panel owns the clock. Its frame loop advances a time in simulated seconds by
 * `dt * rate`, which keeps the runner's motion smooth at any rate instead of
 * stepping sample by sample; the charts and the 3D marker read the same time, so
 * they cannot drift apart.
 */
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import { Alert as TAlert, Card as TCard } from 'tdesign-vue-next'
import { api } from '@/api/client'
import { isApiError } from '@/api/errors'
import { JobSocket } from '@/api/ws'
import type { ChunkRef } from '@/api/maps'
import { LAYER_ELEVATION, chunkKey, levelCellSize, selectLevel } from '@/types/map'
import type { TruthSample } from '@/types/result'
import { engineFromQuery, probeEngine } from '@/render/engine'
import { updateDebug } from '@/render/scene'
import { renderHost, type SceneLease } from '@/render/host'
import type { MapScene } from '@/render/scene'
import { TerrainLayer } from '@/render/terrain'
import { buildChunkMesh, chunkGrid } from '@/render/terrainMesh'
import { createCellSampler } from '@/render/cellSampler'
import { TrajectoryLine, sampleTrajectory, type TrajectoryChannel } from '@/render/trajectory'
import { meshSurfaceSampler, type SurfaceHeight } from '@/render/drape'
import EChart from '@/components/charts/EChart.vue'
import { lineOption } from '@/components/charts/options/line'
import ChannelPicker from '@/components/trajectory/ChannelPicker.vue'
import PlaybackControls from '@/components/trajectory/PlaybackControls.vue'
import SpeedGauge from '@/components/trajectory/SpeedGauge.vue'
import Timeline from '@/components/trajectory/Timeline.vue'
import { useMapsStore } from '@/stores/maps'
import {
  displayLength,
  displaySpeed,
  downsample,
  lengthUnit,
  speedUnit,
  useSimulationsStore,
} from '@/stores/simulations'

/** Resolution the camera is treated as showing when the level is chosen, m/px. */
const TARGET_METRES_PER_PIXEL = 2

/**
 * How far above the drawn surface the trajectory line is held, metres.
 *
 * The samples carry the runner's own altitude, so a line above this reads as a track
 * hovering over the campus; the lift only has to clear the depth fight with the surface.
 */
const TRAJECTORY_LIFT_M = 0.25

const props = defineProps<{ jobId: string }>()

const { t, locale } = useI18n({ useScope: 'global' })
const maps = useMapsStore()
const simulations = useSimulationsStore()

const canvas = ref<HTMLCanvasElement | null>(null)
const samples = ref<TruthSample[]>([])
const index = ref(0)
const playing = ref(false)
const rate = ref(1)
const channel = ref<TrajectoryChannel>('speed')
const status = ref<'idle' | 'loading' | 'ready' | 'failed'>('idle')
const failure = ref<string | null>(null)

let scene: MapScene | null = null
let lease: SceneLease | null = null
let terrain: TerrainLayer | null = null
let surface: SurfaceHeight | null = null
let line: TrajectoryLine | null = null
let frame: number | null = null
let lastFrameMs = 0
let disposed = false

const jobId = computed(() => props.jobId)

/** Times of the loaded samples, seconds. */
const times = computed(() => samples.value.map((sample) => sample.time_s))

/** Sample under the playhead. */
const current = computed<TruthSample | null>(() => samples.value[index.value] ?? null)

/** Speed range of the run, which the gauge and the colouring share. */
const speedRange = computed(() => {
  let min = Number.POSITIVE_INFINITY
  let max = Number.NEGATIVE_INFINITY
  for (const sample of samples.value) {
    min = Math.min(min, sample.speed)
    max = Math.max(max, sample.speed)
  }
  return {
    min: Number.isFinite(min) ? min : 0,
    max: Number.isFinite(max) ? max : 0,
  }
})

/** Trajectory points handed to the 3D line: a strided view of the raw pages. */
const points = computed(() => sampleTrajectory(downsample(samples.value), channel.value))

/** One channel against time, reduced to what a chart can draw. */
function seriesOf(read: (sample: TruthSample) => number): Array<{ x: number; y: number }> {
  return downsample(samples.value).map((sample) => ({ x: sample.time_s, y: read(sample) }))
}

/** Speed against time. */
const speedOption = computed(() =>
  withPlayhead(
    lineOption({
      x: t('simulation.trajectory.axisTime'),
      y: t('simulation.trajectory.axisSpeed'),
      title: t('simulation.trajectory.chartSpeed'),
      series: [{ name: t('simulation.trajectory.channelSpeed'), data: seriesOf((s) => s.speed) }],
    }),
  ),
)

/** Altitude against time: the reported height and the terrain under it. */
const altitudeOption = computed(() =>
  withPlayhead(
    lineOption({
      x: t('simulation.trajectory.axisTime'),
      y: t('simulation.trajectory.axisAltitude'),
      title: t('simulation.trajectory.chartAltitude'),
      series: [
        { name: t('simulation.trajectory.reportedZ'), data: seriesOf((s) => s.z) },
        { name: t('simulation.trajectory.terrainZ'), data: seriesOf((s) => s.terrain_z) },
      ],
    }),
  ),
)

/** Grade against time. */
const gradeOption = computed(() =>
  withPlayhead(
    lineOption({
      x: t('simulation.trajectory.axisTime'),
      y: t('simulation.trajectory.axisGrade'),
      title: t('simulation.trajectory.chartGrade'),
      series: [{ name: t('simulation.trajectory.chartGrade'), data: seriesOf((s) => s.grade) }],
    }),
  ),
)

/** Curvature against time. */
const curvatureOption = computed(() =>
  withPlayhead(
    lineOption({
      x: t('simulation.trajectory.axisTime'),
      y: t('simulation.trajectory.axisCurvature'),
      title: t('simulation.trajectory.chartCurvature'),
      series: [
        {
          name: t('simulation.trajectory.chartCurvature'),
          data: seriesOf((s) => s.kappa_eff),
        },
      ],
    }),
  ),
)

/** Adds the playhead to the first series of an option as a mark line. */
function withPlayhead(option: ReturnType<typeof lineOption>): ReturnType<typeof lineOption> {
  const series = option.series
  if (Array.isArray(series) && series.length > 0) {
    const first = series[0] as Record<string, unknown>
    first['markLine'] = {
      silent: true,
      symbol: 'none',
      label: { show: false },
      data: [{ xAxis: current.value?.time_s ?? 0 }],
    }
  }
  return option
}

/** Length unit the readouts display in. */
const lengthLabel = computed(() => lengthUnit(simulations.units))

/** Speed unit the readouts display in. */
const speedLabel = computed(() => speedUnit(simulations.units))

/** Position readout of the playhead, in the display units. */
const positionText = computed(() => {
  const sample = current.value
  if (sample === null) {
    return '—'
  }
  const x = format(displayLength(sample.position.x, simulations.units))
  const y = format(displayLength(sample.position.y, simulations.units))
  return `${x} / ${y} ${lengthLabel.value}`
})

/** Speed readout of the playhead, in the display units. */
const speedText = computed(() => format(displaySpeed(current.value?.speed ?? 0, simulations.units)))

/** Formats a number in the interface locale. */
function format(value: number, digits = 2): string {
  return new Intl.NumberFormat(locale.value, { maximumFractionDigits: digits }).format(value)
}

/** Starts or stops the frame loop. */
function togglePlayback(): void {
  playing.value = !playing.value
  if (playing.value) {
    lastFrameMs = performance.now()
    frame = requestAnimationFrame(advance)
  } else if (frame !== null) {
    cancelAnimationFrame(frame)
    frame = null
  }
}

/**
 * Advances the playhead by the wall-clock time that passed.
 *
 * The clock is simulated seconds, so a single step can cross several samples and
 * the index is looked up rather than incremented.
 */
function advance(now: number): void {
  if (!playing.value || disposed) {
    return
  }
  const elapsedS = Math.min(0.25, (now - lastFrameMs) / 1000)
  lastFrameMs = now
  const last = times.value[times.value.length - 1] ?? 0
  const nextTime = (current.value?.time_s ?? 0) + elapsedS * rate.value
  if (nextTime >= last) {
    index.value = Math.max(0, times.value.length - 1)
    playing.value = false
    frame = null
    return
  }
  index.value = indexAt(nextTime)
  frame = requestAnimationFrame(advance)
}

/** Index of the sample nearest a time. */
function indexAt(time_s: number): number {
  const list = times.value
  if (list.length === 0) {
    return 0
  }
  let best = 0
  let bestDistance = Number.POSITIVE_INFINITY
  for (let position = 0; position < list.length; position += 1) {
    const distance = Math.abs((list[position] ?? 0) - time_s)
    if (distance < bestDistance) {
      bestDistance = distance
      best = position
    }
  }
  return best
}

/** Moves the playhead by one sample. */
function step(direction: -1 | 1): void {
  playing.value = false
  const next = index.value + direction
  index.value = Math.min(Math.max(0, next), Math.max(0, samples.value.length - 1))
}

/** Returns the playhead to the first sample. */
function rewind(): void {
  playing.value = false
  index.value = 0
}

/**
 * Loads the run's truth pages, then the surface and the line.
 *
 * The panel can be unmounted while one of the awaits is pending, so every one
 * of them is followed by the guard: a resumed load must not build a scene on a
 * canvas that is already detached.
 */
async function load(): Promise<void> {
  status.value = 'loading'
  failure.value = null
  simulations.select(jobId.value)
  await simulations.refreshJob(jobId.value)
  if (disposed) {
    return
  }
  await simulations.loadSummary(jobId.value)
  if (disposed) {
    return
  }
  const truth = await loadTruth()
  if (disposed) {
    return
  }
  samples.value = truth
  if (truth.length === 0) {
    status.value = 'failed'
    failure.value = t('simulation.trajectory.empty')
    return
  }
  await startScene()
  if (disposed) {
    return
  }
  status.value = 'ready'
}

/** How long the job socket is given before the paged HTTP reads take over. */
const SOCKET_TIMEOUT_MS = 8_000

/**
 * Reads the truth timeline over the job socket.
 *
 * One socket connection carries the whole timeline in chunks, which is the
 * transport `api/ws.ts` exists for. It is bounded by a timeout because a socket
 * that cannot be opened leaves its request pending rather than rejecting — a
 * development server whose proxy does not forward the upgrade is exactly that
 * case — and the paged HTTP reads the store caches are the fallback. The export
 * always goes through the service's own writer, never through this.
 */
async function loadTruth(): Promise<TruthSample[]> {
  const socket = new JobSocket(socketUrl('ws'), { topics: [], maxRetries: 1 })
  try {
    const items = await withTimeout(
      socket.fetchAll('truth', { limit: 20_000, maxItems: 200_000 }),
      SOCKET_TIMEOUT_MS,
    )
    const truth = items.filter(isTruthSample)
    if (truth.length > 0) {
      return truth
    }
  } catch {
    // Reported by the fallback path below, which is what the panel uses.
  } finally {
    socket.close()
  }
  return simulations.loadAllTruth()
}

/** Resolves with `promise`, or rejects once `ms` have passed. */
function withTimeout<T>(promise: Promise<T>, ms: number): Promise<T> {
  let timer: ReturnType<typeof setTimeout> | undefined
  const expired = new Promise<T>((_resolve, reject) => {
    timer = setTimeout(() => reject(new Error('the job socket did not answer in time')), ms)
  })
  return Promise.race([promise, expired]).then(
    (value) => {
      if (timer !== undefined) {
        clearTimeout(timer)
      }
      return value
    },
    (error: unknown) => {
      if (timer !== undefined) {
        clearTimeout(timer)
      }
      throw error instanceof Error ? error : new Error(String(error))
    },
  )
}

/** Absolute `ws://` URL of the job's socket; a relative path is resolved against the page. */
function socketUrl(suffix: string): string {
  const url = new URL(
    api.url(`simulations/${encodeURIComponent(jobId.value)}/${suffix}`),
    location.origin,
  )
  url.protocol = url.protocol === 'https:' ? 'wss:' : 'ws:'
  return url.toString()
}

/** Narrows one chunk item to a truth sample. */
function isTruthSample(value: unknown): value is TruthSample {
  if (typeof value !== 'object' || value === null) {
    return false
  }
  const record = value as Record<string, unknown>
  return (
    typeof record.time_s === 'number' &&
    typeof record.position === 'object' &&
    record.position !== null
  )
}

/** Builds the engine, the surface and the trajectory line. */
async function startScene(): Promise<void> {
  const canvasElement = canvas.value
  if (canvasElement === null) {
    return
  }
  const probe = await probeEngine({ forced: engineFromQuery() })
  if (disposed) {
    return
  }
  if (probe.backend === null) {
    failure.value = t('map.viewer.engineFailed')
    return
  }
  const borrowing = await renderHost().acquire(canvasElement, probe.backend)
  if (disposed) {
    // The panel went away while the engine was starting; giving the lease straight
    // back parks the canvas rather than leaving a render loop on a canvas nobody sees.
    borrowing.release()
    return
  }
  lease = borrowing
  scene = borrowing.scene
  terrain = new TerrainLayer(scene.scene, scene)
  line = new TrajectoryLine(scene.scene)
  updateDebug({
    engine: probe.backend,
    mapId: simulations.current?.map_id ?? null,
    frames: 0,
    loaded: false,
    error: null,
  })
  await loadSurface()
  if (disposed) {
    return
  }
  redraw()
}

/** Loads the map's elevation chunks once, at a level the whole extent fits. */
async function loadSurface(): Promise<void> {
  const mapId = simulations.current?.map_id ?? ''
  if (mapId === '' || scene === null || terrain === null) {
    return
  }
  try {
    const info = await maps.loadMetadata(mapId)
    if (disposed) {
      return
    }
    scene.frameBounds(info.summary.bounds)
    const elevation = info.layers.find((layer) => layer.layer_id === LAYER_ELEVATION)
    if (elevation === undefined) {
      return
    }
    const grid = await maps.loadGrid(mapId, LAYER_ELEVATION)
    if (disposed) {
      return
    }
    const level = selectLevel(grid, TARGET_METRES_PER_PIXEL)
    const refs: ChunkRef[] = []
    for (const chunkId of grid.chunks[level] ?? []) {
      refs.push({ mapId, layerId: LAYER_ELEVATION, level, chunkId })
    }
    await maps.loadChunks(refs, { concurrency: 4 })
    if (disposed) {
      return
    }
    const sample = createCellSampler({
      chunks: maps.chunks,
      grid,
      chunkSize: info.summary.chunk_size,
      level,
      layerId: LAYER_ELEVATION,
    })
    // Second channel of the elevation layer; `null` when the map carries no building mask.
    const building = createCellSampler({
      chunks: maps.chunks,
      grid,
      chunkSize: info.summary.chunk_size,
      level,
      layerId: LAYER_ELEVATION,
      channel: 1,
    })
    for (const chunkRef of refs) {
      const key = chunkKey(chunkRef.layerId, chunkRef.level, chunkRef.chunkId)
      if (maps.chunkOf(key) === null) {
        continue
      }
      terrain.setChunk(
        key,
        buildChunkMesh(
          chunkGrid(grid, info.summary.chunk_size, level, chunkRef.chunkId, maps.originOf(mapId)),
          sample,
          { building },
        ),
      )
    }
    // One sampler for both jobs the surface height is needed for: the camera target riding
    // the ground, and the trajectory line being held above it. It reads the height the
    // mesh is drawn at rather than the raw cell, because a line draped on the raw cells
    // sinks under the ramp between them and the run disappears in patches.
    const ground = meshSurfaceSampler(sample, levelCellSize(grid, level), maps.originOf(mapId))
    scene.setGroundHeight((x, y) => ground(x, y))
    surface = ground
  } catch (error) {
    failure.value = isApiError(error) ? error.message : t('simulation.route.surfaceFailed')
  }
}

/** Redraws the trajectory line for the current channel and sample count. */
function redraw(): void {
  line?.setTrajectory(points.value, { surface: surface ?? undefined, lift: TRAJECTORY_LIFT_M })
}

/**
 * Draws the part of the line already run.
 *
 * The playhead is a *sample* index over the whole recording while the line is drawn from a
 * downsampled view of it, so the two are matched by time rather than by index: the drawn
 * point the playhead has reached is the last one whose time is at or before the current
 * sample's. Matching them by index instead put the band's end at the wrong place on the
 * line as soon as the recording was longer than the drawing cap.
 */
function syncProgress(): void {
  if (line === null) {
    return
  }
  const sample = current.value
  if (sample === null) {
    line.setProgress(0)
    return
  }
  const drawn = points.value
  let reached = -1
  for (let at = 0; at < drawn.length; at += 1) {
    const point = drawn[at]
    if (point !== undefined && point.time_s <= sample.time_s) {
      reached = at
    } else {
      break
    }
  }
  line.setProgress(reached)
}

/** Moves the marker to the playhead, orienting it by the sample's own attitude. */
function syncMarker(): void {
  const sample = current.value
  if (sample === null || line === null) {
    return
  }
  line.setPose(
    {
      time_s: sample.time_s,
      x: sample.position.x,
      y: sample.position.y,
      z: sample.z,
      value: sample.speed,
    },
    {
      heading_rad: sample.heading_rad,
      pitch_rad: sample.pitch_rad,
      roll_rad: sample.roll_rad,
    },
  )
}

watch(channel, () => {
  redraw()
  syncProgress()
})

watch(index, () => {
  syncMarker()
  syncProgress()
})

onMounted(() => {
  void load()
})

onBeforeUnmount(() => {
  disposed = true
  if (frame !== null) {
    cancelAnimationFrame(frame)
    frame = null
  }
  line?.dispose()
  terrain?.dispose()
  lease?.release()
  lease = null
  scene = null
  surface = null
  updateDebug({ mapId: null, loaded: false, error: null })
})
</script>

<template>
  <div class="flex flex-col gap-6" data-testid="trajectory-view">
    <TAlert
      v-if="failure !== null"
      theme="error"
      :message="t('simulation.trajectory.loadFailed')"
      data-testid="trajectory-error"
    >
      <p class="text-sm text-muted">{{ failure }}</p>
    </TAlert>
    <p v-else-if="status === 'loading'" class="text-sm text-muted" data-testid="trajectory-loading">
      {{ t('simulation.trajectory.loading') }}
    </p>

    <div class="grid grid-cols-4 gap-6">
      <div class="col-span-3">
        <div class="relative h-[28rem] rounded-card border border-line bg-surface">
          <div ref="canvas" class="block h-full w-full" data-testid="trajectory-canvas" />
          <p class="absolute bottom-3 left-3 font-mono text-xs text-muted">{{ positionText }}</p>
        </div>
        <Timeline
          v-if="times.length > 0"
          class="mt-4"
          :times="times"
          :model-value="index"
          @update:model-value="(value) => (index = value)"
        />
      </div>

      <aside class="flex flex-col gap-5">
        <PlaybackControls
          :playing="playing"
          :rate="rate"
          :enabled="samples.length > 0"
          @toggle="togglePlayback"
          @step="step"
          @rewind="rewind"
          @update:rate="(value) => (rate = value)"
        />
        <ChannelPicker v-model="channel" />
        <SpeedGauge :speed="current?.speed ?? 0" :min="speedRange.min" :max="speedRange.max" />
        <TCard :title="t('simulation.live.state')" size="small">
          <dl class="flex flex-col gap-1 text-sm">
            <div class="flex justify-between gap-3">
              <dt class="text-muted">{{ t('simulation.trajectory.time') }}</dt>
              <dd class="font-mono text-ink">{{ format(current?.time_s ?? 0, 2) }} s</dd>
            </div>
            <div class="flex justify-between gap-3">
              <dt class="text-muted">{{ t('simulation.trajectory.position') }}</dt>
              <dd class="font-mono text-ink">{{ positionText }}</dd>
            </div>
            <div class="flex justify-between gap-3">
              <dt class="text-muted">{{ t('simulation.trajectory.speed') }}</dt>
              <dd class="font-mono text-ink">{{ speedText }} {{ speedLabel }}</dd>
            </div>
            <div class="flex justify-between gap-3">
              <dt class="text-muted">{{ t('simulation.trajectory.chartGrade') }}</dt>
              <dd class="font-mono text-ink">{{ format(current?.grade ?? 0, 4) }}</dd>
            </div>
          </dl>
        </TCard>
      </aside>
    </div>

    <div v-if="samples.length > 0" class="flex flex-col gap-3">
      <h2 class="text-base font-medium text-ink">{{ t('simulation.trajectory.charts') }}</h2>
      <div class="grid grid-cols-2 gap-6">
        <EChart :option="speedOption" :height="240" data-testid="chart-speed" />
        <EChart :option="altitudeOption" :height="240" data-testid="chart-altitude" />
        <EChart :option="gradeOption" :height="240" data-testid="chart-grade" />
        <EChart :option="curvatureOption" :height="240" />
      </div>
    </div>
  </div>
</template>
