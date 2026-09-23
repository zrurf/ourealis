<script setup lang="ts">
/*
 * The run workspace: one page for choosing a map, drawing a route, setting the runner
 * and the sensors, and starting the run.
 *
 * The layout is the point. A tab row says where in the process the reader is and what is
 * still missing; the map is the centre, because a route is a shape on a map rather than a
 * pair of coordinates; and one panel on the right shows the controls of the open stage.
 * Nothing navigates away: the route drawn here is the route that is submitted, which is
 * the failure the two-page studio had.
 *
 * The canvas is the same `MapScene` the viewer uses, with the route handles added on
 * top. Clicking the ground places the next point of the route — start, then goal —
 * and dragging a handle moves it; the planner runs on its own as the route changes.
 * Everything drawn *about* the route — the draft line, the candidates, the path the
 * service planned — is draped on the terrain, because a line at a constant height is
 * under the ground on a hill and hanging in the air over a valley.
 */
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import { useRoute, useRouter } from 'vue-router'
import { Button as TButton, Tag as TTag } from 'tdesign-vue-next'
import { isApiError } from '@/api/errors'
import type { ChunkRef } from '@/api/maps'
import { channelRange } from '@/api/types'
import { LAYER_ELEVATION, chunkKey, selectLevel } from '@/types/map'
import { SERIES_PALETTE } from '@/types/colormap'
import { engineFromQuery, probeEngine } from '@/render/engine'
import { updateDebug } from '@/render/scene'
import { renderHost, type SceneLease } from '@/render/host'
import type { MapScene } from '@/render/scene'
import { TerrainLayer } from '@/render/terrain'
import { OverlaySet, type OverlayKind } from '@/render/overlays'
import { DirectionArrows } from '@/render/directionArrows'
import {
  createOverlaySync,
  overlayAvailability as familiesAvailable,
  syncOverlays,
} from '@/render/overlaySync'
import { buildChunkMesh, chunkGrid } from '@/render/terrainMesh'
import { createCellSampler } from '@/render/cellSampler'
import { RouteHandles, type HandleRole, type HandleSpec } from '@/render/handles'
import { pickGround } from '@/render/picking'
import { createSurfaceSampler, elevationAt } from '@/render/inspect'
import { drapePath, terracedSampler } from '@/render/drape'
import { attachGroundPan, metresPerPixelAt } from '@/render/pan'
import { PathSet } from '@/render/lines'
import { surfaceKey as buildSurfaceKey, surfaceStyle } from '@/render/surfaceStyle'
import ScaleBar from '@/components/map/ScaleBar.vue'
import ViewportControls from '@/components/map/ViewportControls.vue'
import StageTabs from '@/components/run/StageTabs.vue'
import StagePanel from '@/components/run/StagePanel.vue'
import PlanPanel from '@/components/run/PlanPanel.vue'
import MapMenu from '@/components/run/MapMenu.vue'
import {
  buildSimulationRequest,
  pointToVec,
  type PickTarget,
  type SemanticsKind,
} from '@/components/forms/request'
import { useMapsStore } from '@/stores/maps'
import { useNotificationsStore } from '@/stores/notifications'
import { useSimulationsStore } from '@/stores/simulations'
import { useThemeStore } from '@/stores/theme'
import { useViewerStore } from '@/stores/viewer'
import { useWorkspaceStore } from '@/stores/workspace'

/** Resolution the camera is treated as showing when the level is chosen, m/px. */
const TARGET_METRES_PER_PIXEL = 2

/** Height a drawn route is lifted above the ground, metres. */
const ROUTE_LIFT_M = 0.4

/** Milliseconds a pointer move may be coalesced while dragging a handle. */
const DRAG_THROTTLE_MS = 40

/**
 * Milliseconds between two checks of the dragged point.
 *
 * Slower than the move: the reader needs to see the verdict settle, not follow every
 * pixel, and the answer only changes when the point crosses a cell boundary.
 */
const DRAG_CHECK_THROTTLE_MS = 150

const { t } = useI18n({ useScope: 'global' })
const currentRoute = useRoute()
const router = useRouter()
const maps = useMapsStore()
const workspace = useWorkspaceStore()
const simulations = useSimulationsStore()
const notifications = useNotificationsStore()
const viewer = useViewerStore()
const theme = useThemeStore()

