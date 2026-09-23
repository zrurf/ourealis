<script setup lang="ts">
/*
 * The viewport controls: the icon rail a 3D tool puts at the edge of its viewport.
 *
 * The rail carries icons only, because the map is the subject and a row of sliders is not; a
 * click opens a small floating panel with the parameters of that group, and hovering says what
 * the icon is and what it is set to. That is the shape every 3D editor settled on — the viewport
 * keeps its area, the detailed settings live one click away, and the icons carry the current
 * state (the surface icon its terrace step, the sun icon its azimuth).
 *
 * The panel is *bidirectional*, which is what made the previous bar untrustworthy: the camera is
 * moved by the pointer as much as by these controls, so the scene reports its own state back and
 * the panel reads it. A parameter that is baked into the terrain (the sun, the terrace step)
 * rebuilds the loaded chunks; a camera parameter applies on the spot.
 */
import { computed, ref } from 'vue'
import { useI18n } from 'vue-i18n'
import {
  Button as TButton,
  InputNumber as TInputNumber,
  Popup as TPopup,
  Switch as TSwitch,
} from 'tdesign-vue-next'
import AppIcon from '@/components/layout/AppIcon.vue'
import ViewportSlider from '@/components/map/ViewportSlider.vue'
import { autoTerraceStep } from '@/render/shading'
import { useViewerStore } from '@/stores/viewer'
import TTooltip from '@/components/common/AppTooltip.vue'

/** Groups of the rail, in the order a reader needs them. */
type GroupId = 'camera' | 'sun' | 'surface' | 'display'

const props = defineProps<{
  /** Elevation range of the open map, which the automatic terrace step is derived from. */
  range: { min: number; max: number } | null
  /**
   * Which overlay families the open map carries.
   *
   * The availability comes with the metadata, so the rail can grey out a family before the reader
   * switches it on and waits for a request that has nothing to answer with.
   */
  overlays?: Record<string, boolean> | null
}>()

const emit = defineEmits<{
  /** The reader asked for the map's own framing back. */
  recentre: []
}>()

const { t } = useI18n({ useScope: 'global' })
const viewer = useViewerStore()

/** Group whose panel is open, or `null` while the rail is closed. */
const open = ref<GroupId | null>(null)

/** Terrace step the map would use on its own, metres; zero for a flat map. */
const autoStep = computed(() => (props.range === null ? 0 : autoTerraceStep(props.range)))

/** The terrace step in effect, whether chosen or derived. */
const effectiveStep = computed(() => viewer.terraceStepM || autoStep.value)

/** Whether the step on screen is the derived one. */
const stepIsAuto = computed(() => !viewer.terraceStepM)

/** What each group's tooltip says the current setting is. */
const summaries = computed<Record<GroupId, string>>(() => ({
  camera: t('map.viewport.summaryCamera', {
    pitch: Math.round(viewer.pitchDeg),
    fov: Math.round(viewer.fovDeg),
  }),
  sun: t('map.viewport.summarySun', { azimuth: Math.round(viewer.sunAzimuthDeg) }),
  surface: t('map.viewport.summarySurface', { step: effectiveStep.value.toFixed(1) }),
  display: viewer.gridVisible
    ? t('map.viewport.summaryGridOn', { step: Math.round(viewer.gridStepM ?? 0) })
    : t('map.viewport.summaryGridOff'),
}))

/** The overlay families the rail can switch, in the order the sections list them. */
const overlayFamilies = computed(() =>
  (
    [
      { id: 'regions', labelKey: 'map.viewer.overlayRegions' },
      { id: 'connectors', labelKey: 'map.viewer.overlayConnectors' },
      { id: 'skeleton', labelKey: 'map.viewer.overlaySkeleton' },
      { id: 'prm', labelKey: 'map.viewer.overlayPrm' },
      { id: 'direction', labelKey: 'map.viewport.arrows' },
    ] as const
  ).map((family) => ({
    id: family.id,
    labelKey: family.labelKey,
    available: props.overlays?.[family.id] !== false,
  })),
)

/** Sets the terrace step; zero means "do not terrace", `null` means "derive it". */
function setStep(value: number | null): void {
  viewer.terraceStepM = value !== null && value > 0 ? value : value === 0 ? 0 : null
}

/** Restores the derived terrace step and the default sun. */
function resetSurface(): void {
  viewer.sunAzimuthDeg = 315
  viewer.terraceStepM = null
}
</script>

