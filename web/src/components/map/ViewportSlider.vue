<script setup lang="ts">
/*
 * One labelled slider in a viewport panel.
 *
 * Every parameter in the viewport rail is the same control: a name, the value it currently
 * holds, and a slider. Written once, the panels stay readable and every row keeps the same
 * label width and number format — which is what makes a panel of six parameters scannable.
 */
import { computed } from 'vue'
import { Slider as TSlider } from 'tdesign-vue-next'

const props = defineProps<{
  /** Already-translated label. */
  label: string
  /** Current value. */
  value: number
  /** Lower bound of the slider. */
  min: number
  /** Upper bound of the slider. */
  max: number
  /** Step the slider moves in. */
  step: number
  /** Digits in the readout. */
  digits?: number
  /** Text after the readout, e.g. a degree sign or a multiplier. */
  suffix?: string
  /** Test id of the slider itself. */
  testid: string
}>()

const emit = defineEmits<{
  /** The reader settled on a value. */
  change: [value: number]
}>()

const text = computed(() => `${props.value.toFixed(props.digits ?? 0)}${props.suffix ?? ''}`)
</script>

<template>
  <label class="flex flex-col gap-1">
    <span class="flex items-center justify-between text-xs text-muted">
      <span>{{ label }}</span>
      <span class="font-mono text-ink" :data-testid="`${testid}-value`">{{ text }}</span>
    </span>
    <TSlider
      :value="value"
      :min="min"
      :max="max"
      :step="step"
      :data-testid="testid"
      @change="(next) => emit('change', Number(next))"
    />
  </label>
</template>