/** Container the shared canvas is moved into. */
const host = ref<HTMLDivElement | null>(null)
const status = ref<'idle' | 'loading' | 'ready' | 'failed'>('idle')
const failure = ref<string | null>(null)
const submitting = ref(false)
const scaleBarMetres = ref(100)
const groundResolution = ref(1)
/** Extent of the map on screen, so a click on the background places nothing. */
const bounds = ref<{ min_x: number; min_y: number; max_x: number; max_y: number } | null>(null)
/** Elevation range of the open map, for the viewport bar's automatic terrace step. */
const surfaceRange = ref<{ min: number; max: number } | null>(null)

let scene: MapScene | null = null
let lease: SceneLease | null = null
let terrain: TerrainLayer | null = null
let overlays: OverlaySet | null = null
let arrows: DirectionArrows | null = null
let handles: RouteHandles | null = null
let paths: PathSet | null = null
let disposed = false
let detachPan: (() => void) | null = null
let dragging: HandleSpec | null = null
/** Handle the keyboard edits, as `role:index`; independent of the armed field. */
const selected = ref<string | null>(null)
let lastDragMs = 0
let lastDragCheckMs = 0
let pointerFrom: { x: number; y: number; button: number } | null = null
/** Chunk keys of the loaded surface, so a rebuild does not re-read the service. */
const terrainChunks = new Set<string>()
/** Overlay families the workspace has already read for this map. */
const overlayLoaded = createOverlaySync()

/** Which families the open map carries, for the rail's switches. */
const overlayAvailability = computed(() =>
  familiesAvailable(mapId.value === null ? null : maps.metadataOf(mapId.value)),
)

/** The map the workspace plans against. */
const mapId = computed(() => workspace.mapId)

/** The route's points, as the handle layer draws them. */
const handleSpecs = computed<HandleSpec[]>(() => {
  const draft = workspace.draft
  const specs: HandleSpec[] = []
  const id = mapId.value
  const grid = id === null ? null : maps.gridOf(id, LAYER_ELEVATION)
  const cells = id === null ? 0 : (maps.metadataOf(id)?.summary.chunk_size ?? 0)
  const level = viewerLevel()
  const origin = id === null ? { x: 0, y: 0 } : maps.originOf(id)
  const push = (
    role: HandleSpec['role'],
    index: number,
    point: { x: number; y: number } | null,
  ): void => {
    if (point === null) {
      return
    }
    const verdict = workspace.pointChecks[`${role}:${index}`]
    // On the ground the marker belongs to: a symbol drawn at a fixed height ends up
    // under the terrain, and where it then appears on screen is nowhere near the point
    // it stands for — which is what made the markers impossible to grab.
    specs.push({
      role,
      index,
      x: point.x,
      y: point.y,
      z: grid === null ? null : elevationAt(maps.chunks, grid, cells, level, point, origin),
      illegal: verdict?.legal === false,
    })
  }
  push('start', 0, pointToVec(draft.start))
  if (draft.mode !== 'loop') {
    push('goal', 0, pointToVec(draft.goal))
  } else {
    push('reference', 0, pointToVec(draft.reference))
  }
  draft.waypoints.forEach((waypoint, index) => {
    push('waypoint', index, pointToVec(waypoint.position))
  })
  return specs
})

/** Level the surface is drawn at, which is the level the handles are placed on. */
function viewerLevel(): number {
  const id = mapId.value
  const grid = id === null ? null : maps.gridOf(id, LAYER_ELEVATION)
  return grid === null ? 0 : selectLevel(grid, TARGET_METRES_PER_PIXEL)
}

/** The route as a polyline: the handles in order, which is what the planner will take. */
const routePoints = computed<Array<{ x: number; y: number }>>(() => {
  const draft = workspace.draft
  const points: Array<{ x: number; y: number }> = []
  const start = pointToVec(draft.start)
  if (start !== null) {
    points.push(start)
  }
  for (const waypoint of draft.waypoints) {
    const point = pointToVec(waypoint.position)
    if (point !== null) {
      points.push(point)
    }
  }
  if (draft.mode !== 'loop') {
    const goal = pointToVec(draft.goal)
    if (goal !== null) {
      points.push(goal)
    }
  }
  return points
})

/** Colours of the route pipe: the wall, the bore, the water and the crest moving through it. */
const PIPE_COLOURS = {
  wall: '#22303c',
  bore: '#3d6c99',
  water: '#79c4ee',
  crest: '#eef8ff',
} as const

/** Palette colour of the candidate at an index, so a row matches its line. */
function candidateColour(index: number): string {
  return SERIES_PALETTE[(index + 1) % SERIES_PALETTE.length] ?? '#3f7d4e'
}

