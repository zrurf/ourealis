<script setup lang="ts">
/*
 * Route studio: the ground plane the route is drawn on, the candidate set and the
 * speed limit along the chosen path.
 *
 * Planning is the cheapest loop in the tool — `/routes/preview` runs the same
 * environment and configuration a job does and stops before the motion stage — so
 * this page is built around trying routes rather than running them. The map is a
 * surface and a picker: the form owns the numbers, a click on the surface fills
 * the field the user armed, and the geometry on screen is redrawn from the form
 * every time it changes.
 */
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import { useRouter } from 'vue-router'
import {
  Alert as TAlert,
  Button as TButton,
  Card as TCard,
  Select as TSelect,
  Table as TTable,
  Tag as TTag,
} from 'tdesign-vue-next'
import { CreateLineSystem } from '@babylonjs/core/Meshes/Builders/linesBuilder'
import { CreateSphere } from '@babylonjs/core/Meshes/Builders/sphereBuilder'
import { StandardMaterial } from '@babylonjs/core/Materials/standardMaterial'
import { Color3 } from '@babylonjs/core/Maths/math.color'
import { Vector3 } from '@babylonjs/core/Maths/math.vector'
import type { Mesh } from '@babylonjs/core/Meshes/mesh'
import type { LinesMesh } from '@babylonjs/core/Meshes/linesMesh'
import { isApiError } from '@/api/errors'
import type { ChunkRef } from '@/api/maps'
import { listPresets } from '@/api/presets'
import { planRoute, previewRoutes } from '@/api/routes'
import type { RoutePreview } from '@/types/result'
import { LAYER_ELEVATION, chunkKey, selectLevel } from '@/types/map'
import { INK_LIGHT, SERIES_PALETTE, rgbToHex } from '@/types/colormap'
import { engineFromQuery, probeEngine } from '@/render/engine'
import { MapScene, updateDebug } from '@/render/scene'
import { TerrainLayer, buildChunkMesh } from '@/render/terrain'
import { pickGround } from '@/render/picking'
import EChart from '@/components/charts/EChart.vue'
import { lineOption } from '@/components/charts/options/line'
import RouteForm from '@/components/forms/RouteForm.vue'
import {
  buildSimulationRequest,
  defaultFormState,
  pointToVec,
  validateForm,
  type PickTarget,
  type SimulationFormState,
} from '@/components/forms/request'
import { useMapsStore } from '@/stores/maps'
import { useNotificationsStore } from '@/stores/notifications'
import { formatMetric } from '@/stores/simulations'

/** Resolution the camera is treated as showing when the level is chosen, m/px. */
const TARGET_METRES_PER_PIXEL = 2

const { t, locale } = useI18n({ useScope: 'global' })
const router = useRouter()
const maps = useMapsStore()
const notifications = useNotificationsStore()

const canvas = ref<HTMLCanvasElement | null>(null)
const state = ref<SimulationFormState>(defaultFormState())
const presets = ref<Array<{ value: string; label: string }>>([])
const pending = ref<PickTarget | null>(null)
const preview = ref<RoutePreview | null>(null)
const planned = ref<RoutePreview | null>(null)
const status = ref<'idle' | 'loading' | 'ready' | 'failed'>('idle')
const failure = ref<string | null>(null)
const busy = ref<'preview' | 'plan' | null>(null)

let scene: MapScene | null = null
let terrain: TerrainLayer | null = null
let markerMeshes: Mesh[] = []
let candidateMeshes: LinesMesh[] = []
let markerMaterial: StandardMaterial | null = null

/** The map the studio plans against. */
const mapId = computed(() => state.value.mapId ?? maps.summaries[0]?.id ?? '')

/** Points the marker layer draws. */
const markers = computed(() => {
  const out: Array<{ x: number; y: number }> = []
  const start = pointToVec(state.value.start)
  const goal = pointToVec(state.value.goal)
  if (start !== null) {
    out.push(start)
  }
  if (goal !== null && state.value.mode !== 'loop') {
    out.push(goal)
  }
  if (state.value.mode === 'loop') {
    const reference = pointToVec(state.value.reference)
    if (reference !== null) {
      out.push(reference)
    }
  }
  for (const waypoint of state.value.waypoints) {
    const position = pointToVec(waypoint.position)
    if (position !== null) {
      out.push(position)
    }
  }
  for (const checkpoint of state.value.checkpoints) {
    const position = pointToVec(checkpoint.position)
    if (position !== null) {
      out.push(position)
    }
  }
  return out
})

