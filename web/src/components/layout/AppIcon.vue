<script setup lang="ts">
/*
 * One icon, stroked in the current text colour.
 *
 * A component rather than a sprite so a glyph can be dropped anywhere with the same stroke
 * weight and optical size, and so an icon button only has to name the icon it wants.
 */
import { computed } from 'vue'
import { ICONS, type IconName } from './icons'

const props = defineProps<{
  /** Glyph to draw. */
  name: IconName
  /** Drawn size in pixels. */
  size?: number
}>()

const icon = computed(() => ICONS[props.name])
const size = computed(() => props.size ?? 18)
</script>

<template>
  <svg
    :width="size"
    :height="size"
    viewBox="0 0 24 24"
    :fill="icon.filled === true ? 'currentColor' : 'none'"
    stroke="currentColor"
    stroke-width="1.7"
    stroke-linecap="round"
    stroke-linejoin="round"
    aria-hidden="true"
    focusable="false"
  >
    <path v-for="(path, index) in icon.paths" :key="index" :d="path" />
    <circle
      v-for="([cx, cy, r], index) in icon.circles ?? []"
      :key="`c${index}`"
      :cx="cx"
      :cy="cy"
      :r="r"
    />
  </svg>
</template>