/**
 * Draws the route: the planned candidates, the smoothed path, and the draft line.
 *
 * All of them are draped on the terrain. A line at a constant height was the reason a
 * route preview was invisible: the surface is 12–23 m up on a campus map, so a line at
 * 1.2 m spent its life inside the hill. The planner's own candidates are drawn because the
 * panel chooses among them — a table of numbers beside a map that shows none of them
 * cannot be compared at a glance.
 */
function drawRoute(): void {
  const painter = paths
  const draft = routePoints.value
  const preview = workspace.plan.preview
  if (painter === null) {
    return
  }
  const surface = surfaceOf()
  const styles = []
  // Everything else first, the chosen one last: the highlight is a band *above* its neighbours,
  // and drawing order is how that is expressed.
  for (const [index, candidate] of (preview?.candidates ?? []).entries()) {
    const inspected = index === workspace.plan.inspected
    if (!inspected && index !== workspace.plan.chosen) {
      continue
    }
    if (inspected) {
      continue
    }
    styles.push({
      points: drapePath(candidate.points, surface, ROUTE_LIFT_M),
      colour: candidateColour(index),
      widthM: 2,
      alpha: 0.5,
      zOffset: -4,
    })
  }
  const smoothed = preview?.path ?? []
  if (smoothed.length > 1 && workspace.plan.inspected !== workspace.plan.chosen) {
    styles.push({
      points: drapePath(smoothed, surface, ROUTE_LIFT_M * 1.2),
      colour: SERIES_PALETTE[0] ?? '#3f7d4e',
      widthM: 1.6,
      alpha: 0.85,
      zOffset: -5,
    })
  }
  // The selected candidate is a *pipe with water in it*: an outer wall, a bore and the water
  // inside, with a train of slugs sliding through. Three nested bands of decreasing width say
  // "pipe" without a shader, and the slugs say which way the water runs — which is the one thing a
  // static line cannot say about a route.
  const chosen = preview?.candidates[workspace.plan.inspected]
  if (chosen !== undefined) {
    const points = drapePath(chosen.points, surface, ROUTE_LIFT_M * 1.4)
    styles.push(
      { points, colour: PIPE_COLOURS.wall, widthM: 6.4, alpha: 0.95, zOffset: -6 },
      { points, colour: PIPE_COLOURS.bore, widthM: 4.6, alpha: 0.92, zOffset: -7 },
      { points, colour: PIPE_COLOURS.water, widthM: 3.4, alpha: 0.85, zOffset: -8 },
      {
        // The crest inside the water: four slugs of about 24 m sliding through at 10 m/s. They are
        // narrower than the water they run in, which is what makes the pipe read as a pipe rather
        // than as a striped band.
        points,
        colour: PIPE_COLOURS.crest,
        widthM: 2.1,
        alpha: 0.95,
        zOffset: -9,
        flow: { slugs: 4, lengthM: 24, widthM: 2.1, speed: 10, colour: PIPE_COLOURS.crest },
      },
    )
  }
  if (draft.length > 1) {
    styles.push({
      points: drapePath(draft, surface, ROUTE_LIFT_M),
      colour: SERIES_PALETTE[0] ?? '#3f7d4e',
      widthM: 0.6,
      alpha: 0.7,
      zOffset: -3,
    })
  }
  painter.set(styles)
}

/**
 * Ground elevation lookup for the workspace.
 *
 * Rebuilt per call rather than cached: a draw happens after a route edit, and the sampler
 * is cheap to build and answers thousands of points once built.
 */
function surfaceOf(): (x: number, y: number) => number | null {
  const id = mapId.value
  const grid = id === null ? null : maps.gridOf(id, LAYER_ELEVATION)
  const cells = chunkSize()
  if (grid === null || id === null) {
    return () => null
  }
  return terracedSampler(
    createSurfaceSampler(maps.chunks, grid, cells, viewerLevel(), maps.originOf(id)),
    surfaceStyle(styleInput()).terraceM ?? 0,
  )
}

/** Frames the map and loads the surface for a map. */
async function loadSurface(): Promise<void> {
  const id = mapId.value
  const current = scene
  if (id === null || current === null || terrain === null) {
    return
  }
  status.value = 'loading'
  try {
    const info = await maps.loadMetadata(id)
    await maps.loadGrid(id, LAYER_ELEVATION)
    const grid = maps.gridOf(id, LAYER_ELEVATION)
    if (grid === null) {
      return
    }
    const level = selectLevel(grid, TARGET_METRES_PER_PIXEL)
    const refs: ChunkRef[] = (grid.chunks[level] ?? []).map((chunkId) => ({
      mapId: id,
      layerId: LAYER_ELEVATION,
      level,
      chunkId,
    }))
    await maps.loadChunks(refs, { concurrency: 4 })
    terrainChunks.clear()
    for (const target of refs) {
      terrainChunks.add(chunkKey(target.layerId, target.level, target.chunkId))
    }
    surfaceRange.value = channelRange(info.global_stats, LAYER_ELEVATION)
    rebuildSurface()
    await applyOverlays()
    bounds.value = info.summary.bounds
    if (maps.gridOf(id, LAYER_ELEVATION) !== null) {
      current.frameBounds(info.summary.bounds)
      scaleBarMetres.value = current.scaleBarLength(TARGET_METRES_PER_PIXEL)
    }
    status.value = 'ready'
  } catch (error) {
    failure.value = isApiError(error) ? error.message : String(error)
    status.value = 'failed'
  }
}

