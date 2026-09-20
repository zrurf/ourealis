<script setup lang="ts">
/*
 * Map preview: elevation surface, layer drapes, vector overlays and the renderer
 * status.
 *
 * The view owns no geometry of its own: it reads metadata through the maps store,
 * streams the chunks of the visible area into it, and hands the decoded chunks to
 * the render layer. Everything the renderer must know about the map is derived
 * here, which keeps `src/render/*` free of Vue state.
 */
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import { useRoute } from 'vue-router'
import {
  Alert as TAlert,
  Button as TButton,
  Card as TCard,
  Divider as TDivider,
  Select as TSelect,
  Slider as TSlider,
  Switch as TSwitch,
  Tag as TTag,
  Tooltip as TTooltip,
} from 'tdesign-vue-next'
import { isApiError } from '@/api/errors'
import { getConnectors, getPrmGraph, getRegions, getSkeleton } from '@/api/maps'
import type { MapMetadata } from '@/api/types'
import {
  LAYER_ELEVATION,
  chunkKey,
  defaultMapping,
  layerName,
  selectLevel,
  type ColorMapping,
} from '@/types/map'
import { CATEGORY_PALETTE, SCALAR_RAMPS, rampCss } from '@/types/colormap'
import { connectors as readConnectors, prmEdges, prmNodes, regionOutlines } from '@/types/sections'
import { backendLabel, engineFromQuery, probeEngine, type EngineReasonCode } from '@/render/engine'
import { MapScene, updateDebug } from '@/render/scene'
import { TerrainLayer, buildChunkMesh, type ChunkMeshData } from '@/render/terrain'
import { LayerOverlay, drapedMesh, layerTextureData } from '@/render/layers'
import { OverlaySet, type OverlayKind } from '@/render/overlays'
import { pickGround, type GroundPoint, type PickSource } from '@/render/picking'
import EChart from '@/components/charts/EChart.vue'
import { histogramOption } from '@/components/charts/options/histogram'
import { useMapsStore } from '@/stores/maps'
import { useNotificationsStore } from '@/stores/notifications'
import { useViewerStore, type LayerView } from '@/stores/viewer'

/** Samples the height distribution is computed from; a chart does not need a whole map. */
const HEIGHT_SAMPLE_LIMIT = 20_000

/** How many chunks one streaming pass may request. */
const MAX_CHUNKS_PER_PASS = 24

/** Milliseconds a camera change is coalesced before the visible chunks are re-read. */
const STREAM_DEBOUNCE_MS = 200

/** Translation key of each probe reason. */
const REASON_KEYS: Readonly<Record<EngineReasonCode, string>> = {
  available: 'map.viewer.engineReasonAvailable',
  forced: 'map.viewer.engineReasonForced',
  'forced-unavailable': 'map.viewer.engineReasonForcedUnavailable',
  'no-webgl': 'map.viewer.engineReasonNoWebgl',
}

const { t, locale } = useI18n({ useScope: 'global' })
const route = useRoute()
const maps = useMapsStore()
const viewer = useViewerStore()
const notifications = useNotificationsStore()

const mapId = computed(() => String(route.params.id ?? ''))
const canvas = ref<HTMLCanvasElement | null>(null)
const metadata = ref<MapMetadata | null>(null)
const status = ref<'idle' | 'loading' | 'ready' | 'failed'>('idle')
const failureKey = ref<string | null>(null)
const failureDetail = ref<string | null>(null)
const pick = ref<GroundPoint | null>(null)
const pickSource = ref<PickSource>('miss')
const scaleBarMetres = ref(100)

let scene: MapScene | null = null
let terrain: TerrainLayer | null = null
let drapes: LayerOverlay | null = null
let overlays: OverlaySet | null = null
let streamTimer: ReturnType<typeof setTimeout> | null = null
let pointerDown: { x: number; y: number } | null = null
let streaming = false
let disposed = false