/** Rows of the candidate table. */
const candidateRows = computed(() =>
  (preview.value?.candidates ?? []).map((candidate, index) => ({
    key: String(index),
    index,
    chosen: index === preview.value?.chosen,
    length: formatMetric(candidate.length_m, 'm', locale.value, 2),
    cost: formatMetric(candidate.cost_equiv_m, 'm', locale.value, 2),
    probability: formatMetric(candidate.probability, '−', locale.value, 4),
    pathSize: formatMetric(candidate.path_size, '−', locale.value, 3),
    library: candidate.from_library ? t('omf.inspector.yes') : t('omf.inspector.no'),
  })),
)

const columns = computed(() => [
  { colKey: 'index', title: '#', width: 60 },
  { colKey: 'length', title: t('simulation.route.length'), width: 110 },
  { colKey: 'cost', title: t('simulation.route.cost'), width: 110 },
  { colKey: 'probability', title: t('simulation.route.probability'), width: 120 },
  { colKey: 'pathSize', title: t('simulation.route.pathSize'), width: 110 },
  { colKey: 'library', title: t('simulation.route.fromLibrary'), width: 110 },
])

/** Summary line of the preview. */
const previewSummary = computed(() => {
  const result = preview.value
  if (result === null) {
    return []
  }
  return [
    {
      key: 'straight',
      label: t('simulation.route.straightLine'),
      value: formatMetric(result.straight_line_m, 'm', locale.value, 2),
    },
    {
      key: 'length',
      label: t('simulation.route.length'),
      value: formatMetric(result.length_m, 'm', locale.value, 2),
    },
    {
      key: 'cost',
      label: t('simulation.route.cost'),
      value: formatMetric(result.cost_equiv_m, 'm', locale.value, 2),
    },
    {
      key: 'planning',
      label: t('simulation.route.planningMs'),
      value: formatMetric(result.planning_ms, 'ms', locale.value, 1),
    },
  ]
})

/** Speed limit along the smoothed path, against arc length. */
const speedLimitOption = computed(() => {
  const result = planned.value
  if (result === null || result.speed_limit_s.length === 0) {
    return {}
  }
  return lineOption({
    x: t('simulation.route.speedLimitAxis'),
    y: t('simulation.route.speedAxis'),
    title: t('simulation.route.speedLimit'),
    series: [
      {
        name: t('simulation.route.speedLimit'),
        data: result.speed_limit_s.map((arc, index) => ({
          x: arc,
          y: result.speed_limit_mps[index] ?? 0,
        })),
      },
    ],
  })
})

/** Validation issues of the route draft. */
const issues = computed(() => validateForm(state.value))

/** True when the route is complete enough to plan. */
const planable = computed(() => issues.value.length === 0 || issues.value.every(isRoutePointIssue))

/** Issues that do not stop planning: the seed and the individual are always valid. */
function isRoutePointIssue(issue: { field: string }): boolean {
  return issue.field === 'laps' || issue.field === 'checkpoints'
}

/** Stores the engine and starts the surface for the selected map. */
/**
 * In-flight scene creation.
 *
 * The mount path and the map watcher both call this, and the awaits below let them
 * interleave: without a shared promise each would build its own Babylon engine and
 * render loop on the same canvas, and only the last one would ever be disposed.
 */
let sceneStart: Promise<void> | null = null

async function startScene(): Promise<void> {
  if (sceneStart !== null) {
    return sceneStart
  }
  sceneStart = createScene().finally(() => {
    sceneStart = null
  })
  return sceneStart
}

/** Builds the scene once; callers go through {@link startScene}. */
async function createScene(): Promise<void> {
  const canvasElement = canvas.value
  if (canvasElement === null || scene !== null) {
    return
  }
  const probe = await probeEngine({ forced: engineFromQuery() })
  if (probe.backend === null) {
    failure.value = t('map.viewer.engineFailed')
    status.value = 'failed'
    return
  }
  scene = await MapScene.create({ canvas: canvasElement, backend: probe.backend })
  terrain = new TerrainLayer(scene.scene, scene)
  markerMaterial = new StandardMaterial('studioMarker', scene.scene)
  markerMaterial.disableLighting = true
  markerMaterial.emissiveColor = Color3.FromHexString(markerColour())
  updateDebug({ engine: probe.backend, mapId: mapId.value, frames: 0, loaded: false, error: null })
  scene.scene.onPointerUp = () => {
    readPick()
  }
}

