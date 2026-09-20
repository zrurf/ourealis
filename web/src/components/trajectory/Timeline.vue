<script setup lang="ts">
/*
 * Timeline scrubber.
 *
 * The slider is bound to the sample index rather than to the time, so a run whose
 * samples are not evenly spaced still moves the playhead one sample per step and
 * the marker always lands on a sample the service produced. The labels show time,
 * which is what a reader thinks in.
 */
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'
import { Slider as TSlider } from 'tdesign-vue-next'

const props = defineProps<{
  /** Sample times, seconds from the start of the recording, ascending. */
  times: readonly number[]
  /** Index of the sample the playhead is on. */
  modelValue: number
}>()

const emit = defineEmits<{
  /** The index the playhead moved to. */
  'update:modelValue': [index: number]
}>()

const { t, locale } = useI18n({ useScope: 'global' })

/** Highest index the slider can reach. */
const last = computed(() => Math.max(0, props.times.length - 1))

/** Time under the playhead, seconds. */
const current = computed(() => props.times[props.modelValue] ?? 0)

/** End of the recording, seconds. */
const total = computed(() => props.times[last.value] ?? 0)

/** Formats a duration as `mm:ss.s`, which reads better than raw seconds. */
function clock(seconds: number): string {
  const safe = Number.isFinite(seconds) ? Math.max(0, seconds) : 0
  const minutes = Math.floor(safe / 60)
  const rest = safe - minutes * 60
  return `${String(minutes).padStart(2, '0')}:${rest.toFixed(1).padStart(4, '0')}`
}

/** Formats the sample count in the interface locale. */
const sampleText = computed(() =>
  t('simulation.trajectory.samples', {
    count: new Intl.NumberFormat(locale.value).format(props.times.length),
  }),
)
</script>

<template>
  <section data-testid="timeline">
    <div class="flex items-center justify-between text-sm">
      <span class="font-mono text-ink" data-testid="timeline-current">{{ clock(current) }}</span>
      <span class="text-muted">{{ sampleText }}</span>
      <span class="font-mono text-muted" data-testid="timeline-total">{{ clock(total) }}</span>
    </div>
    <TSlider
      class="mt-1"
      :value="modelValue"
      :min="0"
      :max="last"
      :step="1"
      :disabled="times.length === 0"
      :label="false"
      :tooltip-props="{ content: clock(current) }"
      data-testid="timeline-slider"
      @change="(value) => emit('update:modelValue', Number(value))"
    />
  </section>
</template>