/** Terrain geometry per chunk, kept so a layer drape can reuse its surface. */
const terrainData = new Map<string, ChunkMeshData>()
/** Chunks the drape manager has already drawn, keyed by layer and chunk. */
const drapedChunks = new Set<string>()
/** Overlay families already read from the service. */
const overlayLoaded = new Set<OverlayKind>()

const engineText = computed(() =>
  viewer.engineBackend === null
    ? t('map.viewer.engineUnavailable')
    : backendLabel(viewer.engineBackend),
)

const engineReasonKey = computed(() => {
  const reason = viewer.engineReason
  return reason !== null && reason in REASON_KEYS ? REASON_KEYS[reason as EngineReasonCode] : null
})

/** A fallback the user should know about: a backend other than WebGPU was chosen. */
const showFallbackNotice = computed(
  () =>
    !viewer.engineNoticeDismissed &&
    viewer.engineBackend !== null &&
    viewer.engineBackend !== 'webgpu',
)

const exaggerationText = computed(() => viewer.exaggeration.toFixed(1))

/**
 * Whether the map carries what an overlay family draws.
 *
 * The section counts come with the metadata, so the panel can say an overlay has
 * nothing to show before the user switches it on and waits for a request.
 */
const overlayAvailability = computed<Record<OverlayKind, boolean>>(() => ({
  regions: (metadata.value?.sections.regions ?? 0) > 0,
  connectors: (metadata.value?.sections.connectors ?? 0) > 0,
  skeleton: (metadata.value?.skeleton_nodes ?? 0) > 0,
  prm: (metadata.value?.sections.roadmap_batches ?? 0) > 0,
}))

const mappingOptions = computed(() => [
  { value: 'category', label: t('map.viewer.mappingCategory') },
  { value: 'grey', label: t('map.viewer.mappingGrey') },
  { value: 'speed', label: t('map.viewer.mappingSpeed') },
  { value: 'height', label: t('map.viewer.mappingHeight') },
])

/**
 * Elevation samples of the chunks already loaded.
 *
 * The terrain and this distribution come from the same decoded chunks, so the
 * chart describes exactly what is on screen rather than the map's global statistics.
 */
const elevationSamples = computed<number[]>(() => {
  const values: number[] = []
  for (const chunk of maps.chunks.values()) {
    if (chunk.layerId !== LAYER_ELEVATION) {
      continue
    }
    const stride = Math.max(1, Math.ceil(chunk.values.length / (HEIGHT_SAMPLE_LIMIT / 8)))
    for (let index = 0; index < chunk.values.length; index += stride) {
      const value = chunk.values[index]
      if (value !== undefined && Number.isFinite(value)) {
        values.push(value)
      }
    }
  }
  return values
})

/** Histogram of the loaded elevation, in metres. */
const elevationOption = computed(() =>
  histogramOption({
    x: t('map.viewer.heightAxis'),
    y: t('map.viewer.sampleCount'),
    values: elevationSamples.value,
    bins: 24,
    decimals: 0,
  }),
)

/** Localised name of a layer: the service's own names where they are known. */
function layerLabel(layerId: number, serviceName: string): string {
  const keys: Readonly<Record<string, string>> = {
    elevation: 'map.viewer.layerNameElevation',
    slope: 'map.viewer.layerNameSlope',
    edt: 'map.viewer.layerNameEdt',
    hard_forbidden: 'map.viewer.layerNameHardForbidden',
    direction: 'map.viewer.layerNameDirection',
    soft_multiplier: 'map.viewer.layerNameSoftMultiplier',
    regions: 'map.viewer.layerNameRegions',
    prm_graph: 'map.viewer.layerNamePrmGraph',
    kpath_library: 'map.viewer.layerNameKpathLibrary',
    vectors: 'map.viewer.layerNameVectors',
  }
  const feature = /^feature_(\d+)$/.exec(serviceName)
  if (feature?.[1] !== undefined) {
    return t('map.viewer.layerNameFeature', { index: feature[1] })
  }
  const key = keys[serviceName]
  return key === undefined ? t('map.viewer.layerNameOther', { id: layerId }) : t(key)
}

