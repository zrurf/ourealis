<script setup lang="ts">
/*
 * The scale bar.
 *
 * Drawn in the interface rather than in the scene: a line mesh on the ground is tilted,
 * foreshortened and partly buried by the terrain it lies on, so its length stops
 * meaning anything the moment the camera is not looking straight down. As a 2D overlay
 * it is always horizontal, always readable, and always the length it says.
 */
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'

const props = defineProps<{
  /** Ground distance the bar stands for, metres. */
  metres: number
  /** Ground resolution the camera shows, metres per CSS pixel. */
  metresPerPixel: number
}>()

const { locale } = useI18n({ useScope: 'global' })

/** Width of the drawn bar in CSS pixels, from the label's own length. */
const widthPx = computed(() => Math.max(24, props.metres / Math.max(1e-6, props.metresPerPixel)))

/** The label, in the interface locale. */
const label = computed(() => {
  const metres = props.metres
  const text =
    metres >= 1000
      ? `${new Intl.NumberFormat(locale.value, { maximumFractionDigits: 2 }).format(metres / 1000)} km`
      : `${new Intl.NumberFormat(locale.value, { maximumFractionDigits: 0 }).format(metres)} m`
  return text
})
</script>

<template>
  <div class="flex flex-col gap-1" data-testid="scale-bar">
    <div class="flex items-end justify-between text-xs text-muted">
      <span>0</span>
      <span>{{ label }}</span>
    </div>
    <div
      class="h-2 border-b-2 border-l-2 border-r-2 border-ink/70"
      :style="{ width: `${widthPx}px` }"
      aria-hidden="true"
    />
  </div>
</template>
