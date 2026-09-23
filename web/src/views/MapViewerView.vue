<script setup lang="ts">
/*
 * Map preview: elevation surface, layer drapes, vector overlays and the renderer
 * status.
 *
 * The view owns no geometry of its own: it reads metadata through the maps store,
 * streams the chunks of the visible area into it, and hands the decoded chunks to
 * the render layer. Everything the renderer must know about the map is derived
 * here, which keeps `src/render/*` free of Vue state.
 *
 * The viewport bar above the canvas is the exception, and it is the point of the bar: a
 * parameter set there is applied to the shared scene (and, when it is baked into the
 * surface, the loaded chunks are rebuilt from the cache without re-reading anything).
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
  Switch as TSwitch,
  Tag as TTag,
} from 'tdesign-vue-next'
import { isApiError } from '@/api/errors'
import type { ChunkRef } from '@/api/maps'
import { channelRange, type LayerGrid, type MapMetadata } from '@/api/types'
import {
  LAYER_ELEVATION,
  chunkKey,
  defaultMapping,
  drapeSourceLevel,
  flagColour,
  layerName,
  levelCellSize,
  selectLevel,
  type ColorMapping,
} from '@/types/map'
import {
  CATEGORY_PALETTE,
  SCALAR_RAMPS,
  TERRAIN_RAMP,
  TERRAIN_RAMP_DARK,
  rampCss,
} from '@/types/colormap'
import { backendLabel, engineFromQuery, probeEngine, type EngineReasonCode } from '@/render/engine'
import { updateDebug } from '@/render/scene'
import { renderHost, type SceneLease } from '@/render/host'
import type { MapScene } from '@/render/scene'
import { TerrainLayer } from '@/render/terrain'
import { buildChunkMesh, chunkGrid, type ChunkMeshData } from '@/render/terrainMesh'
import { createCellSampler, type CellSampler } from '@/render/cellSampler'
import { cellReport, createSurfaceSampler, type CellReport } from '@/render/inspect'
import {
  surfaceKey as buildSurfaceKey,
  surfaceStyle as buildSurfaceStyle,
  type SurfaceStyleInput,
} from '@/render/surfaceStyle'
import { gridPaths, terracedSampler } from '@/render/drape'
import { attachGroundPan, metresPerPixelAt } from '@/render/pan'
import { DirectionArrows } from '@/render/directionArrows'
import { MapPin } from '@/render/markers'
import { SURFACE_MATERIALS, layerSurfacePlan, featureDimensions } from '@/render/surfaces'
import { SurfaceGrid } from '@/render/lines'
import { legendEntries, type LegendLayerInput, type LegendModel } from '@/render/legend'
import AppIcon from '@/components/layout/AppIcon.vue'
import Legend from '@/components/map/Legend.vue'
import InspectorPanel from '@/components/map/InspectorPanel.vue'
import ScaleBar from '@/components/map/ScaleBar.vue'
import ViewportControls from '@/components/map/ViewportControls.vue'
import { LayerOverlay } from '@/render/layers'
import { drapedMesh, layerTextureData, texelsPerCell } from '@/render/layerTexture'
import { OverlaySet, type OverlayKind } from '@/render/overlays'
import {
  createOverlaySync,
  overlayAvailability as familiesAvailable,
  syncOverlays,
} from '@/render/overlaySync'
import { pickGround, type PickSource } from '@/render/picking'
import EChart from '@/components/charts/EChart.vue'
import { histogramOption } from '@/components/charts/options/histogram'
import { useMapsStore } from '@/stores/maps'
import { useNotificationsStore } from '@/stores/notifications'
import { useThemeStore } from '@/stores/theme'
import { useViewerStore, type LayerView } from '@/stores/viewer'
import TTooltip from '@/components/common/AppTooltip.vue'

/** Samples the height distribution is computed from; a chart does not need a whole map. */
const HEIGHT_SAMPLE_LIMIT = 20_000

/** How many chunks one streaming pass may request. */
const MAX_CHUNKS_PER_PASS = 24