/** Formats a number in the interface locale. */
function number(value: number, fractionDigits = 2): string {
  return new Intl.NumberFormat(locale.value, { maximumFractionDigits: fractionDigits }).format(
    value,
  )
}

/** CSS gradient of the layer's ramp, drawn under the mapping select. */
function rampStyle(mapping: ColorMapping): string {
  return rampCss(mapping === 'category' ? CATEGORY_PALETTE : SCALAR_RAMPS[mapping])
}

/** Failure of the whole view: the message is translated, the service's text is kept beside it. */
function fail(key: string, error?: unknown): void {
  status.value = 'failed'
  failureKey.value = key
  failureDetail.value = isApiError(error) ? error.message : null
}

onMounted(async () => {
  const canvasElement = canvas.value
  if (canvasElement === null) {
    return
  }
  const probe = await probeEngine({ forced: engineFromQuery() })
  if (disposed) {
    return
  }
  viewer.setEngine(probe.backend, probe.reason)
  if (probe.backend === null) {
    updateDebug({ engine: null, error: probe.reason })
    fail('map.viewer.engineFailed')
    return
  }
  status.value = 'loading'
  let created: MapScene | null = null
  try {
    created = await MapScene.create({ canvas: canvasElement, backend: probe.backend })
  } catch (error) {
    fail('map.viewer.engineFailed', error)
    return
  }
  if (disposed) {
    // The view went away while the engine was starting; its render loop would keep
    // running on a canvas nobody can see.
    created.dispose()
    return
  }
  scene = created
  terrain = new TerrainLayer(scene.scene, scene)
  drapes = new LayerOverlay(scene.scene)
  overlays = new OverlaySet(scene.scene)
  updateDebug({ engine: probe.backend, mapId: mapId.value, frames: 0, loaded: false, error: null })
  // A click that ends a camera drag would otherwise move the readout to wherever the
  // drag stopped, so a pick is only read when the pointer stayed where it went down.
  scene.scene.onPointerDown = () => {
    pointerDown = { x: scene?.scene.pointerX ?? 0, y: scene?.scene.pointerY ?? 0 }
  }
  scene.scene.onPointerUp = () => {
    const start = pointerDown
    const current = scene?.scene
    pointerDown = null
    if (start !== null && current !== undefined) {
      const travelled = Math.hypot(current.pointerX - start.x, current.pointerY - start.y)
      if (travelled <= 4) {
        readPick()
      }
    }
  }
  scene.camera.onViewMatrixChangedObservable.add(() => {
    scheduleStream()
  })
  await loadMap()
})

onBeforeUnmount(() => {
  disposed = true
  if (streamTimer !== null) {
    clearTimeout(streamTimer)
    streamTimer = null
  }
  terrain?.dispose()
  drapes?.dispose()
  overlays?.dispose()
  scene?.dispose()
  scene = null
  terrainData.clear()
  drapedChunks.clear()
  maps.clearChunks()
  updateDebug({ mapId: null, loaded: false, error: null })
})

/** Reads metadata, frames the camera and starts streaming the visible chunks. */
async function loadMap(): Promise<void> {
  try {
    const info = await maps.loadMetadata(mapId.value)
    metadata.value = info
    viewer.reset(mapId.value)
    // The store keeps the service's own layer name; the panel translates it, so the
    // name is not translated twice.
    viewer.setLayers(
      info.layers.map((layer) => ({
        layerId: layer.layer_id,
        name: layerName(layer.layer_id),
        mapping: defaultMapping(layer),
        kind: layer.kind,
      })),
    )
    const elevation = info.layers.find((layer) => layer.layer_id === LAYER_ELEVATION)
    if (elevation === undefined) {
      // Without elevation there is no surface to draw, but the overlays and the
      // metadata are still worth showing.
      status.value = 'ready'
      return
    }
    await maps.loadGrid(mapId.value, LAYER_ELEVATION)
    scene?.frameBounds(info.summary.bounds)
    status.value = 'ready'
    await streamVisibleChunks()
  } catch (error) {
    fail('map.viewer.metadataFailed', error)
    updateDebug({ error: isApiError(error) ? error.message : 'metadata failed' })
  }
}

