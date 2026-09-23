<script setup lang="ts">
/*
 * The interface's tooltip: tdesign's own, with the colours the interface needs.
 *
 * tdesign's default tooltip paints the inverse surface — a near-black bubble — and takes its text
 * colour from `--td-text-color-anti`. That token means "text that contrasts with the *surface*",
 * which the dark appearance maps to a near-black ink, so the bubble came out black on black. The
 * two values are pinned on this tooltip's own class instead of on the token, because the same
 * token is what a filled brand button uses for its label and that one must keep following the
 * appearance.
 *
 * A wrapper rather than a prop repeated at every call site: a hint has to look the same wherever
 * it appears, and one place that says so cannot drift.
 */
import { computed } from 'vue'
import { Tooltip as TTooltip } from 'tdesign-vue-next'

const props = defineProps<{
  /** Text of the hint; empty means "no hint", which is how callers switch it off. */
  content?: string
  /** Side the bubble is placed on, relative to its trigger. */
  placement?: 'top' | 'bottom' | 'left' | 'right'
}>()

const resolved = computed(() => ({
  content: props.content ?? '',
  placement: props.placement ?? 'top',
}))
</script>

<template>
  <TTooltip
    :content="resolved.content"
    :placement="resolved.placement"
    overlay-class-name="ourealis-tooltip"
    :show-arrow="true"
  >
    <slot />
  </TTooltip>
</template>