/** Milliseconds a camera change is coalesced before the visible chunks are re-read. */
const STREAM_DEBOUNCE_MS = 200

/** Milliseconds between two hover reads; a pointer move fires far more often. */
const HOVER_THROTTLE_MS = 60

/** Target number of grid lines per axis when the grid step is left to the viewer. */
const GRID_TARGET_LINES = 20

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
const theme = useThemeStore()
const notifications = useNotificationsStore()

const mapId = computed(() => String(route.params.id ?? ''))
/** Container the shared canvas is moved into. */
const host = ref<HTMLDivElement | null>(null)
const metadata = ref<MapMetadata | null>(null)
const status = ref<'idle' | 'loading' | 'ready' | 'failed'>('idle')
const failureKey = ref<string | null>(null)
const failureDetail = ref<string | null>(null)
const pickSource = ref<PickSource>('miss')
const scaleBarMetres = ref(100)
/** Ground resolution the camera shows, metres per CSS pixel; drives the scale bar. */
const groundResolution = ref(1)
/** The pinned cell, and the report the panel renders. */
const pinned = ref<CellReport | null>(null)
const hover = ref<CellReport | null>(null)
/**
 * Elevation range the ramp spans, from the map's own `GLOBAL_STATS`.
 *
 * The map's range rather than the loaded view's: a ramp that rescaled itself as chunks
 * arrived would recolour the ground while the reader watched, and two chunks drawn
 * from different ranges would disagree about the same height.
 */
const surfaceRange = computed(() => channelRange(metadata.value?.global_stats, LAYER_ELEVATION))

let scene: MapScene | null = null
let lease: SceneLease | null = null
let terrain: TerrainLayer | null = null
let drapes: LayerOverlay | null = null
let overlays: OverlaySet | null = null
let arrows: DirectionArrows | null = null
let pin: MapPin | null = null
let detachPan: (() => void) | null = null
let scaleGrid: SurfaceGrid | null = null
let streamTimer: ReturnType<typeof setTimeout> | null = null
let pointerDown: { x: number; y: number } | null = null
let lastHoverMs = 0
let streaming = false
let disposed = false

/** Terrain geometry per chunk, kept so a layer drape can reuse its surface. */
const terrainData = new Map<string, ChunkMeshData>()
/** Chunks the drape manager has already drawn, keyed by layer and chunk. */
const drapedChunks = new Set<string>()
/** Overlay families already read from the service. */
const overlayLoaded = createOverlaySync()

/** The cell the panel describes: the pinned one, or whatever the pointer is over. */
const inspector = computed(() => pinned.value ?? hover.value)

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

/**
 * Whether the map carries what an overlay family draws.
 *
 * The section counts come with the metadata, so the panel can say an overlay has
 * nothing to show before the user switches it on and waits for a request.
 */
/** Which families the map carries, so a control can say so before it is switched on. */
const overlayAvailability = computed(() => familiesAvailable(metadata.value))

const mappingOptions = computed(() => [
  { value: 'material', label: t('map.viewer.mappingMaterial') },
  { value: 'category', label: t('map.viewer.mappingCategory') },
  { value: 'flag', label: t('map.viewer.mappingFlag') },
  { value: 'cost', label: t('map.viewer.mappingCost') },
  { value: 'grey', label: t('map.viewer.mappingGrey') },
  { value: 'speed', label: t('map.viewer.mappingSpeed') },
  { value: 'height', label: t('map.viewer.mappingHeight') },
])

/**
 * Everything baked into the surface geometry and colours.
 *
 * Collected in one place because the same set of values decides the vertex data and the
 * decision to rebuild it: a parameter that changes the data has to be in both.
 */
const surfaceStyle = computed(() => buildSurfaceStyle(surfaceStyleInput()))

/** Identity of the baked parameters, so a change can be told from a re-render. */
const surfaceKey = computed(() => buildSurfaceKey(surfaceStyleInput()))