/** Coalesces a burst of camera changes into one streaming pass. */
function scheduleStream(): void {
  if (disposed || streamTimer !== null) {
    return
  }
  streamTimer = setTimeout(() => {
    streamTimer = null
    void streamVisibleChunks()
  }, STREAM_DEBOUNCE_MS)
}

/**
 * Loads the chunks of the visible area at the level the camera can show.
 *
 * The level comes from the ground resolution the camera sees, so a zoomed-out view
 * reads coarse chunks and a zoomed-in one replaces them with fine chunks; a level
 * change drops the meshes built at the previous level rather than mixing them.
 */
async function streamVisibleChunks(): Promise<void> {
  const info = metadata.value
  const current = scene
  if (streaming || disposed || info === null || current === null || terrain === null) {
    return
  }
  const grid = maps.gridOf(mapId.value, LAYER_ELEVATION)
  if (grid === null) {
    return
  }
  streaming = true
  try {
    const chunkSize = info.summary.chunk_size
    const level = selectLevel(grid, metresPerPixel())
    if (level !== viewer.terrainLevel) {
      terrain.clear()
      terrainData.clear()
      drapes?.clear()
      drapedChunks.clear()
      viewer.terrainLevel = level
    }
    const area = cameraFootprint()
    const refs = maps.chunkRefsInArea(mapId.value, LAYER_ELEVATION, chunkSize, level, area)
    const missing = refs.filter(
      (target) => !terrainData.has(chunkKey(target.layerId, target.level, target.chunkId)),
    )
    await maps.loadChunks(missing.slice(0, MAX_CHUNKS_PER_PASS), {
      onError: () => {
        // A chunk that fails leaves a hole; the count is reported under the canvas.
      },
    })
    for (const target of refs) {
      const key = chunkKey(target.layerId, target.level, target.chunkId)
      const chunk = maps.chunkOf(key)
      if (chunk === null || terrainData.has(key)) {
        continue
      }
      const data = buildChunkMesh(chunk, grid, chunkSize)
      terrainData.set(key, data)
      terrain.setChunk(key, data)
    }
    await streamLayerDrapes(level, chunkSize, area)
  } finally {
    streaming = false
  }
}

/** Reads a layer's chunk geometry once, so a later pass finds it cached. */
async function ensureGrid(layerId: number): Promise<void> {
  if (maps.gridOf(mapId.value, layerId) === null) {
    await maps.loadGrid(mapId.value, layerId)
  }
}