/** Loads the selected map's elevation surface. */
async function loadSurface(): Promise<void> {
  const id = mapId.value
  if (id === '' || scene === null || terrain === null) {
    return
  }
  status.value = 'loading'
  failure.value = null
  try {
    const info = await maps.loadMetadata(id)
    scene.frameBounds(info.summary.bounds)
    const elevation = info.layers.find((layer) => layer.layer_id === LAYER_ELEVATION)
    if (elevation === undefined) {
      status.value = 'ready'
      return
    }
    const grid = await maps.loadGrid(id, LAYER_ELEVATION)
    const level = selectLevel(grid, TARGET_METRES_PER_PIXEL)
    const refs: ChunkRef[] = (grid.chunks[level] ?? []).map((chunkId) => ({
      mapId: id,
      layerId: LAYER_ELEVATION,
      level,
      chunkId,
    }))
    await maps.loadChunks(refs, { concurrency: 4 })
    for (const chunkRef of refs) {
      const key = chunkKey(chunkRef.layerId, chunkRef.level, chunkRef.chunkId)
      const chunk = maps.chunkOf(key)
      if (chunk !== null) {
        terrain.setChunk(key, buildChunkMesh(chunk, grid, info.summary.chunk_size))
      }
    }
    status.value = 'ready'
  } catch (error) {
    failure.value = isApiError(error) ? error.message : String(error)
    status.value = 'failed'
  }
}

/** Reads a click on the surface and fills the armed field. */
function readPick(): void {
  const current = scene
  const target = pending.value
  if (current === null) {
    return
  }
  const result = pickGround(current.scene, current.scene.pointerX, current.scene.pointerY, {
    targets: terrain?.list ?? [],
  })
  if (result.point === null) {
    return
  }
  if (target === null) {
    return
  }
  const point = { x: round(result.point.x), y: round(result.point.y) }
  state.value = applyPick(state.value, target, point)
  pending.value = null
  notifications.push({
    kind: 'info',
    message: t('simulation.route.picked', {
      x: formatMetric(point.x, '', locale.value, 2),
      y: formatMetric(point.y, '', locale.value, 2),
    }),
  })
}

/** Rounds a picked coordinate to a centimetre. */
function round(value: number): number {
  return Math.round(value * 100) / 100
}

/** Applies a picked point to the field the user armed. */
function applyPick(
  current: SimulationFormState,
  target: PickTarget,
  point: { x: number; y: number },
): SimulationFormState {
  switch (target.kind) {
    case 'start':
      return { ...current, start: point }
    case 'goal':
      return { ...current, goal: point }
    case 'reference':
      return { ...current, reference: point }
    case 'waypoint': {
      const waypoints = current.waypoints.map((waypoint, index) =>
        index === target.index ? { ...waypoint, position: point } : waypoint,
      )
      return { ...current, waypoints }
    }
    case 'checkpoint': {
      const checkpoints = current.checkpoints.map((checkpoint, index) =>
        index === target.index ? { ...checkpoint, position: point } : checkpoint,
      )
      return { ...current, checkpoints }
    }
  }
}

/** Arms a field for the next click. */
function arm(target: PickTarget): void {
  pending.value = target
}

/** Draws the markers of the route draft. */
function drawMarkers(): void {
  const current = scene
  if (current === null || markerMaterial === null) {
    return
  }
  for (const mesh of markerMeshes) {
    mesh.dispose()
  }
  markerMeshes = markers.value.map((point, index) => {
    const sphere = CreateSphere(`routeMarker:${index}`, { diameter: 3 }, current.scene)
    sphere.position = new Vector3(point.x, 1.5, point.y)
    sphere.material = markerMaterial
    sphere.isPickable = false
    return sphere
  })
}

/** Colour of the route markers: the palette's first entry, or the neutral ink. */
function markerColour(): string {
  return SERIES_PALETTE[0] ?? rgbToHex(INK_LIGHT)
}

