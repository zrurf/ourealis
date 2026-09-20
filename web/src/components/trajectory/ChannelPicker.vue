<script setup lang="ts">
/*
 * Channel picker: which channel the trajectory line is coloured by.
 *
 * The channels are the ones `render/trajectory.ts` reads out of a truth sample.
 * The picker is a segmented control rather than a select, because switching the
 * colouring is something the user does while watching the path.
 */
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'
import { RadioGroup as TRadioGroup } from 'tdesign-vue-next'
import type { TrajectoryChannel } from '@/render/trajectory'

const props = defineProps<{
  /** Channel currently used for the colouring. */
  modelValue: TrajectoryChannel
}>()

const emit = defineEmits<{
  /** The channel the user chose. */
  'update:modelValue': [channel: TrajectoryChannel]
}>()

const { t } = useI18n({ useScope: 'global' })

const CHANNELS: ReadonlyArray<{ value: TrajectoryChannel; labelKey: string }> = [
  { value: 'speed', labelKey: 'simulation.trajectory.channelSpeed' },
  { value: 'grade', labelKey: 'simulation.trajectory.channelGrade' },
  { value: 'curvature', labelKey: 'simulation.trajectory.channelCurvature' },
  { value: 'height', labelKey: 'simulation.trajectory.channelHeight' },
]

const options = computed(() =>
  CHANNELS.map((channel) => ({ value: channel.value, label: t(channel.labelKey) })),
)

/** The select emits the raw value; anything else is ignored. */
function onChange(value: unknown): void {
  const channel = CHANNELS.find((entry) => entry.value === value)
  if (channel !== undefined) {
    emit('update:modelValue', channel.value)
  }
}
</script>

<template>
  <section data-testid="channel-picker">
    <h3 class="text-sm font-medium text-ink">{{ t('simulation.trajectory.channel') }}</h3>
    <TRadioGroup
      class="mt-2"
      variant="default-filled"
      :value="props.modelValue"
      :options="options"
      data-testid="channel-options"
      @change="onChange"
    />
  </section>
</template>
