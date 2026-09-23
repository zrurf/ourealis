/*
 * Viewer settings: what is drawn and how.
 *
 * The store holds the viewer's own state — which map, which layers are covered, which
 * backend, and the *viewport parameters* the renderer bakes into the surface — while the
 * scene reads it and the renderer keeps no Vue state of its own. Values live here rather
 * than in the component so the controls and the canvas cannot disagree, and so both
 * map-bearing views open the same controls on the same engine.
 *
 * The parameters are split by cost: a camera parameter (zoom step, pitch, field of view,
 * projection) applies immediately, while a parameter that is baked into the vertex
 * colours or the geometry (the sun, the terrace step, the surface ramp) makes the loaded
 * chunks rebuild. The store does not know that — it stores values, and the views decide
 * what to rebuild.
 */
import { defineStore } from 'pinia'
import { computed, ref } from 'vue'
import { LAYER_ELEVATION } from '@/types/map'
import type { ColorMapping } from '@/types/map'
import type { OverlayKind } from '@/render/overlays'
import type { EngineBackend } from '@/render/engine'

/** Height exaggeration bounds the viewer offers, per doc §6.4. */
export const MIN_EXAGGERATION = 0.5

/** Upper bound of the height exaggeration control. */
export const MAX_EXAGGERATION = 3.0

/** Zoom applied by one wheel notch, as a fraction of the camera radius. */
export const ZOOM_STEP = 0.1

/** Overlay visibility, one flag per family. */
export interface OverlayToggles {
  /** Region outlines. */
  regions: boolean
  /** Z-axis connectors. */
  connectors: boolean
  /** Aggregated quadtree blocks. */
  skeleton: boolean
  /** Roadmap edges. */
  prm: boolean
  /** Direction-constraint arrows. */
  direction: boolean
}

/** One layer's client-side display state. */
export interface LayerView {
  /** Layer id as the file names it. */
  layerId: number
  /** Layer name, as the service reports it. */
  name: string
  /** Whether the layer is drawn. */
  visible: boolean
  /** How its samples are coloured. */
  mapping: ColorMapping
  /**
   * Layer kind, as the file names it (`raster`, `bitmap`, `region`, `vector`, `graph`).
   *
   * Only the cell kinds have chunks to drape, so the kind decides whether a layer can
   * be loaded as a surface at all.
   */
  kind: string
}

/** What the viewer draws and how. */
export const useViewerStore = defineStore('viewer', () => {
  const mapId = ref<string | null>(null)
  const layers = ref<LayerView[]>([])
  const overlays = ref<OverlayToggles>({
    regions: false,
    connectors: false,
    skeleton: false,
    prm: false,
    direction: false,
  })
  const exaggeration = ref(1)
  const engineBackend = ref<EngineBackend | null>(null)
  const engineReason = ref<string | null>(null)
  const engineNoticeDismissed = ref(false)
  const terrainLevel = ref(0)

  const zoomStep = ref(ZOOM_STEP)
  const pitchDeg = ref(45)
  const fovDeg = ref(45)
  const orthographic = ref(false)
  const zoomToPointer = ref(true)
  const sunAzimuthDeg = ref(315)
  const sunElevationDeg = ref(52)
  /**
   * Terrace step in metres; `0` draws the surface as surveyed.
   *
   * Zero is the default: a terraced surface is a *reading* of the relief (it turns a slope into
   * levels), not a neutral way to draw it, and a reader who has not asked for it should see the
   * ground rather than a staircase. `null` means "derive a step from the map's relief", which is
   * the middle ground the surface panel offers.
   */
  const terraceStepM = ref<number | null>(0)
  /** Grid step in metres, or `null` for a step derived from the map's extent. */
  const gridStepM = ref<number | null>(null)
  const gridVisible = ref(false)

  /** Layers the viewer is drawing. */
  const visibleLayers = computed(() => layers.value.filter((layer) => layer.visible))

  /** Families whose toggle is on. */
  const activeOverlays = computed(() =>
    (Object.entries(overlays.value) as Array<[OverlayKind, boolean]>)
      .filter(([, visible]) => visible)
      .map(([kind]) => kind),
  )

  /** True while the vertical scale is at its neutral value. */
  const isNeutralScale = computed(() => Math.abs(exaggeration.value - 1) < 1e-6)

  /** Replaces the layer list, keeping the state of layers that are still present. */
  function setLayers(next: Array<Omit<LayerView, 'visible'>>): void {
    const previous = new Map(layers.value.map((layer) => [layer.layerId, layer]))
    layers.value = next.map((layer) => ({
      ...layer,
      // Only the surface starts visible: it is the map's shape, while a thematic layer
      // covers it entirely, so one switched on by default would hide the terrain
      // behind a single field and look like the viewer had drawn nothing.
      visible: previous.get(layer.layerId)?.visible ?? layer.layerId === LAYER_ELEVATION,
      mapping: previous.get(layer.layerId)?.mapping ?? layer.mapping,
    }))
  }

  /** Shows or hides one layer. */
  function setLayerVisible(layerId: number, visible: boolean): void {
    layers.value = layers.value.map((layer) =>
      layer.layerId === layerId ? { ...layer, visible } : layer,
    )
  }

  /** Switches one layer's colour mapping. */
  function setLayerMapping(layerId: number, mapping: ColorMapping): void {
    layers.value = layers.value.map((layer) =>
      layer.layerId === layerId ? { ...layer, mapping } : layer,
    )
  }

  /** Shows or hides one overlay family. */
  function setOverlay(kind: OverlayKind, visible: boolean): void {
    overlays.value = { ...overlays.value, [kind]: visible }
  }

  /** Sets the height exaggeration, clamped to the range the control offers. */
  function setExaggeration(value: number): void {
    exaggeration.value = Math.min(MAX_EXAGGERATION, Math.max(MIN_EXAGGERATION, value))
  }

  /** Records the backend the probe chose and its reason. */
  function setEngine(backend: EngineBackend | null, reason: string | null): void {
    engineBackend.value = backend
    engineReason.value = reason
  }

  /** Restores the map-dependent state for a newly opened map, keeping the viewport. */
  function reset(nextMapId: string | null): void {
    mapId.value = nextMapId
    layers.value = []
    overlays.value = {
      regions: false,
      connectors: false,
      skeleton: false,
      prm: false,
      direction: false,
    }
    // The terrace and grid steps are derived from the map, so they follow it.
    terraceStepM.value = 0
    gridStepM.value = null
    terrainLevel.value = 0
    exaggeration.value = 1
  }

  return {
    mapId,
    layers,
    overlays,
    exaggeration,
    engineBackend,
    engineReason,
    engineNoticeDismissed,
    terrainLevel,
    zoomStep,
    pitchDeg,
    fovDeg,
    orthographic,
    zoomToPointer,
    sunAzimuthDeg,
    sunElevationDeg,
    terraceStepM,
    gridStepM,
    gridVisible,
    visibleLayers,
    activeOverlays,
    isNeutralScale,
    setLayers,
    setLayerVisible,
    setLayerMapping,
    setOverlay,
    setExaggeration,
    setEngine,
    reset,
  }
})