<template>
  <!-- The rail floats over the canvas's own top-left corner, where a 3D tool puts it: inside
       the viewport it controls, clear of whatever the map is showing. -->
  <div class="pointer-events-auto flex flex-col gap-1" data-testid="viewport-bar">
    <div class="glass flex items-center gap-0.5 p-1">
      <TPopup
        :visible="open === 'camera'"
        placement="bottom-left"
        trigger="click"
        :show-arrow="false"
        @visible-change="(visible) => (open = visible ? 'camera' : null)"
      >
        <template #content>
          <div class="flex w-72 flex-col gap-3 p-3" data-testid="viewport-panel-camera">
            <p class="text-xs font-medium text-ink">{{ t('map.viewport.groupCamera') }}</p>
            <ViewportSlider
              :label="t('map.viewport.zoomStep')"
              :value="Math.round(viewer.zoomStep * 100)"
              :min="2"
              :max="30"
              :step="1"
              suffix="%"
              testid="viewport-zoom-step"
              @change="(value) => (viewer.zoomStep = value / 100)"
            />
            <ViewportSlider
              :label="t('map.viewport.pitch')"
              :value="viewer.pitchDeg"
              :min="5"
              :max="88"
              :step="1"
              suffix="°"
              testid="viewport-pitch"
              @change="(value) => (viewer.pitchDeg = value)"
            />
            <ViewportSlider
              :label="t('map.viewport.fov')"
              :value="viewer.fovDeg"
              :min="20"
              :max="90"
              :step="1"
              suffix="°"
              testid="viewport-fov"
              @change="(value) => (viewer.fovDeg = value)"
            />
            <label class="flex items-center justify-between gap-2 text-xs text-muted">
              <span>{{ t('map.viewport.zoomToPointer') }}</span>
              <TSwitch
                :value="viewer.zoomToPointer"
                size="small"
                data-testid="viewport-zoom-to-pointer"
                @change="(value) => (viewer.zoomToPointer = Boolean(value))"
              />
            </label>
            <label class="flex items-center justify-between gap-2 text-xs text-muted">
              <span>{{ t('map.viewport.orthographic') }}</span>
              <TSwitch
                :value="viewer.orthographic"
                size="small"
                data-testid="viewport-orthographic"
                @change="(value) => (viewer.orthographic = Boolean(value))"
              />
            </label>
          </div>
        </template>
        <TTooltip
          placement="bottom"
          :content="`${t('map.viewport.groupCamera')} · ${summaries.camera}`"
        >
          <button
            type="button"
            class="rounded-control p-1.5 text-muted transition-colors hover:bg-page hover:text-ink"
            :class="open === 'camera' ? 'bg-page text-ink' : ''"
            :aria-expanded="open === 'camera'"
            :aria-label="t('map.viewport.groupCamera')"
            data-testid="viewport-camera"
          >
            <AppIcon name="camera" />
          </button>
        </TTooltip>
      </TPopup>

      <TPopup
        :visible="open === 'sun'"
        placement="bottom-left"
        trigger="click"
        :show-arrow="false"
        @visible-change="(visible) => (open = visible ? 'sun' : null)"
      >
        <template #content>
          <div class="flex w-72 flex-col gap-3 p-3" data-testid="viewport-panel-sun">
            <p class="text-xs font-medium text-ink">{{ t('map.viewport.groupSun') }}</p>
            <ViewportSlider
              :label="t('map.viewport.sun')"
              :value="viewer.sunAzimuthDeg"
              :min="0"
              :max="360"
              :step="15"
              suffix="°"
              testid="viewport-sun"
              @change="(value) => (viewer.sunAzimuthDeg = value)"
            />
            <ViewportSlider
              :label="t('map.viewport.sunHeight')"
              :value="viewer.sunElevationDeg"
              :min="12"
              :max="85"
              :step="1"
              suffix="°"
              testid="viewport-sun-height"
              @change="(value) => (viewer.sunElevationDeg = value)"
            />
            <TButton
              size="small"
              variant="outline"
              data-testid="viewport-reset-surface"
              @click="resetSurface()"
            >
              {{ t('map.viewport.resetSurface') }}
            </TButton>
          </div>
        </template>
        <TTooltip placement="bottom" :content="`${t('map.viewport.groupSun')} · ${summaries.sun}`">
          <button
            type="button"
            class="rounded-control p-1.5 text-muted transition-colors hover:bg-page hover:text-ink"
            :class="open === 'sun' ? 'bg-page text-ink' : ''"
            :aria-expanded="open === 'sun'"
            :aria-label="t('map.viewport.groupSun')"
            data-testid="viewport-sun-button"
          >
            <AppIcon name="sun" />
          </button>
        </TTooltip>
      </TPopup>

      <TPopup
        :visible="open === 'surface'"
        placement="bottom-left"
        trigger="click"
        :show-arrow="false"
        @visible-change="(visible) => (open = visible ? 'surface' : null)"
      >
        <template #content>
          <div class="flex w-72 flex-col gap-3 p-3" data-testid="viewport-panel-surface">
            <p class="text-xs font-medium text-ink">{{ t('map.viewport.groupSurface') }}</p>
            <ViewportSlider
              :label="t('map.viewport.exaggeration')"
              :value="viewer.exaggeration"
              :min="0.5"
              :max="3"
              :step="0.1"
              :digits="1"
              suffix="×"
              testid="viewport-exaggeration"
              @change="(value) => viewer.setExaggeration(value)"
            />
            <label class="flex items-center justify-between gap-2 text-xs text-muted">
              <span>{{ t('map.viewport.terrace') }}</span>
              <span class="flex items-center gap-1">
                <TInputNumber
                  :value="effectiveStep"
                  :min="0"
                  :max="50"
                  :step="0.1"
                  :decimal-places="2"
                  size="small"
                  class="w-24"
                  data-testid="viewport-terrace"
                  @change="(value) => setStep(value === '' ? null : Number(value))"
                />
                <span>{{ t('map.viewport.terraceUnit') }}</span>
              </span>
            </label>
            <p class="text-[0.6875rem] text-muted">{{ t('map.viewport.terraceHint') }}</p>
            <TButton
              size="small"
              variant="outline"
              :disabled="stepIsAuto"
              data-testid="viewport-terrace-auto"
              @click="setStep(null)"
            >
              {{ t('map.viewport.auto') }}
            </TButton>
          </div>
        </template>
        <TTooltip
          placement="bottom"
          :content="`${t('map.viewport.groupSurface')} · ${summaries.surface}`"
        >
          <button
            type="button"
            class="rounded-control p-1.5 text-muted transition-colors hover:bg-page hover:text-ink"
            :class="open === 'surface' ? 'bg-page text-ink' : ''"
            :aria-expanded="open === 'surface'"
            :aria-label="t('map.viewport.groupSurface')"
            data-testid="viewport-surface"
          >
            <AppIcon name="cube" />
          </button>
        </TTooltip>
      </TPopup>

      <TPopup
        :visible="open === 'display'"
        placement="bottom-left"
        trigger="click"
        :show-arrow="false"
        @visible-change="(visible) => (open = visible ? 'display' : null)"
      >
        <template #content>
          <div class="flex w-72 flex-col gap-3 p-3" data-testid="viewport-panel-display">
            <p class="text-xs font-medium text-ink">{{ t('map.viewport.groupDisplay') }}</p>
            <label class="flex items-center justify-between gap-2 text-xs text-muted">
              <span>{{ t('map.viewport.grid') }}</span>
              <span class="flex items-center gap-1">
                <TInputNumber
                  :value="viewer.gridStepM ?? 0"
                  :min="0"
                  :max="200"
                  :step="5"
                  size="small"
                  class="w-20"
                  data-testid="viewport-grid-step"
                  @change="
                    (value) => (viewer.gridStepM = Number(value) <= 0 ? null : Number(value))
                  "
                />
                <span>{{ t('map.viewport.gridUnit') }}</span>
                <TSwitch
                  :value="viewer.gridVisible"
                  size="small"
                  data-testid="viewport-grid"
                  @change="(value) => (viewer.gridVisible = Boolean(value))"
                />
              </span>
            </label>
            <p class="text-[0.6875rem] text-muted">{{ t('map.viewport.gridHint') }}</p>
            <p class="mt-1 text-xs font-medium text-ink">{{ t('map.viewer.overlays') }}</p>
            <label
              v-for="family in overlayFamilies"
              :key="family.id"
              class="flex items-center justify-between gap-2 text-xs"
              :class="family.available ? 'text-muted' : 'text-line'"
            >
              <span>{{ t(family.labelKey) }}</span>
              <TSwitch
                :value="viewer.overlays[family.id]"
                size="small"
                :disabled="!family.available"
                :data-testid="`viewport-overlay-${family.id}`"
                @change="(value) => viewer.setOverlay(family.id, Boolean(value))"
              />
            </label>
            <p class="text-[0.6875rem] text-muted">{{ t('map.viewport.overlaysHint') }}</p>
          </div>
        </template>
        <TTooltip
          placement="bottom"
          :content="`${t('map.viewport.groupDisplay')} · ${summaries.display}`"
        >
          <button
            type="button"
            class="rounded-control p-1.5 text-muted transition-colors hover:bg-page hover:text-ink"
            :class="open === 'display' ? 'bg-page text-ink' : ''"
            :aria-expanded="open === 'display'"
            :aria-label="t('map.viewport.groupDisplay')"
            data-testid="viewport-display"
          >
            <AppIcon name="grid" />
          </button>
        </TTooltip>
      </TPopup>

      <span class="mx-1 h-5 w-px bg-line" aria-hidden="true" />

      <TTooltip placement="bottom" :content="t('map.viewport.resetView')">
        <button
          type="button"
          class="rounded-control p-1.5 text-muted transition-colors hover:bg-page hover:text-ink"
          :aria-label="t('map.viewport.resetView')"
          data-testid="viewport-reset-view"
          @click="emit('recentre')"
        >
          <AppIcon name="target" />
        </button>
      </TTooltip>
    </div>
  </div>
</template>