/** Rebuilds every loaded chunk from the cached samples, after a viewport change. */
function rebuildSurface(): void {
  const id = mapId.value
  const grid = id === null ? null : maps.gridOf(id, LAYER_ELEVATION)
  const current = terrain
  if (id === null || grid === null || current === null) {
    return
  }
  const cells = chunkSize()
  const level = selectLevel(grid, TARGET_METRES_PER_PIXEL)
  const origin = maps.originOf(id)
  const sample = createCellSampler({
    chunks: maps.chunks,
    grid,
    chunkSize: cells,
    level,
    layerId: LAYER_ELEVATION,
  })
  const style = surfaceStyle(styleInput())
  current.clear()
  for (const key of terrainChunks) {
    if (maps.chunkOf(key) === null) {
      continue
    }
    const chunkId = Number(key.split('/')[2])
    current.setChunk(
      key,
      buildChunkMesh(chunkGrid(grid, cells, level, chunkId, origin), sample, style),
    )
  }
  drawRoute()
}

/** The viewport values the surface is derived from. */
function styleInput() {
  return {
    range: surfaceRange.value,
    dark: theme.isDark,
    terraceM: viewer.terraceStepM,
    sunAzimuthDeg: viewer.sunAzimuthDeg,
  }
}

/**
 * Brings the overlays in line with the rail's switches.
 *
 * The same controller the preview uses, on the same map: a family is read the first time it is
 * switched on and its geometry is draped on this view's own surface, so what the workspace draws
 * and what the preview draws cannot disagree.
 */
async function applyOverlays(): Promise<void> {
  await syncOverlays(overlayLoaded, {
    manager: overlays as OverlaySet,
    arrows,
    mapId: mapId.value ?? '',
    metadata: () => (mapId.value === null ? null : maps.metadataOf(mapId.value)),
    toggles: () => viewer.overlays,
    level: () => viewerLevel(),
    surface: () => surfaceOf(),
    chunks: () => maps.chunks,
    gridOf: (layerId) => (mapId.value === null ? null : maps.gridOf(mapId.value, layerId)),
    ensureGrid: async (layerId) => {
      if (mapId.value !== null && maps.gridOf(mapId.value, layerId) === null) {
        await maps.loadGrid(mapId.value, layerId)
      }
    },
    refsInArea: (layerId, level, area) =>
      mapId.value === null
        ? []
        : maps.chunkRefsInArea(mapId.value, layerId, chunkSize(), level, area),
    loadChunks: async (refs) => maps.loadChunks(refs, { concurrency: 4 }),
    footprint: () => bounds.value ?? { min_x: 0, min_y: 0, max_x: 0, max_y: 0 },
    origin: () => (mapId.value === null ? { x: 0, y: 0 } : maps.originOf(mapId.value)),
    onError: (_kind: OverlayKind, error) =>
      notifications.pushError(t('map.viewer.metadataFailed'), error),
  })
}

/** Cells per chunk side of the open map. */
function chunkSize(): number {
  return mapId.value === null ? 0 : (maps.metadataOf(mapId.value)?.summary.chunk_size ?? 0)
}

/** Applies the camera half of the viewport parameters to the live camera. */
function applyViewport(): void {
  const current = scene
  if (current === null) {
    return
  }
  applying = true
  current.setZoomStep(viewer.zoomStep)
  current.setPitch(viewer.pitchDeg)
  current.setFov(viewer.fovDeg)
  current.setOrthographic(viewer.orthographic)
  current.setZoomToPointer(viewer.zoomToPointer)
  current.setSun(viewer.sunAzimuthDeg, viewer.sunElevationDeg)
  // The write is what the observer below must not echo; the flag is cleared on the next frame.
  void nextTick().then(() => {
    applying = false
    return undefined
  })
}

/**
 * Reports the camera back into the store.
 *
 * The pointer moves the camera as much as the panel does, so without this the panel's sliders
 * describe a camera the reader left behind — which is what made the old bar feel disconnected
 * from the view. The write is guarded so that the panel's own change does not echo back.
 */