/** Draws the candidate paths, the chosen one first and in the first palette colour. */
function drawCandidates(): void {
  const current = scene
  if (current === null) {
    return
  }
  for (const mesh of candidateMeshes) {
    // Each candidate owns its material, and `Mesh.dispose` leaves a material alive.
    mesh.dispose(false, true)
  }
  candidateMeshes = []
  const result = preview.value
  if (result === null) {
    return
  }
  result.candidates.forEach((candidate, index) => {
    const color = index === result.chosen ? SERIES_PALETTE[0] : SERIES_PALETTE[(index % 6) + 1]
    const lines = [candidate.points.map((point) => new Vector3(point.x, 2, point.y))]
    const mesh = CreateLineSystem(`candidate:${index}`, { lines, updatable: false }, current.scene)
    const material = new StandardMaterial(`candidateMaterial:${index}`, current.scene)
    material.disableLighting = true
    material.emissiveColor = Color3.FromHexString(color ?? markerColour())
    mesh.material = material
    mesh.isPickable = false
    candidateMeshes.push(mesh)
  })
  const path = result.path
  if (path.length > 1) {
    const mesh = CreateLineSystem(
      'plannedPath',
      { lines: [path.map((point) => new Vector3(point.x, 3, point.y))], updatable: false },
      current.scene,
    )
    const material = new StandardMaterial('plannedPathMaterial', current.scene)
    material.disableLighting = true
    material.emissiveColor = Color3.FromHexString(markerColour())
    mesh.material = material
    mesh.isPickable = false
    candidateMeshes.push(mesh)
  }
}

/** Assembles the request and runs a preview. */
async function runPreview(): Promise<void> {
  busy.value = 'preview'
  try {
    const request = buildSimulationRequest(state.value)
    preview.value = await previewRoutes(request)
    planned.value = null
    drawCandidates()
  } catch (error) {
    notifications.pushError(t('simulation.route.previewFailed'), error)
  } finally {
    busy.value = null
  }
}

/** Assembles the request and plans with the speed profile. */
async function runPlan(): Promise<void> {
  busy.value = 'plan'
  try {
    const request = buildSimulationRequest(state.value)
    planned.value = await planRoute(request)
    preview.value = planned.value
    drawCandidates()
  } catch (error) {
    notifications.pushError(t('simulation.route.previewFailed'), error)
  } finally {
    busy.value = null
  }
}

/** Submits the route as a run. */
async function runIt(): Promise<void> {
  void router.push('/simulations/new')
}

/**
 * Drops the surface and everything drawn on it.
 *
 * The terrain leaves its height meshes registered with the scene, so it is disposed
 * before the engine goes; the candidate meshes own their materials, which their own
 * disposal would otherwise leave behind.
 */
function disposeScene(): void {
  for (const mesh of candidateMeshes) {
    mesh.dispose(false, true)
  }
  candidateMeshes = []
  for (const mesh of markerMeshes) {
    mesh.dispose()
  }
  markerMeshes = []
  markerMaterial?.dispose()
  markerMaterial = null
  terrain?.dispose()
  terrain = null
  scene?.dispose()
  scene = null
}

watch(mapId, async () => {
  disposeScene()
  await startScene()
  await loadSurface()
})

watch(markers, () => drawMarkers(), { deep: true })

onMounted(async () => {
  await maps.loadMaps()
  if (state.value.mapId === null && maps.summaries[0] !== undefined) {
    state.value = { ...state.value, mapId: maps.summaries[0].id }
  }
  try {
    const page = await listPresets()
    presets.value = page.items.map((preset) => ({ value: preset.preset, label: preset.preset }))
    if (page.items[0] !== undefined && !page.items.some((p) => p.preset === state.value.preset)) {
      state.value = { ...state.value, preset: page.items[0].preset }
    }
  } catch {
    // The studio works with whatever preset the state already carries.
  }
  await startScene()
  await loadSurface()
  drawMarkers()
})

onBeforeUnmount(() => {
  disposeScene()
  updateDebug({ mapId: null, loaded: false, error: null })
})
</script>