/** Drapes every visible non-elevation layer over the terrain it covers. */
async function streamLayerDrapes(
  level: number,
  chunkSize: number,
  area: { min_x: number; min_y: number; max_x: number; max_y: number },
): Promise<void> {
  const manager = drapes
  if (manager === null) {
    return
  }
  for (const layer of viewer.visibleLayers) {
    if (layer.layerId === LAYER_ELEVATION) {
      continue
    }
    // A region, vector or graph layer is one payload, not a grid of cells: it has no
    // chunks to drape, and its own overlay is the way to show it.
    if (!drapeable(layer)) {
      continue
    }
    // Layers are draped one after another: each grid is read on demand, and a burst
    // of layer loads on one connection would starve the terrain chunks.
    // oxlint-disable-next-line no-await-in-loop
    await ensureGrid(layer.layerId)
    const refs = maps
      .chunkRefsInArea(mapId.value, layer.layerId, chunkSize, level, area)
      .slice(0, MAX_CHUNKS_PER_PASS)
    // oxlint-disable-next-line no-await-in-loop
    await maps.loadChunks(refs)
    for (const target of refs) {
      const key = chunkKey(target.layerId, target.level, target.chunkId)
      const chunk = maps.chunkOf(key)
      if (chunk === null) {
        continue
      }
      // Every layer shares the chunk grid, so the surface to drape on is the
      // *elevation* chunk with the same level and id — looking it up under this
      // layer's own key found nothing and silently attached no drape at all.
      const terrainMesh = terrainData.get(chunkKey(LAYER_ELEVATION, target.level, target.chunkId))
      const drapeKey = `${layer.layerId}:${key}`
      if (terrainMesh === undefined || drapedChunks.has(drapeKey)) {
        continue
      }
      manager.setChunk(
        drapeKey,
        layerTextureData(chunk, { mapping: layer.mapping, emptyAlpha: 0 }),
        drapedMesh(terrainMesh, DRAPE_LIFT_M * drapeOrder(layer.layerId)),
      )
      drapedChunks.add(drapeKey)
    }
  }
}

/** Ground resolution the camera currently shows, metres per CSS pixel. */
function metresPerPixel(): number {
  const current = scene
  const element = canvas.value
  if (current === null || element === null || element.clientHeight === 0) {
    return 1
  }
  return (current.camera.radius * 2) / element.clientHeight
}

/** Area the camera can see, as the map's own metre plane. */
function cameraFootprint(): { min_x: number; min_y: number; max_x: number; max_y: number } {
  const info = metadata.value
  const target = scene?.camera.target
  const radius = scene?.camera.radius ?? 100
  const half = Math.max(20, radius * 0.9)
  const centerX = target?.x ?? 0
  const centerY = target?.z ?? 0
  const bounds = info?.summary.bounds
  return {
    min_x: Math.max(bounds?.min_x ?? centerX - half, centerX - half),
    min_y: Math.max(bounds?.min_y ?? centerY - half, centerY - half),
    max_x: Math.min(bounds?.max_x ?? centerX + half, centerX + half),
    max_y: Math.min(bounds?.max_y ?? centerY + half, centerY + half),
  }
}

/** Reads the ground point under the last pointer position and reports it. */
function readPick(): void {
  const current = scene
  if (current === null) {
    return
  }
  const result = pickGround(current.scene, current.scene.pointerX, current.scene.pointerY, {
    targets: terrain?.list ?? [],
  })
  pick.value = result.point
  pickSource.value = result.source
}

/** Reads an optional section the first time its overlay is switched on. */
async function ensureOverlay(kind: OverlayKind): Promise<void> {
  if (overlayLoaded.has(kind) || overlays === null) {
    return
  }
  overlayLoaded.add(kind)
  try {
    switch (kind) {
      case 'regions': {
        const section = await getRegions(mapId.value)
        overlays.setRegions(regionOutlines(section.json))
        break
      }
      case 'connectors': {
        const section = await getConnectors(mapId.value)
        overlays.setConnectors(readConnectors(section.json))
        break
      }
      case 'skeleton': {
        const skeleton = await getSkeleton(mapId.value)
        overlays.setSkeleton(skeleton.nodes)
        break
      }
      case 'prm': {
        const section = await getPrmGraph(mapId.value)
        overlays.setPrm(prmNodes(section.json), prmEdges(section.json))
        break
      }
    }
    overlays.setVisible(kind, viewer.overlays[kind])
  } catch (error) {
    overlayLoaded.delete(kind)
    notifications.pushError(t('map.viewer.metadataFailed'), error)
  }
}

/** Applies the exaggeration control to the scene. */
watch(
  () => viewer.exaggeration,
  (value) => {
    scene?.setExaggeration(value)
    scaleBarMetres.value = scene?.scaleBarLength ?? scaleBarMetres.value
  },
)

/**
 * Height between two stacked drapes, metres.
 *
 * Small enough to be invisible against a terrain that spans tens of metres, large
 * enough that two enabled layers do not fight for the same depth.
 */