function observeCamera(): void {
  const current = scene
  if (current === null) {
    return
  }
  current.observeCamera((state) => {
    if (applying) {
      return
    }
    viewer.pitchDeg = state.pitchDeg
    // The field of view and the zoom step are set by the reader, not by the pointer; only the
    // angles and the radius are the camera's to report.
    groundResolution.value = metresPerPixel()
    scaleBarMetres.value = current.scaleBarLength(groundResolution.value)
  })
}

/** True while the panel's own values are being pushed onto the camera. */
let applying = false

/** Starts the engine and wires the pointer. */
async function startScene(): Promise<void> {
  const container = host.value
  if (container === null || scene !== null) {
    return
  }
  const probe = await probeEngine({ forced: engineFromQuery() })
  if (probe.backend === null) {
    failure.value = t('map.viewer.engineFailed')
    status.value = 'failed'
    return
  }
  const borrowing = await renderHost().acquire(container, probe.backend)
  if (disposed) {
    borrowing.release()
    return
  }
  lease = borrowing
  scene = borrowing.scene
  terrain = new TerrainLayer(scene.scene, scene)
  overlays = new OverlaySet(scene)
  arrows = new DirectionArrows(scene)
  handles = new RouteHandles(scene.scene, scene)
  paths = new PathSet(scene)
  updateDebug({ engine: probe.backend, mapId: mapId.value, frames: 0, loaded: false, error: null })

  // Which gesture a pointer sequence is: dragging the handle it started on, or a click
  // on the ground that places the next point.
  scene.scene.onPointerDown = (event) => {
    const current = scene
    if (current === null) {
      return
    }
    // Only the primary button edits the route. A right-click opens the menu, and
    // treating it as a placement made the menu report a point it had just created —
    // the reader right-clicked for the menu and got a route point as well.
    pointerFrom = {
      x: current.scene.pointerX,
      y: current.scene.pointerY,
      button: event.button ?? 0,
    }
    if (pointerFrom.button !== 0) {
      return
    }
    const hit = handles?.pickAt(current.scene.pointerX, current.scene.pointerY) ?? null
    if (hit !== null) {
      dragging = hit
      workspace.setDragging(true)
      selected.value = `${hit.role}:${hit.index}`
    }
  }
  scene.scene.onPointerMove = () => {
    if (dragging === null) {
      return
    }
    const now = Date.now()
    if (now - lastDragMs < DRAG_THROTTLE_MS) {
      return
    }
    lastDragMs = now
    moveDragged()
  }
  scene.scene.onPointerUp = () => {
    const current = scene
    const from = pointerFrom
    pointerFrom = null
    if (dragging !== null) {
      dragging = null
      // The route-wide check resumes with the plan, which the release schedules.
      workspace.setDragging(false)
      workspace.schedulePlan()
      return
    }
    if (current === null || from === null) {
      return
    }
    // Nothing is placed on a map that is still loading: the extent a click is checked
    // against is not known yet, so a point could land outside the map and the planner
    // would refuse the route it belongs to. And nothing is placed by a button that was
    // not the primary one: a right-click is how the menu is asked for.
    if (status.value !== 'ready' || from.button !== 0) {
      return
    }
    const travelled = Math.hypot(current.scene.pointerX - from.x, current.scene.pointerY - from.y)
    if (travelled > 4) {
      return
    }
    const pick = pickGround(current.scene, current.scene.pointerX, current.scene.pointerY, {
      targets: terrain?.list ?? [],
    })

    if (pick.point === null || !insideMap(pick.point.x, pick.point.y)) {
      return
    }
    if (workspace.stage === 'route') {
      workspace.handleGroundClick(pick.point.x, pick.point.y)
    }
  }
  scene.camera.onViewMatrixChangedObservable.add(() => {
    groundResolution.value = metresPerPixel()
    scaleBarMetres.value = scene?.scaleBarLength(groundResolution.value) ?? scaleBarMetres.value
  })
  // On the window rather than on the canvas: nudging a point should work while the
  // pointer is over the panel, and the handler defers to a focused text field.
  window.addEventListener('keydown', onKeyDown)
  window.addEventListener('pointerdown', onMenuDismiss)
  // The middle button pans; the right one belongs to the map menu, so it is left alone here.
  detachPan = attachGroundPan(scene.scene, scene.camera, {
    buttons: [1],
    metresPerPixel,
    onPanned: () => drawRoute(),
  })
  applyViewport()
  observeCamera()
  await loadSurface()
  await nextTick()
  applying = false
}

