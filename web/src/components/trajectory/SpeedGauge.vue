<script setup lang="ts">
/*
 * Speed gauge: the speed at the playhead, as a number and as a bar.
 *
 * A readout rather than a dial: the trajectory page already draws the speed curve,
 * and a second graphical encoding of the same number would compete with it. The
 * bar is scaled to the run's own speed range, which is passed in so the gauge and
 * the coloured path agree on what "fast" means. The props stay in the wire unit
 * (m/s); the numbers and the label follow the display preference.
 */
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'
import { Progress as TProgress } from 'tdesign-vue-next'
import { displaySpeed, speedUnit, useSimulationsStore } from '@/stores/simulations'

const props = defineProps<{
  /** Speed at the playhead, m/s. */
  speed: number
  /** Lowest speed of the run, m/s. */
  min: number
  /** Highest speed of the run, m/s. */
  max: number
}>()

const { t, locale } = useI18n({ useScope: 'global' })
const simulations = useSimulationsStore()

/** Unit label of the display preference. */
const unitLabel = computed(() => speedUnit(simulations.units))

/** Position on the bar, 0 to 100. */
const percentage = computed(() => {
  const span = props.max - props.min
  if (!Number.isFinite(span) || span <= 1e-9) {
    return props.speed > 0 ? 100 : 0
  }
  return Math.min(100, Math.max(0, ((props.speed - props.min) / span) * 100))
})

/** Speed at the playhead, in the display units. */
const speedText = computed(() =>
  new Intl.NumberFormat(locale.value, { maximumFractionDigits: 2 }).format(
    displaySpeed(props.speed, simulations.units),
  ),
)

/** Pace in minutes per kilometre, which is how a runner reads a speed. */
const paceText = computed(() => {
  if (!Number.isFinite(props.speed) || props.speed <= 0) {
    return '—'
  }
  const secondsPerKm = 1000 / props.speed
  const minutes = Math.floor(secondsPerKm / 60)
  const seconds = Math.round(secondsPerKm - minutes * 60)
  return `${minutes}:${String(seconds).padStart(2, '0')}`
})

/** The range the bar is scaled to, as text. */
const rangeText = computed(() => {
  const min = format(displaySpeed(props.min, simulations.units))
  const max = format(displaySpeed(props.max, simulations.units))
  return `${min}–${max} ${unitLabel.value}`
})

/** Formats one speed in the interface locale. */
function format(value: number): string {
  return new Intl.NumberFormat(locale.value, { maximumFractionDigits: 2 }).format(value)
}
</script>

<template>
  <section data-testid="speed-gauge">
    <h3 class="text-sm font-medium text-ink">{{ t('simulation.trajectory.gaugeTitle') }}</h3>
    <p class="mt-1 text-3xl font-semibold text-ink">
      <span data-testid="gauge-speed">{{ speedText }}</span>
      <span class="ml-1 text-base font-normal text-muted">{{ unitLabel }}</span>
    </p>
    <TProgress
      class="mt-2"
      theme="line"
      :percentage="percentage"
      :label="false"
      :stroke-width="8"
    />
    <dl class="mt-2 flex flex-col gap-1 text-sm">
      <div class="flex justify-between gap-3">
        <dt class="text-muted">{{ t('simulation.trajectory.pace') }}</dt>
        <dd class="font-mono text-ink" data-testid="gauge-pace">
          {{ paceText }} {{ t('simulation.trajectory.paceUnit') }}
        </dd>
      </div>
      <div class="flex justify-between gap-3">
        <dt class="text-muted">{{ t('simulation.trajectory.channelSpeed') }}</dt>
        <dd class="font-mono text-ink" data-testid="gauge-range">{{ rangeText }}</dd>
      </div>
    </dl>
  </section>
</template>
