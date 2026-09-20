<script setup lang="ts">
/*
 * Playback controls: play, pause, rate, single-step and rewind.
 *
 * The component holds no clock of its own — the view owns the animation frame
 * loop and tells this one what the state is — so a rate change immediately affects
 * the running playback instead of waiting for the next tick of a second timer.
 */
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'
import { Button as TButton, Select as TSelect } from 'tdesign-vue-next'

const props = defineProps<{
  /** Whether playback is running. */
  playing: boolean
  /** Playback speed, real seconds per simulated second. */
  rate: number
  /** Whether the playhead can move at all. */
  enabled?: boolean
}>()

const emit = defineEmits<{
  /** Toggles play and pause. */
  toggle: []
  /** Moves the playhead by one sample; `-1` or `1`. */
  step: [direction: -1 | 1]
  /** Sets the playback speed. */
  'update:rate': [rate: number]
  /** Returns the playhead to the first sample. */
  rewind: []
}>()

const { t } = useI18n({ useScope: 'global' })

/** Rates the control offers; 1 is real time. */
const RATES = [0.25, 0.5, 1, 2, 4, 8]

const rateOptions = computed(() => RATES.map((rate) => ({ value: rate, label: `${rate}×` })))

const active = computed(() => props.enabled !== false)
</script>

<template>
  <div class="flex items-center gap-2" data-testid="playback-controls">
    <TButton
      theme="primary"
      :disabled="!active"
      data-testid="playback-toggle"
      @click="emit('toggle')"
    >
      {{ playing ? t('simulation.trajectory.pause') : t('simulation.trajectory.play') }}
    </TButton>
    <TButton
      variant="outline"
      :disabled="!active"
      data-testid="playback-back"
      @click="emit('step', -1)"
    >
      {{ t('simulation.trajectory.stepBack') }}
    </TButton>
    <TButton
      variant="outline"
      :disabled="!active"
      data-testid="playback-forward"
      @click="emit('step', 1)"
    >
      {{ t('simulation.trajectory.stepForward') }}
    </TButton>
    <TButton variant="outline" :disabled="!active" @click="emit('rewind')">
      {{ t('simulation.trajectory.reset') }}
    </TButton>
    <label class="ml-2 flex items-center gap-2">
      <span class="text-sm text-muted">{{ t('simulation.trajectory.rate') }}</span>
      <TSelect
        :value="rate"
        :options="rateOptions"
        class="w-24"
        data-testid="playback-rate"
        @change="(value) => emit('update:rate', Number(value))"
      />
    </label>
  </div>
</template>
