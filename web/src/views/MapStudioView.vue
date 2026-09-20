<script setup lang="ts">
/*
 * Map studio: region outlines and connectors drawn on the map's ground plane and
 * written back into an OMF image.
 *
 * Two honest limits shape this page, both set by the API rather than by the UI:
 * the edit script the service accepts carries a map information record, region
 * features with their outlines, connectors and raw metadata records — a hard
 * forbidden mask or a vector road is not expressible as a patch, so the studio
 * offers the six region tags the format defines and connectors, and nothing it
 * could not write back. And the base image comes from `/maps/{id}/image`, because
 * a library entry is otherwise only reachable as metadata and chunks.
 */
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import { useRoute, useRouter } from 'vue-router'
import {
  Alert as TAlert,
  Button as TButton,
  Card as TCard,
  InputNumber as TInputNumber,
  Input as TInput,
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
import { downloadMapImage } from '@/api/maps'
import { isApiError } from '@/api/errors'
import type { ChunkRef } from '@/api/maps'
import { getRegions } from '@/api/maps'
import type { ConnectorEdit } from '@/api/omf'
import type { Vec2 } from '@/api/types'
import { regionOutlines } from '@/types/sections'
import { LAYER_ELEVATION, chunkKey, selectLevel } from '@/types/map'
import { INK_LIGHT, SERIES_PALETTE, rgbToHex } from '@/types/colormap'
import { engineFromQuery, probeEngine } from '@/render/engine'
import { MapScene, updateDebug } from '@/render/scene'
import { TerrainLayer, buildChunkMesh } from '@/render/terrain'
import { pickGround } from '@/render/picking'
import { useMapsStore } from '@/stores/maps'
import { useNotificationsStore } from '@/stores/notifications'
import { polygonArea, regionsEdit, useOmfStore, type RegionDraft } from '@/stores/omf'
import { downloadBytes, formatMetric } from '@/stores/simulations'

/** Resolution the camera is treated as showing when the level is chosen, m/px. */
const TARGET_METRES_PER_PIXEL = 2

/** Region tags `map-format` defines, with the catalog label of each. */
const REGION_TAGS: ReadonlyArray<{ value: number; labelKey: string }> = [
  { value: 0, labelKey: 'omf.studio.kindHighRise' },
  { value: 1, labelKey: 'omf.studio.kindOverpass' },
  { value: 2, labelKey: 'omf.studio.kindCanyon' },
  { value: 3, labelKey: 'omf.studio.kindTunnel' },
  { value: 4, labelKey: 'omf.studio.kindIndoor' },
  { value: 5, labelKey: 'omf.studio.kindMagnetic' },
]

const { t, locale } = useI18n({ useScope: 'global' })
const route = useRoute()
const router = useRouter()
const maps = useMapsStore()
const notifications = useNotificationsStore()
const omf = useOmfStore()

const canvas = ref<HTMLCanvasElement | null>(null)
const mapId = ref<string | null>(null)
const tool = ref<'regions' | 'connectors'>('regions')
const tagId = ref(0)
const draftPoints = ref<Vec2[]>([])
const connectorA = ref<Vec2 | null>(null)
const regionDrafts = ref<RegionDraft[]>([])
const connectorDrafts = ref<ConnectorEdit[]>([])
const triggerMode = ref('probabilistic')
const pMultipath = ref<number | null>(null)
const pLoss = ref<number | null>(null)
const status = ref<'idle' | 'loading' | 'ready' | 'failed'>('idle')
const failure = ref<string | null>(null)
const baseName = ref<string | null>(null)
const exporting = ref(false)

let scene: MapScene | null = null
let terrain: TerrainLayer | null = null
let draftMesh: LinesMesh | null = null
let existingMesh: LinesMesh | null = null
let markerMeshes: Mesh[] = []

/** Whether an image is available to edit. */
const hasBase = computed(() => omf.sourceBytes !== null)

/** Identifier from the path: `/maps/:id/studio` names the map the studio edits. */
const routeId = computed(() => String(route.params.id ?? ''))

/**
 * Map the studio works on: the route's id when the library holds it.
 *
 * An absent or unknown id falls back to the first map, which is what the studio
 * offered before the route carried an id.
 */
function mapIdFromRoute(): string | null {
  const wanted = routeId.value
  if (wanted !== '' && maps.summaries.some((map) => map.id === wanted)) {
    return wanted
  }
  return maps.summaries[0]?.id ?? null
}

/** Rows of the drawn-region list. */
const regionRows = computed(() =>
  regionDrafts.value.map((draft, index) => ({
    key: `region:${index}`,
    index,
    tag: tagLabel(draft.tag_id),
    points: draft.points.length,
    area: `${formatMetric(polygonArea(draft.points), 'm²', locale.value, 1)}`,
  })),
)

/** Rows of the drawn-connector list. */
const connectorRows = computed(() =>
  connectorDrafts.value.map((connector, index) => ({
    key: `connector:${index}`,
    index,
    type: connector.type_id,
    a: `${connector.a[0].toFixed(1)}, ${connector.a[1].toFixed(1)}`,
    b: `${connector.b[0].toFixed(1)}, ${connector.b[1].toFixed(1)}`,
  })),
)

/** Region tag options of the picker. */
const tagOptions = computed(() =>
  REGION_TAGS.map((tag) => ({ value: tag.value, label: t(tag.labelKey) })),
)

/** Translated label of a region tag, falling back to its number. */
function tagLabel(value: number): string {
  const tag = REGION_TAGS.find((entry) => entry.value === value)
  return tag === undefined ? String(value) : t(tag.labelKey)
}

/** Loads the selected map's elevation surface. */
async function loadSurface(): Promise<void> {
  const id = mapId.value
  if (id === null || id === '') {
    return
  }
  status.value = 'loading'
  failure.value = null
  try {
    await startScene()
    const info = await maps.loadMetadata(id)
    scene?.frameBounds(info.summary.bounds)
    const elevation = info.layers.find((layer) => layer.layer_id === LAYER_ELEVATION)
    if (elevation !== undefined && terrain !== null) {
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
    }
    await loadExistingRegions()
    status.value = 'ready'
  } catch (error) {
    failure.value = isApiError(error) ? error.message : String(error)
    status.value = 'failed'
  }
}

/** Draws the outlines the service reports for the loaded map. */
async function loadExistingRegions(): Promise<void> {
  const id = mapId.value
  if (id === null || scene === null) {
    return
  }
  try {
    const section = await getRegions(id)
    const outlines = regionOutlines(section.json)
    existingMesh?.dispose()
    existingMesh = null
    if (outlines.length === 0) {
      return
    }
    const mesh = CreateLineSystem(
      'existingRegions',
      {
        lines: outlines.map((outline) =>
          outline.points.map((point) => new Vector3(point.x, 1.5, point.y)),
        ),
      },
      scene.scene,
    )
    const material = new StandardMaterial('existingRegionsMaterial', scene.scene)
    material.disableLighting = true
    material.emissiveColor = Color3.FromHexString(SECOND_COLOUR())
    mesh.material = material
    mesh.isPickable = false
    existingMesh = mesh
  } catch {
    // A map without a region section is the normal case, not a failure.
  }
}

/** Second palette colour, used for the outlines already in the map. */
function SECOND_COLOUR(): string {
  return SERIES_PALETTE[1] ?? SERIES_PALETTE[0] ?? rgbToHex(INK_LIGHT)
}

/** First palette colour, used for the draft and the markers. */
function FIRST_COLOUR(): string {
  return SERIES_PALETTE[0] ?? rgbToHex(INK_LIGHT)
}

/** Starts the engine and wires the surface picker. */
async function startScene(): Promise<void> {
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
  updateDebug({ engine: probe.backend, mapId: mapId.value, frames: 0, loaded: false, error: null })
  scene.scene.onPointerUp = () => {
    readPick()
  }
}

/** Adds the picked point to the current draft. */
function readPick(): void {
  const current = scene
  if (current === null) {
    return
  }
  const result = pickGround(current.scene, current.scene.pointerX, current.scene.pointerY, {
    targets: terrain?.list ?? [],
  })
  if (result.point === null) {
    return
  }
  const point: Vec2 = {
    x: Math.round(result.point.x * 100) / 100,
    y: Math.round(result.point.y * 100) / 100,
  }
  if (tool.value === 'regions') {
    draftPoints.value = [...draftPoints.value, point]
    drawDraft()
    return
  }
  if (connectorA.value === null) {
    connectorA.value = point
    drawDraft()
    return
  }
  connectorDrafts.value = [
    ...connectorDrafts.value,
    {
      type_id: 0,
      a: [connectorA.value.x, connectorA.value.y, 0],
      b: [point.x, point.y, 0],
      direction: 'both',
    },
  ]
  connectorA.value = null
  drawDraft()
}

/** Draws the work in progress: the draft outline or the connector's first endpoint. */
function drawDraft(): void {
  const current = scene
  if (current === null) {
    return
  }
  draftMesh?.dispose()
  draftMesh = null
  for (const mesh of markerMeshes) {
    mesh.dispose()
  }
  markerMeshes = []
  const material = new StandardMaterial('draftMaterial', current.scene)
  material.disableLighting = true
  material.emissiveColor = Color3.FromHexString(FIRST_COLOUR())
  const points =
    tool.value === 'regions'
      ? draftPoints.value
      : connectorA.value === null
        ? []
        : [connectorA.value]
  if (points.length > 1) {
    const mesh = CreateLineSystem(
      'draft',
      { lines: [points.map((point) => new Vector3(point.x, 2.5, point.y))] },
      current.scene,
    )
    mesh.material = material
    mesh.isPickable = false
    draftMesh = mesh
  }
  markerMeshes = points.map((point, index) => {
    const sphere = CreateSphere(`draftVertex:${index}`, { diameter: 2 }, current.scene)
    sphere.position = new Vector3(point.x, 2.5, point.y)
    sphere.material = material
    sphere.isPickable = false
    return sphere
  })
}

/** Closes the outline being drawn and adds it to the region list. */
function finishRegion(): void {
  if (draftPoints.value.length < 3) {
    return
  }
  regionDrafts.value = [
    ...regionDrafts.value,
    {
      points: draftPoints.value.map((point) => [point.x, point.y] as [number, number]),
      tag_id: tagId.value,
      trigger_mode: triggerMode.value,
      ...(pMultipath.value === null ? {} : { p_mp: pMultipath.value }),
      ...(pLoss.value === null ? {} : { p_loss: pLoss.value }),
    },
  ]
  draftPoints.value = []
  drawDraft()
}

/** Drops the outline being drawn. */
function cancelDraft(): void {
  draftPoints.value = []
  connectorA.value = null
  drawDraft()
}

/** Removes the last vertex of the outline being drawn. */
function undoVertex(): void {
  draftPoints.value = draftPoints.value.slice(0, -1)
  drawDraft()
}

/** Removes one drawn region. */
function removeRegion(index: number): void {
  regionDrafts.value = regionDrafts.value.filter((_, position) => position !== index)
}

/** Removes one drawn connector. */
function removeConnector(index: number): void {
  connectorDrafts.value = connectorDrafts.value.filter((_, position) => position !== index)
}

/**
 * Downloads the library map as the base image.
 */
async function useLibraryImage(): Promise<void> {
  const id = mapId.value
  if (id === null || id === '') {
    return
  }
  try {
    const { bytes, fileName } = await downloadMapImage(id)
    omf.setSource(bytes, fileName)
    baseName.value = fileName
    notifications.push({
      kind: 'success',
      message: t('omf.studio.baseReady', { size: `${bytes.byteLength} B` }),
    })
  } catch (error) {
    notifications.pushError(t('omf.studio.baseFailed'), error)
  }
}

/** Writes the drawn features back into the base image and downloads it. */
async function exportImage(): Promise<void> {
  if (!hasBase.value) {
    notifications.push({ kind: 'warning', message: t('omf.studio.needsBase') })
    return
  }
  if (regionDrafts.value.length === 0 && connectorDrafts.value.length === 0) {
    notifications.push({ kind: 'warning', message: t('omf.studio.emptyScript') })
    return
  }
  exporting.value = true
  try {
    const edits = {
      ...(regionDrafts.value.length === 0
        ? {}
        : { regions: regionsEdit(regionDrafts.value, true) }),
      ...(connectorDrafts.value.length === 0
        ? {}
        : { connectors: { connectors: connectorDrafts.value, merge: true } }),
    }
    const bytes = await omf.applyEdits(edits)
    if (bytes === null) {
      notifications.pushError(t('omf.studio.exportFailed'), new Error(omf.editError ?? ''))
      return
    }
    downloadBytes(bytes, omf.editedName ?? `map-edited.omf`)
    notifications.push({ kind: 'success', message: t('omf.studio.exported') })
  } finally {
    exporting.value = false
  }
}

/**
 * Loads the surface of the selected map; the only place a scene is built.
 *
 * The dropdown and the route both write `mapId`, so this watcher is the single
 * load path and a mount starts exactly one engine.
 */
watch(mapId, async () => {
  cancelDraft()
  regionDrafts.value = []
  connectorDrafts.value = []
  scene?.dispose()
  scene = null
  terrain = null
  existingMesh = null
  await loadSurface()
})

watch(routeId, () => {
  mapId.value = mapIdFromRoute()
})

watch(tool, () => {
  cancelDraft()
})

onMounted(async () => {
  await maps.loadMaps()
  mapId.value = mapIdFromRoute()
})

onBeforeUnmount(() => {
  draftMesh?.dispose()
  existingMesh?.dispose()
  for (const mesh of markerMeshes) {
    mesh.dispose()
  }
  markerMeshes = []
  terrain?.dispose()
  scene?.dispose()
  scene = null
  updateDebug({ mapId: null, loaded: false, error: null })
})
</script>

<template>
  <section class="mx-auto max-w-7xl px-8 py-8" data-testid="map-studio">
    <div class="flex items-center justify-between gap-4">
      <div class="flex items-center gap-3">
        <h1 class="font-semibold text-ink">{{ t('views.mapStudio.title') }}</h1>
        <TTag v-if="baseName !== null" size="small" variant="light" theme="success">
          {{ t('omf.studio.baseReady', { size: omf.sourceSize + ' B' }) }}
        </TTag>
      </div>
      <div class="flex items-center gap-2">
        <TButton
          variant="outline"
          :disabled="mapId === null"
          data-testid="studio-base"
          @click="useLibraryImage()"
        >
          {{ t('omf.studio.downloadBase') }}
        </TButton>
        <TButton
          theme="primary"
          :loading="exporting"
          :disabled="!hasBase"
          data-testid="studio-export"
          @click="exportImage()"
        >
          {{ exporting ? t('omf.studio.exporting') : t('omf.studio.export') }}
        </TButton>
        <TButton variant="outline" @click="router.push('/omf')">
          {{ t('views.omf.title') }}
        </TButton>
      </div>
    </div>

    <TAlert
      v-if="failure !== null"
      class="mt-4"
      theme="error"
      :message="t('omf.studio.surfaceFailed')"
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
            {{
              tool === 'regions'
                ? t('omf.studio.noVertices')
                : connectorA === null
                  ? t('omf.studio.pickFirst')
                  : t('omf.studio.pickSecond')
            }}
          </p>
          <p v-if="status === 'loading'" class="absolute left-3 top-3 text-sm text-muted">
            {{ t('omf.studio.loading') }}
          </p>
        </div>
      </div>

      <aside class="flex flex-col gap-4">
        <TCard :title="t('omf.studio.surface')" size="small">
          <TSelect
            :value="mapId ?? ''"
            :options="maps.summaries.map((map) => ({ value: map.id, label: map.name }))"
            :disabled="maps.summaries.length === 0"
            :placeholder="t('omf.studio.chooseMap')"
            data-testid="studio-map"
            @change="(value) => (mapId = String(value))"
          />
          <p class="mt-2 text-xs text-muted">{{ t('omf.studio.surfaceHint') }}</p>
          <p v-if="maps.summaries.length === 0" class="mt-2 text-sm text-muted">
            {{ t('omf.studio.noMaps') }}
          </p>
        </TCard>

        <TCard :title="t('omf.studio.tools')" size="small">
          <TSelect
            :value="tool"
            :options="[
              { value: 'regions', label: t('omf.studio.toolRegions') },
              { value: 'connectors', label: t('omf.studio.toolConnectors') },
            ]"
            data-testid="studio-tool"
            @change="(value) => (tool = value === 'connectors' ? 'connectors' : 'regions')"
          />
          <template v-if="tool === 'regions'">
            <label class="mt-2 block">
              <span class="text-xs text-muted">{{ t('omf.studio.kind') }}</span>
              <TSelect
                :value="tagId"
                :options="tagOptions"
                data-testid="studio-tag"
                @change="(value) => (tagId = Number(value))"
              />
            </label>
            <label class="mt-2 block">
              <span class="text-xs text-muted">{{ t('omf.edit.regionTrigger') }}</span>
              <TSelect
                :value="triggerMode"
                :options="[
                  { value: 'probabilistic', label: t('omf.edit.triggerProbabilistic') },
                  { value: 'spatial_deterministic', label: t('omf.edit.triggerDeterministic') },
                ]"
                @change="(value) => (triggerMode = String(value))"
              />
            </label>
            <div class="mt-2 flex gap-2">
              <label class="flex-1">
                <span class="text-xs text-muted">{{ t('omf.edit.regionMultipath') }}</span>
                <TInputNumber
                  :value="pMultipath ?? undefined"
                  :min="0"
                  :max="1"
                  :decimal-places="4"
                  @change="(value) => (pMultipath = Number(value))"
                />
              </label>
              <label class="flex-1">
                <span class="text-xs text-muted">{{ t('omf.edit.regionLoss') }}</span>
                <TInputNumber
                  :value="pLoss ?? undefined"
                  :min="0"
                  :max="1"
                  :decimal-places="4"
                  @change="(value) => (pLoss = Number(value))"
                />
              </label>
            </div>
            <div class="mt-2 flex items-center gap-2">
              <TButton
                size="small"
                theme="primary"
                :disabled="draftPoints.length < 3"
                data-testid="studio-finish-region"
                @click="finishRegion()"
              >
                {{ t('omf.studio.finish') }}
              </TButton>
              <TButton size="small" variant="outline" @click="undoVertex()">
                {{ t('omf.studio.undo') }}
              </TButton>
              <TButton size="small" variant="outline" @click="cancelDraft()">
                {{ t('omf.studio.cancel') }}
              </TButton>
            </div>
            <p class="mt-2 text-xs text-muted">
              {{ t('omf.studio.vertices') }}: {{ draftPoints.length }}
            </p>
          </template>
          <template v-else>
            <p class="mt-2 text-xs text-muted">
              {{ connectorA === null ? t('omf.studio.pickFirst') : t('omf.studio.pickSecond') }}
            </p>
            <TButton class="mt-2" size="small" variant="outline" @click="cancelDraft()">
              {{ t('omf.studio.cancel') }}
            </TButton>
          </template>
        </TCard>
      </aside>
    </div>

    <div class="mt-6 grid grid-cols-2 gap-6">
      <TCard :title="t('omf.studio.regionsDrawn', { count: regionDrafts.length })" size="small">
        <p v-if="regionRows.length === 0" class="text-sm text-muted">
          {{ t('omf.studio.drawnEmpty') }}
        </p>
        <TTable
          v-else
          :data="regionRows"
          :columns="[
            { colKey: 'tag', title: t('omf.studio.kind') },
            { colKey: 'points', title: t('omf.studio.vertices'), width: 100 },
            { colKey: 'area', title: t('omf.studio.area'), width: 120 },
            { colKey: 'remove', title: ' ', width: 100 },
          ]"
          row-key="key"
          size="small"
          data-testid="studio-regions"
        >
          <template #remove="{ row }">
            <TButton variant="text" size="small" @click="removeRegion(row.index)">
              {{ t('omf.studio.remove') }}
            </TButton>
          </template>
        </TTable>
      </TCard>

      <TCard
        :title="t('omf.studio.connectorsDrawn', { count: connectorDrafts.length })"
        size="small"
      >
        <p v-if="connectorRows.length === 0" class="text-sm text-muted">
          {{ t('omf.studio.drawnEmpty') }}
        </p>
        <TTable
          v-else
          :data="connectorRows"
          :columns="[
            { colKey: 'type', title: t('omf.edit.connectorType'), width: 100 },
            { colKey: 'a', title: t('omf.studio.pointA') },
            { colKey: 'b', title: t('omf.studio.pointB') },
            { colKey: 'remove', title: ' ', width: 100 },
          ]"
          row-key="key"
          size="small"
          data-testid="studio-connectors"
        >
          <template #remove="{ row }">
            <TButton variant="text" size="small" @click="removeConnector(row.index)">
              {{ t('omf.studio.remove') }}
            </TButton>
          </template>
        </TTable>
      </TCard>
    </div>

    <TCard class="mt-6" :title="t('omf.edit.title')" size="small">
      <p class="text-sm text-muted">{{ t('omf.edit.unsupported') }}</p>
      <p class="mt-1 text-sm text-muted">{{ t('omf.edit.stored') }}</p>
      <TInput
        class="mt-2"
        :value="baseName ?? ''"
        readonly
        :placeholder="t('omf.studio.sourceImage')"
      />
    </TCard>
  </section>
</template>
