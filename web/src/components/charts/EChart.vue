<script setup lang="ts">
/*
 * vue-echarts wrapper.
 *
 * Three things the raw component leaves to the caller: the echarts modules the
 * builders use are registered here and nowhere else, the light/dark theme is
 * re-applied by rebuilding the chart — echarts paints into a canvas and cannot
 * follow a CSS variable — and a `ResizeObserver` keeps the canvas at its
 * container's size even where vue-echarts' own observer is unavailable.
 */
import { onBeforeUnmount, onMounted, ref, shallowRef, watch } from 'vue'
import VChart from 'vue-echarts'
import { BarChart, LineChart, ScatterChart } from 'echarts/charts'
import {
  GridComponent,
  LegendComponent,
  MarkLineComponent,
  TitleComponent,
  TooltipComponent,
} from 'echarts/components'
import { registerTheme, use, type EChartsCoreOption } from 'echarts/core'
import { CanvasRenderer } from 'echarts/renderers'
import { CHART_THEMES, themeNameFor } from './theme'
import { useThemeStore } from '@/stores/theme'

/** The chart options this app builds need no other echarts module. */
use([CanvasRenderer, LineChart, BarChart, ScatterChart])
use([GridComponent, TooltipComponent, LegendComponent, TitleComponent, MarkLineComponent])

for (const [name, theme] of Object.entries(CHART_THEMES)) {
  registerTheme(name, theme)
}

/** The methods vue-echarts exposes on the component instance. */
interface ChartHandle {
  /** Draws an option, replacing the previous one when `notMerge` is set. */
  setOption(option: EChartsCoreOption, notMerge?: boolean): void
  /** Re-measures the container and repaints. */
  resize(): void
}

const props = withDefaults(
  defineProps<{
    /** Option built by one of `options/*`. */
    option: EChartsCoreOption
    /** Chart height; the width always follows the container. */
    height?: number | string
    /** Whether to show echarts' own loading state. */
    loading?: boolean
  }>(),
  { height: 320, loading: false },
)

const themeStore = useThemeStore()
const chart = shallowRef<ChartHandle | null>(null)
const container = ref<HTMLElement | null>(null)
let observer: ResizeObserver | null = null

/** Redraws from scratch, which is what a theme change needs: echarts keeps the old palette otherwise. */
function rebuild(): void {
  chart.value?.setOption(props.option, true)
}

watch(() => props.option, rebuild)
watch(() => themeStore.theme, rebuild)

onMounted(() => {
  if (typeof ResizeObserver === 'undefined' || container.value === null) {
    return
  }
  observer = new ResizeObserver(() => chart.value?.resize())
  observer.observe(container.value)
})

onBeforeUnmount(() => {
  observer?.disconnect()
  observer = null
})
</script>

<template>
  <div
    ref="container"
    class="w-full"
    :style="{ height: typeof props.height === 'number' ? `${props.height}px` : props.height }"
  >
    <VChart
      ref="chart"
      class="h-full w-full"
      :option="props.option"
      :theme="themeNameFor(themeStore.isDark)"
      :loading="props.loading"
      autoresize
    />
  </div>
</template>