/** The viewport values the surface is derived from. */
function surfaceStyleInput(): SurfaceStyleInput {
  return {
    range: surfaceRange.value,
    dark: theme.isDark,
    terraceM: viewer.terraceStepM,
    sunAzimuthDeg: viewer.sunAzimuthDeg,
  }
}

/** The ramp the surface is drawn with, for its legend entry. */
const surfaceRamp = computed(() => (theme.isDark ? TERRAIN_RAMP_DARK : TERRAIN_RAMP))

/**
 * Elevation samples of the chunks already loaded.
 *
 * The terrain and this distribution come from the same decoded chunks, so the
 * chart describes exactly what is on screen rather than the map's global statistics.
 */
const elevationSamples = computed<number[]>(() => loadSamples(LAYER_ELEVATION))

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

/** Thinned samples of one layer's loaded chunks, for the legend and the histogram. */
function loadSamples(layerId: number): number[] {
  const values: number[] = []
  for (const chunk of maps.chunks.values()) {
    if (chunk.layerId !== layerId) {
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
}

/** Cell size the viewer is drawing at, metres, as text. */
const cellSizeText = computed(() => {
  const grid = maps.gridOf(mapId.value, LAYER_ELEVATION)
  return grid === null ? '—' : `${number(levelCellSize(grid, viewer.terrainLevel))} m`
})

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

/** Dimensions the map's own feature schema declares, which name what a layer means. */
const featureDims = computed(() => featureDimensions(metadata.value?.feature_schema))

/** How one layer's cells are textured: its material table, its pattern. */
function surfacePlan(layer: LayerView) {
  return layerSurfacePlan(layer.layerId, layer.kind, featureDims.value)
}

/**
 * The legend: the surface, then every draped layer that is switched on.
 *
 * The surface comes first and is always described, because it is always drawn: the reader
 * who asked "what is the light grey" gets an answer even with every layer off.
 */
const legendModel = computed<LegendModel>(() => {
  const surface: LegendLayerInput = {
    layerId: LAYER_ELEVATION,
    name: layerName(LAYER_ELEVATION),
    mapping: 'grey',
    range: surfaceRange.value,
    ramp: surfaceRamp.value,
    unit: t('map.legend.elevation'),
  }
  const draped = viewer.visibleLayers
    .filter((layer) => layer.layerId !== LAYER_ELEVATION && drapeable(layer))
    .map((layer) => ({
      layerId: layer.layerId,
      name: layer.name,
      mapping: layer.mapping,
      range: channelRange(metadata.value?.global_stats, layer.layerId),
      samples: loadSamples(layer.layerId),
      materials: surfacePlan(layer).materials,
      pattern: surfacePlan(layer).pattern,
    }))
  return legendEntries([surface, ...draped], { limit: 6 })
})

/** Unit shown beside a layer's ticks, when one is known. */
function unitFor(layerId: number): string | null {
  return layerId === LAYER_ELEVATION ? t('map.legend.elevation') : null
}

/** CSS gradient of a layer's ramp, drawn under the mapping select. */
function rampStyle(mapping: ColorMapping): string {
  if (mapping === 'material') {
    return rampCss(SURFACE_MATERIALS.map((material) => material.colour))
  }
  if (mapping === 'category') {
    return rampCss(CATEGORY_PALETTE)
  }
  if (mapping === 'flag') {
    return rampCss([
      [217, 217, 220],
      [176, 48, 44],
    ])
  }
  return rampCss(SCALAR_RAMPS[mapping])
}

/** Failure of the whole view: the message is translated, the service's text is kept beside it. */
function fail(key: string, error?: unknown): void {
  status.value = 'failed'
  failureKey.value = key
  failureDetail.value = isApiError(error) ? error.message : null
}

onMounted(async () => {
  const container = host.value
  if (container === null) {
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
  let borrowing: SceneLease | null = null
  try {
    borrowing = await renderHost().acquire(container, probe.backend)
  } catch (error) {
    fail('map.viewer.engineFailed', error)
    return
  }
  if (disposed) {
    // The view went away while the engine was starting; giving the lease straight back
    // parks the canvas rather than leaving a second render loop running.
    borrowing.release()
    return
  }
  lease = borrowing
  scene = borrowing.scene
  terrain = new TerrainLayer(scene.scene, scene)
  drapes = new LayerOverlay(scene.scene)
  overlays = new OverlaySet(scene)
  arrows = new DirectionArrows(scene)
  pin = new MapPin(scene)
  pin.hide()
  scaleGrid = new SurfaceGrid(scene)
  updateDebug({ engine: probe.backend, mapId: mapId.value, frames: 0, loaded: false, error: null })
  applyViewport()
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
        // A click is what pins the inspector: the hover readout follows the pointer,
        // and only an explicit choice keeps a cell on screen while the camera moves.
        readPick()
        pinned.value = hover.value
        placePin()
      }
    }
  }
  scene.scene.onPointerMove = () => {
    // Throttled: a pointer move fires many times per frame, and reading a cell walks
    // the loaded chunks.
    const now = Date.now()
    if (now - lastHoverMs < HOVER_THROTTLE_MS) {
      return
    }
    lastHoverMs = now
    readPick()
  }
  scene.camera.onViewMatrixChangedObservable.add(() => {
    scheduleStream()
    readPick()
  })
  // The middle button pans, as in every other map; the right one does too, because a plan view
  // is read with it and nothing else on this page wants the button.
  detachPan = attachGroundPan(scene.scene, scene.camera, {
    buttons: [1, 2],
    metresPerPixel,
    onPanned: () => readPick(),
  })
  observeCamera()
  window.addEventListener('keydown', onKeyDown)
  await loadMap()
})