const DRAPE_LIFT_M = 0.05

/**
 * Stacking order of a draped layer: elevation is the surface itself, so everything
 * else is lifted above it.
 */
function drapeOrder(layerId: number): number {
  const index = viewer.visibleLayers.findIndex((layer) => layer.layerId === layerId)
  return Math.max(1, index + 1)
}

/**
 * Layer kinds that have cells to drape.
 *
 * Matches `LayerKind::is_chunked` on the service side: everything else is an opaque
 * section (regions, vectors, graphs) with a dedicated overlay.
 */
const DRAPEABLE_KINDS = new Set(['raster', 'bitmap'])

/**
 * True when a layer can be drawn as a drape over the terrain.
 *
 * A section layer has no cells, so its switch cannot change the surface; the panel
 * reports that instead of offering a control that would do nothing.
 */
function drapeable(layer: LayerView): boolean {
  return DRAPEABLE_KINDS.has(layer.kind)
}

/** Redraws the drapes when a layer's visibility or mapping changes. */
watch(
  () =>
    viewer.layers.map((layer) => `${layer.layerId}:${layer.visible}:${layer.mapping}`).join(','),
  () => {
    const elevation = viewer.layers.find((layer) => layer.layerId === LAYER_ELEVATION)
    terrain?.setVisible(elevation?.visible ?? true)
    drapedChunks.clear()
    drapes?.clear()
    void streamVisibleChunks()
  },
)

/** Switches an overlay family on or off. */
watch(
  () => ({ ...viewer.overlays }),
  (toggles) => {
    for (const [kind, visible] of Object.entries(toggles) as Array<[OverlayKind, boolean]>) {
      if (visible) {
        void ensureOverlay(kind)
      } else {
        overlays?.setVisible(kind, false)
      }
    }
  },
  { deep: true },
)

/** Frames the map again on demand. */
function resetView(): void {
  const bounds = metadata.value?.summary.bounds
  if (bounds !== undefined && scene !== null) {
    scene.frameBounds(bounds)
    scheduleStream()
  }
}
</script>