/** Moves the dragged handle to the ground under the pointer. */
function moveDragged(): void {
  const current = scene
  const target = dragging
  if (current === null || target === null) {
    return
  }
  const pick = pickGround(current.scene, current.scene.pointerX, current.scene.pointerY, {
    targets: terrain?.list ?? [],
  })
  if (pick.point === null || !insideMap(pick.point.x, pick.point.y)) {
    return
  }
  workspace.setPoint({ kind: target.role, index: target.index }, pick.point.x, pick.point.y)
  const now = Date.now()
  if (now - lastDragCheckMs >= DRAG_CHECK_THROTTLE_MS) {
    lastDragCheckMs = now
    void workspace.checkPoint(`${target.role}:${target.index}`, pick.point.x, pick.point.y)
  }
}

/**
 * Whether a point lies inside the map's extent.
 *
 * The picker falls back to the ground plane, which reaches well past the mapped area, so
 * a click on the background would otherwise place a route point in the void — a route
 * the planner refuses, reported as a failure the reader cannot connect to their click.
 */
function insideMap(x: number, y: number): boolean {
  const extent = bounds.value
  if (extent === null) {
    return true
  }
  return x >= extent.min_x && x <= extent.max_x && y >= extent.min_y && y <= extent.max_y
}

/**
 * Which handle is selected, as the map draws it.
 *
 * Kept in step with the render layer so the emphasis on screen and the field the arrow
 * keys move are the same thing.
 */
watch(selected, (key) => {
  handles?.setSelected(key)
})

/** Whether a keystroke is the interface's to handle, or a text field's. */
function withinTextField(target: EventTarget | null): boolean {
  const element = target as HTMLElement | null
  if (element === null) {
    return false
  }
  const tag = element.tagName
  return (
    tag === 'INPUT' || tag === 'TEXTAREA' || tag === 'SELECT' || element.isContentEditable === true
  )
}

/** The handle a key names, or `null` when nothing is selected. */
function selectedSpec(): HandleSpec | null {
  const key = selected.value
  if (key === null) {
    return null
  }
  return handleSpecs.value.find((spec) => `${spec.role}:${spec.index}` === key) ?? null
}

/** Moves the selection by `step` places through the handles. */
function cycleSelection(step: number): void {
  const specs = handleSpecs.value
  if (specs.length === 0) {
    return
  }
  const current = selected.value
  const index = specs.findIndex((spec) => `${spec.role}:${spec.index}` === current)
  const next = index === -1 ? 0 : (index + step + specs.length) % specs.length
  const spec = specs[next]
  if (spec !== undefined) {
    selected.value = `${spec.role}:${spec.index}`
  }
}

/**
 * Slides the selected handle, clamped to the map.
 *
 * Clamped rather than refused: an arrow key that does nothing looks broken, while a
 * point that stops at the map's edge is the obvious meaning of "move east" there.
 */
function nudge(dx: number, dy: number): void {
  const spec = selectedSpec()
  const extent = bounds.value
  if (spec === null || extent === null) {
    return
  }
  const x = Math.min(extent.max_x, Math.max(extent.min_x, spec.x + dx))
  const y = Math.min(extent.max_y, Math.max(extent.min_y, spec.y + dy))
  workspace.setPoint({ kind: spec.role, index: spec.index }, x, y)
}

/** What the keyboard does while the workspace is open. */
function onKeyDown(event: KeyboardEvent): void {
  if (withinTextField(event.target)) {
    return
  }
  switch (event.key) {
    case 'Tab':
      // The rail and the panel need Tab less than the map does: a route is edited by
      // cycling its points, and `Escape` hands the keyboard back.
      event.preventDefault()
      cycleSelection(event.shiftKey ? -1 : 1)
      return
    case 'Escape':
      selected.value = null
      menu.value = null
      workspace.arm(null)
      return
    case 'Delete':
    case 'Backspace': {
      const spec = selectedSpec()
      if (spec === null) {
        return
      }
      event.preventDefault()
      workspace.removePoint({ kind: spec.role, index: spec.index })
      selected.value = null
      return
    }
    default:
      break
  }
  const step = event.shiftKey ? 10 : 1
  const moves: Record<string, [number, number]> = {
    ArrowLeft: [-step, 0],
    ArrowRight: [step, 0],
    ArrowUp: [0, step],
    ArrowDown: [0, -step],
  }
  const move = moves[event.key]
  if (move === undefined) {
    return
  }
  event.preventDefault()
  nudge(move[0], move[1])
}

