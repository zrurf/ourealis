<script setup lang="ts">
/*
 * The cell inspector.
 *
 * Shows what the loaded data holds for the cell under the cursor: the position, the
 * global cell index, the chunk it belongs to and every declared layer's value there.
 * It is the answer to "what is actually in the file at this spot", which the viewer
 * could previously only answer for elevation and only as a number in a corner.
 *
 * Everything is rendered as it arrives from `render/inspect.ts`; this component does no
 * arithmetic on the values, so what is shown is what was read.
 */
import { computed, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import { Button as TButton, Tag as TTag } from 'tdesign-vue-next'
import { isFeatureLayer, type CellMeaning, type CellReport } from '@/render/inspect'
import { readStored, writeStored } from '@/utils/storage'

const props = defineProps<{
  /** The cell, or `null` while nothing is picked. */
  report: CellReport | null
  /** Localised name of a layer, when the view has one. */
  layerLabel: (layerId: number, serviceName: string) => string
  /** Whether the pick came from the terrain rather than the plane fallback. */
  onTerrain: boolean
}>()

const { t, locale } = useI18n({ useScope: 'global' })

/**
 * Whether the body is folded away.
 *
 * Remembered across sessions like the appearance: a reader who collapsed the panel did it
 * to see the map, and having it unfold again on every visit would make the control
 * pointless.
 */
const COLLAPSE_KEY = 'ourealis.inspector.collapsed'

const collapsed = ref(readStored(COLLAPSE_KEY) === '1')

watch(collapsed, (value) => writeStored(COLLAPSE_KEY, value ? '1' : '0'))

/** Number in the interface locale. */
function number(value: number, digits = 2): string {
  return new Intl.NumberFormat(locale.value, { maximumFractionDigits: digits }).format(value)
}

/** Layers that carry a value, so the empty ones can be reported as one line. */
const loaded = computed(() => (props.report?.layers ?? []).filter((layer) => layer.loaded))

/** Layers the map declares that hold no cells, or whose chunk is not loaded. */
const unavailable = computed(() => (props.report?.layers ?? []).filter((layer) => !layer.loaded))

/** Elevation as a string, or a dash while it is not loaded. */
const elevationText = computed(() =>
  props.report?.elevation === null || props.report?.elevation === undefined
    ? '—'
    : `${number(props.report.elevation)} m`,
)

/** Catalog key of a cell's meaning, so the panel says what a value *is*. */
const MEANING_KEYS: Readonly<Record<CellMeaning, string>> = {
  elevation: 'map.inspector.meaningElevation',
  slope: 'map.inspector.meaningSlope',
  distance: 'map.inspector.meaningDistance',
  cost: 'map.inspector.meaningCost',
  blocked: 'map.inspector.meaningBlocked',
  passable: 'map.inspector.meaningPassable',
  direction: 'map.inspector.meaningDirection',
  surface: 'map.inspector.meaningSurface',
  feature: 'map.inspector.meaningFeature',
  section: 'map.inspector.meaningSection',
  unknown: 'map.inspector.meaningUnknown',
}

/** Meaning of one layer's reading, as words. */
function meaningText(layer: { meaning: CellMeaning; blocked?: boolean }): string {
  if (layer.meaning === 'blocked' && layer.blocked === false) {
    return t('map.inspector.meaningPassable')
  }
  return t(MEANING_KEYS[layer.meaning])
}

/** One-line summary of the pinned cell, shown while the body is folded. */
const summary = computed(() => {
  const report = props.report
  if (report === null) {
    return t('map.inspector.empty')
  }
  return `${number(report.position.x, 1)}, ${number(report.position.y, 1)} · ${elevationText.value}`
})
</script>

<template>
  <section class="glass flex w-80 flex-col gap-3 overflow-hidden p-4" data-testid="inspector-panel">
    <header class="flex items-center justify-between gap-2">
      <h2 class="text-sm font-medium text-ink">{{ t('map.inspector.title') }}</h2>
      <div class="flex items-center gap-1">
        <TTag v-if="report !== null" size="small" variant="light">
          {{ onTerrain ? t('map.inspector.sourceTerrain') : t('map.inspector.sourcePlane') }}
        </TTag>
        <TButton
          size="small"
          variant="text"
          :aria-expanded="!collapsed"
          :aria-label="collapsed ? t('map.inspector.expand') : t('map.inspector.collapse')"
          data-testid="inspector-toggle"
          @click="collapsed = !collapsed"
        >
          {{ collapsed ? t('map.inspector.expand') : t('map.inspector.collapse') }}
        </TButton>
      </div>
    </header>

    <p v-if="collapsed" class="truncate text-xs text-muted" data-testid="inspector-summary">
      {{ summary }}
    </p>

    <div v-else class="flex min-h-0 flex-1 flex-col gap-3 overflow-y-auto">
      <p v-if="report === null" class="text-sm text-muted" data-testid="inspector-empty">
        {{ t('map.inspector.empty') }}
      </p>

      <template v-else>
        <dl class="flex flex-col gap-1 text-sm" data-testid="inspector-cell">
          <div class="flex justify-between gap-3">
            <dt class="text-muted">{{ t('map.inspector.position') }}</dt>
            <dd class="font-mono text-ink" data-testid="inspector-position">
              {{ number(report.position.x) }}, {{ number(report.position.y) }}
            </dd>
          </div>
          <div class="flex justify-between gap-3">
            <dt class="text-muted">{{ t('map.inspector.elevation') }}</dt>
            <dd class="font-mono text-ink">{{ elevationText }}</dd>
          </div>
          <div class="flex justify-between gap-3">
            <dt class="text-muted">{{ t('map.inspector.cell') }}</dt>
            <dd class="font-mono text-ink">
              {{ report.cell.i }}, {{ report.cell.j }} @ {{ number(report.level.cellSizeM) }} m
            </dd>
          </div>
          <div class="flex justify-between gap-3">
            <dt class="text-muted">{{ t('map.inspector.level') }}</dt>
            <dd class="font-mono text-ink">{{ report.level.level }}</dd>
          </div>
          <div class="flex justify-between gap-3">
            <dt class="text-muted">{{ t('map.inspector.chunk') }}</dt>
            <dd class="font-mono text-ink" data-testid="inspector-chunk">
              {{ report.chunk.id === null ? '—' : `0x${report.chunk.id.toString(16)}` }}
            </dd>
          </div>
          <div class="flex justify-between gap-3">
            <dt class="text-muted">{{ t('map.inspector.chunkGrid') }}</dt>
            <dd class="font-mono text-ink">{{ report.chunk.ix }}, {{ report.chunk.iy }}</dd>
          </div>
          <div class="flex justify-between gap-3">
            <dt class="text-muted">{{ t('map.inspector.localCell') }}</dt>
            <dd class="font-mono text-ink">
              {{ report.chunk.local.x }}, {{ report.chunk.local.y }}
            </dd>
          </div>
          <div v-if="report.chunk.shape !== null" class="flex justify-between gap-3">
            <dt class="text-muted">{{ t('map.inspector.chunkShape') }}</dt>
            <dd class="font-mono text-ink">
              {{ report.chunk.shape.width }}×{{ report.chunk.shape.height }}×{{
                report.chunk.shape.channels
              }}
            </dd>
          </div>
          <div v-if="report.chunk.inMap !== null" class="flex justify-between gap-3">
            <dt class="text-muted">{{ t('map.inspector.chunkInMap') }}</dt>
            <dd class="font-mono text-ink">
              {{ report.chunk.inMap.width }}×{{ report.chunk.inMap.height }}
            </dd>
          </div>
          <div class="flex justify-between gap-3">
            <dt class="text-muted">{{ t('map.inspector.cellExtent') }}</dt>
            <dd class="font-mono text-ink">
              {{ number(report.cellBounds.min_x, 1) }}, {{ number(report.cellBounds.min_y, 1) }} →
              {{ number(report.cellBounds.max_x, 1) }}, {{ number(report.cellBounds.max_y, 1) }}
            </dd>
          </div>
          <div class="flex justify-between gap-3">
            <dt class="text-muted">{{ t('map.inspector.levelDims') }}</dt>
            <dd class="font-mono text-ink">
              {{ report.levelDims.width }}×{{ report.levelDims.height }}
            </dd>
          </div>
          <div class="flex justify-between gap-3">
            <dt class="text-muted">{{ t('map.inspector.inMap') }}</dt>
            <dd class="text-ink">{{ report.inMap ? t('common.yes') : t('common.no') }}</dd>
          </div>
        </dl>

        <div class="flex flex-col gap-2">
          <h3 class="text-xs font-medium uppercase text-muted">{{ t('map.inspector.layers') }}</h3>
          <div
            v-for="layer in loaded"
            :key="layer.layerId"
            class="flex flex-col gap-0.5 border-b border-line pb-1 last:border-b-0"
            :data-testid="`inspector-layer-${layer.layerId}`"
          >
            <div class="flex items-center justify-between gap-2">
              <span class="truncate text-sm text-ink">{{
                layerLabel(layer.layerId, layer.name)
              }}</span>
              <TTag
                v-if="isFeatureLayer(layer.layerId)"
                size="small"
                variant="light"
                :data-testid="`inspector-type-${layer.layerId}`"
              >
                {{ t('map.inspector.feature') }}
              </TTag>
            </div>
            <!-- What the cell *is*, before its number: the question a reader brings to the panel. -->
            <p class="text-xs text-ink" :data-testid="`inspector-meaning-${layer.layerId}`">
              {{ meaningText(layer) }}
              <span class="text-muted">
                · {{ layer.kind }} · {{ t('map.inspector.channels', { count: layer.channels }) }}
              </span>
            </p>
            <p class="font-mono text-xs text-muted">
              <span v-for="(value, index) in layer.values" :key="index" class="mr-2">
                {{ t('map.inspector.channel', { index }) }}
                {{ number(value, 3) }}
              </span>
            </p>
            <p v-if="layer.loadedRange !== null" class="font-mono text-[0.6875rem] text-muted">
              {{
                t('map.inspector.layerRange', {
                  min: number(layer.loadedRange.min, 2),
                  max: number(layer.loadedRange.max, 2),
                })
              }}
              ·
              {{
                t('map.inspector.quantisation', {
                  scale: number(layer.quantisation.scale, 4),
                  bias: number(layer.quantisation.bias, 2),
                })
              }}
            </p>
            <p v-if="layer.direction !== undefined" class="text-xs text-muted">
              {{
                t('map.inspector.direction', {
                  angle: number(layer.direction.angleDeg, 0),
                  strength: layer.direction.strength,
                })
              }}
            </p>
            <p
              v-if="layer.blocked !== undefined"
              class="text-xs"
              :class="layer.blocked ? 'text-danger' : 'text-muted'"
            >
              {{ layer.blocked ? t('map.inspector.forbidden') : t('map.inspector.passable') }}
            </p>
          </div>

          <p v-if="unavailable.length > 0" class="text-xs text-muted">
            {{ t('map.inspector.unavailable', { count: unavailable.length }) }}
          </p>
        </div>
      </template>
    </div>
  </section>
</template>