<template>
  <section class="flex h-[calc(100vh-3.5rem)] min-h-0">
    <div class="flex min-w-0 flex-1 flex-col">
      <div class="flex items-center justify-between gap-3 border-b border-line px-6 py-3">
        <div class="flex items-center gap-3">
          <h1 class="font-semibold text-ink">{{ t('views.mapViewer.title') }}</h1>
          <span v-if="metadata !== null" class="text-sm text-muted">{{
            metadata.summary.name
          }}</span>
        </div>
        <div class="flex items-center gap-3">
          <span class="text-xs text-muted">{{ t('map.viewer.engine') }}</span>
          <TTooltip :content="engineReasonKey === null ? '' : t(engineReasonKey)">
            <TTag :theme="viewer.engineBackend === null ? 'danger' : 'success'" variant="light">
              <span data-testid="engine-chip">{{ engineText }}</span>
            </TTag>
          </TTooltip>
        </div>
      </div>

      <TAlert
        v-if="status === 'failed'"
        class="m-4"
        theme="error"
        :message="failureKey === null ? t('common.error') : t(failureKey)"
        data-testid="viewer-error"
      >
        <p v-if="failureDetail !== null" class="text-sm text-muted">
          <TTag size="small" variant="light">{{ t('map.viewer.fromService') }}</TTag>
          <span class="ml-2">{{ failureDetail }}</span>
        </p>
      </TAlert>

      <TAlert
        v-else-if="showFallbackNotice"
        class="m-4"
        theme="warning"
        :title="t('map.viewer.engineFallbackTitle')"
        :message="t('map.viewer.engineFallbackBody', { backend: engineText })"
        data-testid="engine-notice"
        @close="viewer.engineNoticeDismissed = true"
      />

      <div class="relative min-h-0 flex-1">
        <canvas ref="canvas" class="block h-full w-full" data-testid="viewer-canvas" />
        <p v-if="status === 'loading'" class="absolute left-4 top-4 text-sm text-muted">
          {{ t('map.viewer.loading') }}
        </p>
        <p class="absolute bottom-4 left-4 text-xs text-muted">
          {{ t('map.viewer.scaleBar', { metres: number(scaleBarMetres, 0) }) }}
        </p>
        <div class="absolute bottom-4 right-4 text-xs text-muted" data-testid="pick-readout">
          <span v-if="pick === null">{{ t('map.viewer.pickHint') }}</span>
          <span v-else>
            {{
              t('map.viewer.pickResult', {
                x: number(pick.x),
                y: number(pick.y),
                z: number(pick.z),
              })
            }}
            <em v-if="pickSource === 'plane'">{{ t('map.viewer.pickPlane') }}</em>
          </span>
        </div>
      </div>
    </div>

    <aside class="w-80 shrink-0 overflow-y-auto border-l border-line bg-surface px-4 py-4">
      <section>
        <h2 class="text-sm font-medium text-ink">{{ t('map.viewer.exaggeration') }}</h2>
        <div class="mt-2 flex items-center gap-3">
          <TSlider
            :value="viewer.exaggeration"
            :min="0.5"
            :max="3"
            :step="0.1"
            class="flex-1"
            @change="viewer.setExaggeration(Number($event))"
          />
          <span class="w-12 text-right text-sm text-muted" data-testid="exaggeration-value">
            {{ t('map.viewer.exaggerationValue', { value: exaggerationText }) }}
          </span>
        </div>
      </section>

      <TDivider class="my-4" />

      <section>
        <h2 class="text-sm font-medium text-ink">{{ t('map.viewer.layers') }}</h2>
        <p v-if="viewer.layers.length === 0" class="mt-2 text-sm text-muted">
          {{ t('map.viewer.layersEmpty') }}
        </p>
        <ul v-else class="mt-2 flex flex-col gap-3">
          <li v-for="layer in viewer.layers" :key="layer.layerId" class="flex flex-col gap-1">
            <div class="flex items-center justify-between gap-2">
              <span class="text-sm text-ink">{{ layerLabel(layer.layerId, layer.name) }}</span>
              <TSwitch
                :value="drapeable(layer) && layer.visible"
                size="small"
                :disabled="!drapeable(layer)"
                :aria-label="
                  t('map.viewer.layerToggle', { name: layerLabel(layer.layerId, layer.name) })
                "
                :data-testid="`layer-${layer.layerId}`"
                @change="viewer.setLayerVisible(layer.layerId, Boolean($event))"
              />
            </div>
            <!-- A section layer carries one payload rather than a grid, so there is
                 nothing to drape over the terrain; the row says so instead of
                 offering a switch whose state the surface cannot show. -->
            <p v-if="!drapeable(layer)" class="text-xs text-muted">
              {{ t('map.viewer.layerSectionHint') }}
            </p>
            <template v-else-if="layer.visible">
              <TSelect
                :value="layer.mapping"
                :options="mappingOptions"
                size="small"
                @change="viewer.setLayerMapping(layer.layerId, $event as ColorMapping)"
              />
              <div
                class="h-1.5 w-full rounded-control"
                :style="{ background: rampStyle(layer.mapping) }"
              />
            </template>
          </li>
        </ul>
      </section>

      <TDivider class="my-4" />

      <section>
        <h2 class="text-sm font-medium text-ink">{{ t('map.viewer.overlays') }}</h2>
        <ul class="mt-2 flex flex-col gap-2">
          <li
            v-for="kind in ['regions', 'connectors', 'skeleton', 'prm'] as OverlayKind[]"
            :key="kind"
            class="flex items-center justify-between gap-2"
          >
            <TTooltip
              :content="overlayAvailability[kind] ? '' : t('map.viewer.overlayUnavailable')"
            >
              <span class="text-sm text-ink">{{
                t(
                  `map.viewer.overlay${kind === 'prm' ? 'Prm' : kind.charAt(0).toUpperCase() + kind.slice(1)}`,
                )
              }}</span>
            </TTooltip>
            <TSwitch
              :value="viewer.overlays[kind]"
              size="small"
              :disabled="!overlayAvailability[kind]"
              :data-testid="`overlay-${kind}`"
              @change="viewer.setOverlay(kind, Boolean($event))"
            />
          </li>
        </ul>
      </section>

      <TDivider class="my-4" />

      <section>
        <h2 class="text-sm font-medium text-ink">{{ t('map.viewer.stats') }}</h2>
        <dl class="mt-2 flex flex-col gap-2 text-sm">
          <div class="flex justify-between gap-3">
            <dt class="text-muted">{{ t('map.viewer.level') }}</dt>
            <dd class="text-ink" data-testid="lod-level">
              {{ t('map.viewer.levelValue', { level: viewer.terrainLevel }) }}
            </dd>
          </div>
          <div class="flex justify-between gap-3">
            <dt class="text-muted">{{ t('map.viewer.resolution') }}</dt>
            <dd class="text-ink">
              {{ metadata === null ? '—' : `${number(metadata.summary.base_res_m)} m` }}
            </dd>
          </div>
          <div class="flex justify-between gap-3">
            <dt class="text-muted">{{ t('map.viewer.chunkSize') }}</dt>
            <dd class="text-ink">
              {{
                metadata === null
                  ? '—'
                  : `${metadata.summary.chunk_size} / ${metadata.summary.lod_count}`
              }}
            </dd>
          </div>
          <div class="flex justify-between gap-3">
            <dt class="text-muted">{{ t('map.viewer.statChunks') }}</dt>
            <dd class="text-ink" data-testid="chunk-count">{{ maps.loadedChunkCount }}</dd>
          </div>
          <div class="flex justify-between gap-3">
            <dt class="text-muted">{{ t('map.viewer.statSkeletonNodes') }}</dt>
            <dd class="text-ink">{{ metadata?.skeleton_nodes ?? '—' }}</dd>
          </div>
        </dl>
        <p
          v-if="maps.failures.size > 0"
          class="mt-2 text-sm text-danger"
          data-testid="chunk-failures"
        >
          {{ t('map.viewer.chunkFailures', { count: maps.failures.size }) }}
        </p>
        <TButton class="mt-3 w-full" variant="outline" @click="resetView()">
          {{ t('map.viewer.resetView') }}
        </TButton>
      </section>

      <section v-if="elevationSamples.length > 0" class="mt-4">
        <h2 class="text-sm font-medium text-ink">{{ t('map.viewer.heightDistribution') }}</h2>
        <EChart class="mt-2" :option="elevationOption" :height="160" />
      </section>

      <TCard
        v-if="metadata !== null && metadata.derived.length > 0"
        class="mt-4"
        :title="t('map.viewer.derivedTitle')"
        size="small"
      >
        <ul class="flex flex-col gap-1">
          <li
            v-for="layer in metadata.derived"
            :key="layer.layer_id"
            class="flex items-center gap-2"
          >
            <span class="text-sm text-ink">{{ layer.name }}</span>
            <TTag
              size="small"
              variant="light"
              :theme="
                layer.status === 'valid'
                  ? 'success'
                  : layer.status === 'stale'
                    ? 'warning'
                    : 'default'
              "
            >
              {{
                t(
                  `map.viewer.derived${layer.status.charAt(0).toUpperCase() + layer.status.slice(1)}`,
                )
              }}
            </TTag>
            <TTooltip v-if="layer.reason != null" :content="layer.reason">
              <span class="text-xs text-muted">{{ t('map.viewer.fromService') }}</span>
            </TTooltip>
          </li>
        </ul>
      </TCard>
    </aside>
  </section>
</template>