/**
 * The context menu, open at the pointer.
 *
 * `handle` is what was under the pointer when it opened, which decides what the menu
 * offers; a menu opened on the ground places points instead.
 */
const menu = ref<{ x: number; y: number; handle: HandleSpec | null } | null>(null)

/** Opens the menu where the pointer is, over whatever it found. */
function openMenu(event: MouseEvent): void {
  const current = scene
  const element = host.value
  if (current === null || element === null) {
    return
  }
  const box = element.getBoundingClientRect()
  const x = event.clientX - box.left
  const y = event.clientY - box.top
  menu.value = {
    x: event.clientX,
    y: event.clientY,
    handle: handles?.pickAt(x, y) ?? null,
  }
}

/** The ground position under the pointer, or `null` when it is off the map. */
function groundUnderPointer(): { x: number; y: number } | null {
  const current = scene
  if (current === null) {
    return null
  }
  const pick = pickGround(current.scene, current.scene.pointerX, current.scene.pointerY, {
    targets: terrain?.list ?? [],
  })
  if (pick.point === null || !insideMap(pick.point.x, pick.point.y)) {
    return null
  }
  return { x: pick.point.x, y: pick.point.y }
}

/** Applies a menu action that places a point. */
function menuPlace(target: { kind: HandleRole; index: number }): void {
  const point = groundUnderPointer()
  menu.value = null
  if (point === null) {
    return
  }
  if (target.kind === 'waypoint' && target.index < 0) {
    selected.value = `waypoint:${workspace.addWaypoint(point.x, point.y)}`
    return
  }
  workspace.setPoint({ kind: target.kind, index: target.index } as PickTarget, point.x, point.y)
  selected.value = `${target.kind}:${target.index}`
}

/** Applies a menu action that retags a waypoint. */
function menuSemantics(index: number, semantics: SemanticsKind): void {
  workspace.patch({
    waypoints: workspace.draft.waypoints.map((waypoint, position) =>
      position === index ? { ...waypoint, semantics } : waypoint,
    ),
  })
  menu.value = null
}

/** Closes the menu when the reader clicks or presses Escape anywhere. */
function onMenuDismiss(event: Event): void {
  if (menu.value === null) {
    return
  }
  const target = event.target as HTMLElement | null
  if (target?.closest('[data-testid="map-menu"]') !== null && event.type === 'pointerdown') {
    return
  }
  menu.value = null
}

/** A description of the selection, announced to a screen reader. */
const selectionText = computed(() => {
  const spec = selectedSpec()
  if (spec === null) {
    return ''
  }
  return t('run.handle.selected', {
    role: t(`run.handle.role.${spec.role}`),
    x: spec.x.toFixed(1),
    y: spec.y.toFixed(1),
  })
})

/** Ground resolution the camera shows, metres per CSS pixel. */
function metresPerPixel(): number {
  const current = scene
  const element = host.value
  if (current === null || element === null || element.clientHeight === 0) {
    return 1
  }
  return metresPerPixelAt(current.camera.radius, element.clientHeight, current.camera.fov)
}

/** Submits the run and follows it. */
async function submit(): Promise<void> {
  if (submitting.value || !workspace.runnable) {
    return
  }
  submitting.value = true
  try {
    const id = await simulations.submit(buildSimulationRequest(workspace.draft))
    await router.push({ name: 'simulation', params: { id } })
    notifications.push({ kind: 'success', message: t('simulation.form.submitted') })
  } catch (error) {
    notifications.pushError(t('simulation.form.submitFailed'), error)
  } finally {
    submitting.value = false
  }
}

/** Frames the map again on demand. */
function recentre(): void {
  const id = mapId.value
  if (id === null || scene === null) {
    return
  }
  const info = maps.metadataOf(id)
  if (info !== null) {
    scene.frameBounds(info.summary.bounds)
  }
}

onMounted(async () => {
  await maps.loadMaps()
  // The route editor is the entry stage unless the page was opened for another one.
  workspace.stage = currentRoute.query.stage === 'map' ? 'map' : 'route'
  await startScene()
})

watch(
  () => mapId.value,
  async () => {
    await loadSurface()
  },
)

// Redraw as the draft changes: the handles follow the numbers a form field edits, and
// the route line follows the handles.
watch(
  () => [
    handleSpecs.value.map(
      (spec) =>
        `${spec.role}:${spec.index}:${spec.x}:${spec.y}:${spec.z ?? 'x'}:${spec.illegal === true}`,
    ),
  ],
  () => {
    handles?.update(handleSpecs.value)
    drawRoute()
  },
  { immediate: true },
)

watch(
  () => workspace.plan.preview,
  () => {
    drawRoute()
  },
)