<template>
  <section class="mx-auto max-w-7xl px-8 py-8" data-testid="route-studio">
    <div class="flex items-center justify-between gap-4">
      <div class="flex items-center gap-3">
        <h1 class="font-semibold text-ink">{{ t('views.routeStudio.title') }}</h1>
        <TTag v-if="pending !== null" size="small" variant="light" theme="warning">
          {{ t('simulation.route.picking') }}
        </TTag>
      </div>
      <div class="flex items-center gap-2">
        <label class="w-40">
          <TSelect
            :value="state.preset"
            :options="presets"
            :placeholder="t('simulation.person.preset')"
            data-testid="studio-preset"
            @change="(value) => (state = { ...state, preset: String(value) })"
          />
        </label>
        <TButton
          theme="primary"
          variant="outline"
          :loading="busy === 'preview'"
          :disabled="!planable"
          data-testid="route-preview"
          @click="runPreview()"
        >
          {{
            busy === 'preview' ? t('simulation.route.previewing') : t('simulation.route.preview')
          }}
        </TButton>
        <TButton
          theme="primary"
          :loading="busy === 'plan'"
          :disabled="!planable"
          data-testid="route-plan"
          @click="runPlan()"
        >
          {{ busy === 'plan' ? t('simulation.route.planning') : t('simulation.route.plan') }}
        </TButton>
        <TButton variant="outline" :disabled="!planable" @click="runIt()">
          {{ t('simulation.form.submit') }}
        </TButton>
      </div>
    </div>

    <TAlert
      v-if="failure !== null"
      class="mt-4"
      theme="error"
      :message="t('simulation.route.surfaceFailed')"
      data-testid="studio-error"
    >
      <p class="text-sm text-muted">
        <TTag size="small" variant="light">{{ t('simulation.live.fromService') }}</TTag>
        <span class="ml-2">{{ failure }}</span>
      </p>
    </TAlert>

    <div class="mt-4 grid grid-cols-3 gap-6">
      <div class="col-span-2">
        <div class="relative h-[30rem] rounded-card border border-line bg-surface">
          <canvas ref="canvas" class="block h-full w-full" data-testid="studio-canvas" />
          <p class="absolute bottom-3 left-3 text-xs text-muted">
            {{ t('simulation.route.pickHint') }}
          </p>
          <p v-if="status === 'loading'" class="absolute left-3 top-3 text-sm text-muted">
            {{ t('common.loading') }}
          </p>
        </div>
      </div>

      <aside class="flex flex-col gap-4">
        <TCard :title="t('simulation.route.mapSurface')" size="small">
          <TSelect
            :value="state.mapId ?? ''"
            :options="maps.summaries.map((map) => ({ value: map.id, label: map.name }))"
            :disabled="maps.summaries.length === 0"
            :placeholder="t('simulation.form.map')"
            data-testid="studio-map"
            @change="(value) => (state = { ...state, mapId: String(value) })"
          />
          <p class="mt-2 text-xs text-muted">{{ t('simulation.form.mapHint') }}</p>
          <p v-if="maps.summaries.length === 0" class="mt-2 text-sm text-muted">
            {{ t('simulation.list.empty') }}
          </p>
        </TCard>
      </aside>
    </div>

    <TCard class="mt-6" :title="t('simulation.form.groupRoute')" size="small">
      <RouteForm v-model="state" :maps="maps.summaries" pickable @pick="arm" />
    </TCard>

    <div class="mt-6 grid grid-cols-2 gap-6">
      <TCard :title="t('simulation.route.candidates')" size="small">
        <p v-if="candidateRows.length === 0" class="text-sm text-muted">
          {{ t('simulation.route.empty') }}
        </p>
        <template v-else>
          <dl class="flex flex-col gap-1 text-sm">
            <div v-for="row in previewSummary" :key="row.key" class="flex justify-between gap-3">
              <dt class="text-muted">{{ row.label }}</dt>
              <dd class="font-mono text-ink">{{ row.value }}</dd>
            </div>
          </dl>
          <TTable
            class="mt-3"
            :data="candidateRows"
            :columns="columns"
            row-key="key"
            size="small"
            data-testid="candidate-table"
          >
            <template #index="{ row }">
              <span :class="row.chosen ? 'font-semibold text-brand' : 'text-ink'">
                {{ row.index }}
                <TTag v-if="row.chosen" size="small" variant="light" theme="success">
                  {{ t('simulation.route.chosen') }}
                </TTag>
              </span>
            </template>
          </TTable>
        </template>
      </TCard>

      <TCard :title="t('simulation.route.speedLimit')" size="small">
        <p v-if="planned === null" class="text-sm text-muted">
          {{ t('simulation.route.plan') }}
        </p>
        <EChart v-else :option="speedLimitOption" :height="280" data-testid="speed-limit-chart" />
      </TCard>
    </div>
  </section>
</template>