/** Reports the camera back into the store, so the rail shows what the reader is looking at. */
function observeCamera(): void {
  scene?.observeCamera((state) => {
    // The pointer rotates the camera, so the pitch the panel shows is the camera's own.
    viewer.pitchDeg = state.pitchDeg
    groundResolution.value = metresPerPixel()
    scaleBarMetres.value = scene?.scaleBarLength(groundResolution.value) ?? scaleBarMetres.value
  })
}

/** Releases the pinned cell when the reader presses Escape. */
function onKeyDown(event: KeyboardEvent): void {
  if (event.key === 'Escape') {
    pinned.value = null
    pin?.hide()
  }
}

onBeforeUnmount(() => {
  disposed = true
  window.removeEventListener('keydown', onKeyDown)
  if (streamTimer !== null) {
    clearTimeout(streamTimer)
    streamTimer = null
  }
  detachPan?.()
  detachPan = null
  terrain?.dispose()
  drapes?.dispose()
  overlays?.dispose()
  arrows?.dispose()
  pin?.dispose()
  scaleGrid?.dispose()
  lease?.release()
  lease = null
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
    const dims = featureDimensions(info.feature_schema)
    viewer.setLayers(
      info.layers.map((layer) => ({
        layerId: layer.layer_id,
        name: layerName(layer.layer_id),
        // A dimension that declares materials is drawn as those materials — a surface type is a
        // road or a lawn, not a number — and the panel says so instead of offering a ramp.
        mapping:
          layerSurfacePlan(layer.layer_id, layer.kind, dims).materials === null
            ? defaultMapping(layer)
            : 'material',
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
    scene?.setSun(viewer.sunAzimuthDeg)
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
      dropSurface()
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
    buildTerrain(grid, chunkSize, level, refs)
    refreshGrid()
    await syncOverlayFamilies()
    await streamLayerDrapes(level, chunkSize, area)
  } finally {
    streaming = false
  }
}

/** Drops the surface so the next pass rebuilds it from the cached chunks. */
function dropSurface(): void {
  terrain?.clear()
  terrainData.clear()
  drapes?.clear()
  drapedChunks.clear()
  scaleGrid?.clear()
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
  const elevationGrid = maps.gridOf(mapId.value, LAYER_ELEVATION)
  if (manager === null || elevationGrid === null) {
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
    const layerGrid = maps.gridOf(mapId.value, layer.layerId)
    const sourceLevel = layerGrid === null ? null : drapeSourceLevel(layerGrid, level)
    if (layerGrid === null || sourceLevel === null) {
      continue
    }
    // The layer's own chunks are read at the level the layer stores, which is not
    // necessarily the level the surface is drawn at.
    const refs = maps
      .chunkRefsInArea(mapId.value, layer.layerId, chunkSize, sourceLevel, area)
      .slice(0, MAX_CHUNKS_PER_PASS)
    // oxlint-disable-next-line no-await-in-loop
    await maps.loadChunks(refs)
    const sample = cellSamplerFor(layer.layerId, layerGrid, chunkSize, level, sourceLevel)
    // The drape is one mesh per *terrain* chunk: its vertices are the surface's own, and
    // its grid is the surface's grid — a chunk id from the layer's own level names a
    // different area and would put the drape somewhere the chunk is not.
    for (const [key, terrainMesh] of terrainData) {
      const drapeKey = `${layer.layerId}:${level}:${key}`
      if (drapedChunks.has(drapeKey) || !key.startsWith(`${LAYER_ELEVATION}/`)) {
        continue
      }
      const spec = chunkGrid(
        elevationGrid,
        chunkSize,
        level,
        chunkIdOf(key),
        maps.originOf(mapId.value),
      )
      const plan = surfacePlan(layer)
      manager.setChunk(
        drapeKey,
        layerTextureData(spec, sample, {
          mapping: layer.mapping,
          emptyAlpha: 0,
          flag: flagColour(layer.layerId),
          surface: plan,
          texelsPerCell: texelsPerCell(plan),
        }),
        drapedMesh(terrainMesh, DRAPE_LIFT_M * drapeOrder(layer.layerId)),
      )
      drapedChunks.add(drapeKey)
    }
  }
}

/** Chunk id inside a `layer/level/chunk` key. */
function chunkIdOf(key: string): number {
  return Number(key.split('/')[2] ?? 0)
}

/** Ground resolution the camera currently shows, metres per CSS pixel. */
function metresPerPixel(): number {
  const current = scene
  const element = host.value
  if (current === null || element === null || element.clientHeight === 0) {
    return 1
  }
  return metresPerPixelAt(current.camera.radius, element.clientHeight, current.camera.fov)
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

/**
 * Plants the pin where the reader pinned a cell.
 *
 * The panel says "this cell"; the pin says *which* cell, in the map's own space, which is what a
 * reader needs while the camera keeps moving.
 */
function placePin(): void {
  const marker = pin
  const report = pinned.value
  if (marker === null) {
    return
  }
  if (report === null) {
    marker.hide()
    return
  }
  marker.place(report.position.x, report.position.y, report.elevation ?? 0)
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
  pickSource.value = result.source
  groundResolution.value = metresPerPixel()
  scaleBarMetres.value = current.scaleBarLength(groundResolution.value)
  // The cell the pointer is over, so the panel has something to describe. Reading it
  // touches only the chunks already in memory: a pick never fetches.
  hover.value =
    result.point === null
      ? null
      : cellReport({
          metadata: metadata.value as MapMetadata,
          grid: maps.gridOf(mapId.value, LAYER_ELEVATION),
          chunkSize: metadata.value?.summary.chunk_size ?? 0,
          level: viewer.terrainLevel,
          chunks: maps.chunks,
          point: result.point,
        })
}

/**
 * A cell reader for one layer, over the chunks already loaded.
 *
 * The layer is read at the finest level it stores that is no finer than the surface — see
 * `drapeSourceLevel` — so a layer that only exists at one level still draws on a surface
 * drawn from another.
 */
function cellSamplerFor(
  layerId: number,
  grid: LayerGrid,
  chunkSize: number,
  level: number,
  sourceLevel: number = level,
): CellSampler {
  return createCellSampler({
    chunks: maps.chunks,
    grid,
    chunkSize,
    level,
    layerId,
    sourceLevel,
  })
}

/**
 * Builds every mesh whose surface is ready.
 *
 * A chunk's mesh needs its neighbours' cells to draw the quad that spans the boundary, so
 * a chunk whose neighbour is not loaded yet is built with what is known and rebuilt when
 * that neighbour arrives — see {@link buildTerrain}.
 */
function buildTerrain(
  grid: LayerGrid,
  chunkSize: number,
  level: number,
  refs: readonly ChunkRef[],
): void {
  const manager = terrain
  if (manager === null) {
    return
  }
  const origin = maps.originOf(mapId.value)
  const sample = cellSamplerFor(LAYER_ELEVATION, grid, chunkSize, level)
  for (const target of refs) {
    const key = chunkKey(target.layerId, target.level, target.chunkId)
    if (maps.chunkOf(key) === null || terrainData.has(key)) {
      continue
    }
    const data = buildChunkMesh(
      chunkGrid(grid, chunkSize, target.level, target.chunkId, origin),
      sample,
      surfaceStyle.value,
    )
    terrainData.set(key, data)
    manager.setChunk(key, data)
  }
}

/**
 * The sampler the overlays and the grid read their heights from.
 *
 * Terraced, so an annotation lands on the surface that is drawn rather than on the survey
 * under it.
 */
function surfaceSampler(): (x: number, y: number) => number | null {
  const gridRef = maps.gridOf(mapId.value, LAYER_ELEVATION)
  if (gridRef === null || metadata.value === null) {
    return () => null
  }
  return terracedSampler(
    createSurfaceSampler(
      maps.chunks,
      gridRef,
      metadata.value.summary.chunk_size,
      viewer.terrainLevel,
      maps.originOf(mapId.value),
    ),
    surfaceStyle.value.terraceM ?? 0,
  )
}

/** Rebuilds the scale grid from the terrain currently loaded. */
function refreshGrid(): void {
  const manager = scaleGrid
  const bounds = metadata.value?.summary.bounds
  if (manager === null || bounds === undefined) {
    return
  }
  manager.setVisible(viewer.gridVisible)
  if (!viewer.gridVisible) {
    manager.clear()
    return
  }
  const step = viewer.gridStepM ?? gridSpacing(bounds, GRID_TARGET_LINES)
  manager.set(gridPaths(bounds, step, surfaceSampler(), { lift: 0.2 }), step)
}

/** Grid step that gives roughly `lines` lines per axis. */
function gridSpacing(
  bounds: { min_x: number; min_y: number; max_x: number; max_y: number },
  lines: number,
): number {
  const span = Math.max(bounds.max_x - bounds.min_x, bounds.max_y - bounds.min_y, 1)
  const raw = span / Math.max(1, lines)
  const magnitude = 10 ** Math.floor(Math.log10(raw))
  const normalized = raw / magnitude
  const step = normalized >= 5 ? 5 : normalized >= 2 ? 2 : 1
  return step * magnitude
}

/** Brings the overlays in line with the toggles, loading what is missing. */
async function syncOverlayFamilies(): Promise<void> {
  await syncOverlays(overlayLoaded, {
    manager: overlays as OverlaySet,
    arrows,
    mapId: mapId.value,
    metadata: () => metadata.value,
    toggles: () => viewer.overlays,
    level: () => viewer.terrainLevel,
    surface: surfaceSampler,
    chunks: () => maps.chunks,
    gridOf: (layerId) => maps.gridOf(mapId.value, layerId),
    ensureGrid,
    refsInArea: (layerId, level, area) =>
      maps.chunkRefsInArea(mapId.value, layerId, chunkSizeOf(), level, area),
    loadChunks: (refs) => maps.loadChunks(refs, { concurrency: 4 }),
    footprint: cameraFootprint,
    origin: () => maps.originOf(mapId.value),
    onError: (_kind, error) => notifications.pushError(t('map.viewer.metadataFailed'), error),
  })
}

/** Ground resolution the camera currently shows, metres per CSS pixel. */
function chunkSizeOf(): number {
  return metadata.value?.summary.chunk_size ?? 0
}

/**
 * Applies the camera half of the viewport parameters.
 *
 * Separate from the baked half: none of these changes a vertex, so they are applied
 * straight onto the live camera.
 */
function applyViewport(): void {
  const current = scene
  if (current === null) {
    return
  }
  current.setZoomStep(viewer.zoomStep)
  current.setPitch(viewer.pitchDeg)
  current.setFov(viewer.fovDeg)
  current.setOrthographic(viewer.orthographic)
  current.setZoomToPointer(viewer.zoomToPointer)
  current.setSun(viewer.sunAzimuthDeg)
}

/** Applies a viewport parameter the camera carries. */
watch(
  () =>
    [
      viewer.zoomStep,
      viewer.pitchDeg,
      viewer.fovDeg,
      viewer.orthographic,
      viewer.zoomToPointer,
    ].join(','),
  () => applyViewport(),
)

/** Rebuilds the surface when a parameter that is baked into it changes. */
watch(surfaceKey, async () => {
  scene?.setSun(viewer.sunAzimuthDeg)
  dropSurface()
  await streamVisibleChunks()
})

/** Redraws the scale grid when its own parameters change. */
watch(
  () => `${viewer.gridVisible}:${viewer.gridStepM ?? 'auto'}`,
  () => refreshGrid(),
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

/** True when the layer is the surface itself, whose look the viewport bar owns. */
function isSurface(layer: LayerView): boolean {
  return layer.layerId === LAYER_ELEVATION
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
  () => {
    void syncOverlayFamilies()
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
          <!-- The way back to the library, on the page itself: a reader who opened a map from the
               list has to be able to leave it without the browser's back button. -->
          <RouterLink
            to="/maps"
            class="flex items-center gap-1 text-sm text-muted transition-colors hover:text-ink"
            data-testid="viewer-back"
          >
            <AppIcon name="chevron-left" :size="16" />
            {{ t('map.viewer.backToList') }}
          </RouterLink>
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
        <div ref="host" class="block h-full w-full" data-testid="viewer-canvas" />

        <!-- Everything below floats over the canvas. The map is the subject; these are
             read at a glance and must not compete with it, which is why they are 2D
             and why only the inspector is a panel. -->
        <p v-if="status === 'loading'" class="absolute left-4 top-4 text-sm text-muted">
          {{ t('map.viewer.loading') }}
        </p>

        <!-- The viewport rail: icons in the corner of the canvas, panels on demand. -->
        <div class="pointer-events-none absolute left-3 top-3">
          <ViewportControls :range="surfaceRange" @recentre="resetView()" />
        </div>

        <div class="pointer-events-none absolute bottom-4 left-4 flex flex-col gap-3">
          <Legend :model="legendModel" :label-for="layerLabel" :unit-for="unitFor" />
          <ScaleBar :metres="scaleBarMetres" :metres-per-pixel="groundResolution" />
        </div>

        <!-- Anchored to the canvas on both axes so the panel can never be taller than the map it
             describes: a report with nine layers used to run off the bottom of the screen. -->
        <div
          class="pointer-events-none absolute inset-y-4 right-4 flex max-w-[22rem] flex-col items-end"
          data-testid="pick-readout"
        >
          <InspectorPanel
            v-if="inspector !== null"
            class="pointer-events-auto max-h-full"
            :report="inspector"
            :layer-label="layerLabel"
            :on-terrain="pickSource === 'terrain'"
          />
          <p v-else class="glass pointer-events-auto px-3 py-2 text-xs text-muted">
            {{ t('map.inspector.hint') }}
          </p>
        </div>
      </div>
    </div>

    <aside class="w-80 shrink-0 overflow-y-auto border-l border-line bg-surface px-4 py-4">
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
            <!-- The surface's own look is a viewport parameter, not a layer mapping:
                 offering a ramp here contradicted the colour the ground was drawn
                 with, which is how the legend came to describe a mapping in use. -->
            <p v-else-if="isSurface(layer)" class="text-xs text-muted">
              {{ t('map.viewer.layerSurfaceHint') }}
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
            <dd class="text-ink">{{ cellSizeText }}</dd>
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