/** Brings the overlays in line when a switch flips. */
watch(
  () => ({ ...viewer.overlays }),
  () => {
    void applyOverlays()
  },
  { deep: true },
)

/** Redraws the surface when a viewport parameter that is baked into it changes. */
watch(
  () => buildSurfaceKey(styleInput()),
  () => {
    scene?.setSun(viewer.sunAzimuthDeg)
    rebuildSurface()
  },
)

/** Applies the camera half of the viewport parameters. */
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

onBeforeUnmount(() => {
  disposed = true
  window.removeEventListener('keydown', onKeyDown)
  window.removeEventListener('pointerdown', onMenuDismiss)
  detachPan?.()
  detachPan = null
  paths?.dispose()
  handles?.dispose()
  overlays?.dispose()
  arrows?.dispose()
  terrain?.dispose()
  lease?.release()
  lease = null
  scene = null
  terrainChunks.clear()
  maps.clearChunks()
  updateDebug({ mapId: null, loaded: false, error: null })
})
</script>

<template>
  <section class="flex h-[calc(100vh-3.5rem)] min-h-0" data-testid="run-workspace">
    <div class="flex min-w-0 flex-1 flex-col">
      <div class="relative flex min-h-0 flex-1 flex-col">
        <div
          ref="host"
          class="block h-full w-full focus-visible:outline focus-visible:outline-2 focus-visible:outline-brand"
          style="touch-action: none"
          tabindex="0"
          :aria-label="t('run.canvasLabel')"
          data-testid="map-canvas"
          @contextmenu.prevent="openMenu($event)"
        />
        <MapMenu
          v-if="menu !== null"
          :at="{ x: menu.x, y: menu.y }"
          :handle="
            menu.handle === null ? null : { role: menu.handle.role, index: menu.handle.index }
          "
          :has-start="pointToVec(workspace.draft.start) !== null"
          :has-goal="pointToVec(workspace.draft.goal) !== null"
          @close="menu = null"
          @place="menuPlace"
          @semantics="menuSemantics"
        />
        <p class="sr-only" aria-live="polite" data-testid="handle-selection">
          {{ selectionText }}
        </p>
        <p v-if="status === 'loading'" class="absolute left-4 top-4 text-sm text-muted">
          {{ t('common.loading') }}
        </p>
        <span v-else-if="status === 'ready'" class="sr-only" data-testid="run-ready">
          {{ t('run.ready') }}
        </span>
        <p v-if="failure !== null" class="absolute left-4 top-4 text-sm text-danger">
          {{ failure }}
        </p>

        <!-- The same viewport rail the preview carries, because it is the same engine and the
             same surface: floating over the corner of the canvas, out of the map's way. -->
        <div class="pointer-events-none absolute left-3 top-12">
          <ViewportControls
            :range="surfaceRange"
            :overlays="overlayAvailability"
            @recentre="recentre()"
          />
        </div>

        <div class="pointer-events-none absolute bottom-4 left-4">
          <ScaleBar :metres="scaleBarMetres" :metres-per-pixel="groundResolution" />
        </div>
      </div>
    </div>

    <aside
      class="@container flex w-[26rem] shrink-0 flex-col gap-4 overflow-y-auto border-l border-line bg-surface px-4 py-4"
    >
      <header class="flex items-center justify-between gap-2">
        <h1 class="font-semibold text-ink">{{ t('views.run.title') }}</h1>
        <div class="flex items-center gap-2">
          <TTag size="small" variant="light">{{ t(`run.mode.${workspace.mode}`) }}</TTag>
          <TButton
            size="small"
            variant="text"
            data-testid="mode-toggle"
            @click="workspace.setMode(workspace.mode === 'simple' ? 'expert' : 'simple')"
          >
            {{ workspace.mode === 'simple' ? t('run.mode.expert') : t('run.mode.simple') }}
          </TButton>
        </div>
      </header>

      <!-- The stages are the panel's tab bar: one row where a left column used to say the
           same five words and take a fifth of the map's width to do it. -->
      <StageTabs />

      <StagePanel />

      <PlanPanel v-if="workspace.stage === 'route'" />

      <div class="mt-auto flex items-center gap-2 pt-2">
        <TButton
          theme="primary"
          :loading="submitting"
          :disabled="!workspace.runnable"
          data-testid="run-submit"
          @click="submit()"
        >
          {{ t('run.submit') }}
        </TButton>
        <TButton variant="outline" data-testid="run-reset" @click="workspace.reset()">
          {{ t('common.refresh') }}
        </TButton>
      </div>
    </aside>
  </section>
</template>
