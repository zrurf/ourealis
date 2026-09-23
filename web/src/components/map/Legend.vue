<script setup lang="ts">
/*
 * The legend of everything drawn on the map.
 *
 * One entry per visible layer, describing the mapping that layer is *actually* drawn
 * with: a ramp with its ticks for a scalar field, a colour per value for a categorical
 * one, a warning colour for a set cell of a mask. It reports the range of the loaded
 * cells and says so, because a legend that reported whole-map statistics beside a
 * one-screenful surface would be wrong in a way nobody could see.
 */
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'
import type { LegendEntry, LegendModel } from '@/render/legend'

const props = defineProps<{
  /** The legend to draw. */
  model: LegendModel
  /** Localised name of a layer. */
  labelFor: (layerId: number, name: string) => string
  /** Localised unit of a layer's values, when the view has one. */
  unitFor?: (layerId: number) => string | null
}>()

const { locale } = useI18n({ useScope: 'global' })

/** Ticks as text in the interface locale. */
function tickText(value: number): string {
  const digits = Math.abs(value) >= 100 ? 0 : Math.abs(value) >= 10 ? 1 : 2
  return new Intl.NumberFormat(locale.value, { maximumFractionDigits: digits }).format(value)
}

/** Unit of an entry, from the view or from its own report. */
function unitOf(entry: LegendEntry): string | null {
  return props.unitFor?.(entry.layerId) ?? entry.unit
}

const entries = computed(() => props.model.entries)
</script>

<template>
  <section class="glass flex max-w-64 flex-col gap-2 p-3" data-testid="legend">
    <p class="text-[0.6875rem] font-medium uppercase text-muted">{{ $t('map.legend.title') }}</p>

    <div
      v-for="entry in entries"
      :key="entry.layerId"
      class="flex flex-col gap-1"
      :data-testid="`legend-entry-${entry.layerId}`"
    >
      <h3 class="truncate text-xs text-ink">{{ labelFor(entry.layerId, entry.name) }}</h3>

      <div v-if="entry.swatches.length > 0" class="flex flex-wrap gap-x-3 gap-y-1">
        <span v-for="swatch in entry.swatches" :key="swatch.label" class="flex items-center gap-1">
          <span
            class="inline-block h-2.5 w-2.5 rounded-sm"
            :style="{ background: swatch.colour }"
            aria-hidden="true"
          />
          <!-- A material is named, not numbered: "asphalt" is what a reader needs, and the
               value stays beside it so the inspector can be read against the legend. -->
          <span class="text-[0.6875rem] text-muted">
            {{ swatch.labelKey === undefined ? swatch.label : $t(swatch.labelKey) }}
          </span>
        </span>
      </div>

      <template v-else-if="entry.ramp !== null">
        <div class="h-2 w-full rounded-sm" :style="{ background: entry.ramp }" aria-hidden="true" />
        <div class="flex justify-between text-[0.6875rem] text-muted">
          <span v-for="tick in entry.ticks" :key="tick">{{ tickText(tick) }}</span>
        </div>
      </template>

      <p v-if="unitOf(entry) !== null" class="text-[0.6875rem] text-muted">{{ unitOf(entry) }}</p>
      <p
        v-if="entry.mapping === 'flag'"
        class="text-[0.6875rem] text-muted"
        data-testid="legend-flag-note"
      >
        {{ entry.swatches.length > 0 ? $t('map.legend.flagSet') : $t('map.legend.flagEmpty') }}
      </p>
      <p
        v-else-if="entry.mapping === 'category'"
        class="text-[0.6875rem] text-muted"
        data-testid="legend-category-note"
      >
        {{
          entry.swatches.length > 0 ? $t('map.legend.categoryNote') : $t('map.legend.categoryEmpty')
        }}
      </p>
    </div>

    <p v-if="model.hidden > 0" class="text-[0.6875rem] text-muted">
      {{ $t('map.legend.more', { count: model.hidden }) }}
    </p>
    <p class="text-[0.6875rem] text-muted">{{ $t('map.legend.scope') }}</p>
  </section>
</template>
