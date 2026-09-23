<script setup lang="ts">
/*
 * The sensor block: sample rates, the event models, the mount and determinism.
 *
 * Every control has a "service default" position, because the request is a sparse
 * override and a form that filled the blank fields in would silently stop tracking
 * the simulator's own defaults. The tri-state toggles are selects rather than
 * switches for that reason.
 */
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'
import { InputNumber as TInputNumber, Select as TSelect } from 'tdesign-vue-next'
import type { SensorDraft, SimulationFormState } from './request'

const props = defineProps<{
  /** The form state this component edits. */
  modelValue: SimulationFormState
  /**
   * Whether to show only the sample rates.
   *
   * The rates decide what the run produces; the noise, event and mount switches are
   * the expert half, and a recipe that wants them sets them.
   */
  simple?: boolean
}>()

const emit = defineEmits<{
  /** The form state after an edit. */
  'update:modelValue': [state: SimulationFormState]
}>()

const { t } = useI18n({ useScope: 'global' })

/** On/off/default choices of a tri-state toggle. */
const toggleOptions = computed(() => [
  { value: 'default', label: t('simulation.form.serviceDefault') },
  { value: 'on', label: t('simulation.form.on') },
  { value: 'off', label: t('simulation.form.off') },
])

const mountOptions = computed(() => [
  { value: 'default', label: t('simulation.form.serviceDefault') },
  { value: 'body', label: t('simulation.sensors.mountBody') },
  { value: 'head', label: t('simulation.sensors.mountHead') },
])

/** Applies a patch to the sensor block. */
function setSensor(changes: Partial<SensorDraft>): void {
  emit('update:modelValue', {
    ...props.modelValue,
    sensors: { ...props.modelValue.sensors, ...changes },
  })
}

/** Reads a tri-state toggle the way the select shows it. */
function toggleValue(value: boolean | null): string {
  return value === null ? 'default' : value ? 'on' : 'off'
}

/** Turns a select payload back into a tri-state value. */
function toggleTo(value: unknown): boolean | null {
  return value === 'on' ? true : value === 'off' ? false : null
}

/** Sets one nullable number field. */
function setNumber(field: keyof SensorDraft, value: unknown): void {
  const numeric = Number(value)
  setSensor({ [field]: Number.isFinite(numeric) ? numeric : null } as Partial<SensorDraft>)
}

/** Reads a nullable number field for an input. */
function numberValue(field: keyof SensorDraft): number | null {
  const value = props.modelValue.sensors[field]
  return typeof value === 'number' ? value : null
}
</script>

<template>
  <section data-testid="sensor-form">
    <h3 class="text-sm font-medium text-ink">{{ t('simulation.sensors.rates') }}</h3>
    <!-- One control per row: the panel is a fixed-width column, and a row of four
         number fields in it left each field narrower than its own stepper buttons —
         the labels overlapped and the last one was clipped off the edge. -->
    <div class="mt-2 flex flex-col gap-2">
      <label class="flex flex-col gap-1">
        <span class="text-xs text-muted">{{ t('simulation.sensors.gnss') }}</span>
        <TInputNumber
          :value="numberValue('gnss_rate_hz') ?? undefined"
          :min="0"
          :decimal-places="3"
          class="w-full"
          :placeholder="t('simulation.form.serviceDefault')"
          data-testid="sensor-gnss-rate"
          @change="(value) => setNumber('gnss_rate_hz', value)"
        />
      </label>
      <label class="flex flex-col gap-1">
        <span class="text-xs text-muted">{{ t('simulation.sensors.imu') }}</span>
        <TInputNumber
          :value="numberValue('imu_rate_hz') ?? undefined"
          :min="0"
          :decimal-places="3"
          class="w-full"
          :placeholder="t('simulation.form.serviceDefault')"
          data-testid="sensor-imu-rate"
          @change="(value) => setNumber('imu_rate_hz', value)"
        />
      </label>
      <label class="flex flex-col gap-1">
        <span class="text-xs text-muted">{{ t('simulation.sensors.mag') }}</span>
        <TInputNumber
          :value="numberValue('mag_rate_hz') ?? undefined"
          :min="0"
          :decimal-places="3"
          class="w-full"
          :placeholder="t('simulation.form.serviceDefault')"
          data-testid="sensor-mag-rate"
          @change="(value) => setNumber('mag_rate_hz', value)"
        />
      </label>
      <label class="flex flex-col gap-1">
        <span class="text-xs text-muted">{{ t('simulation.sensors.baro') }}</span>
        <TInputNumber
          :value="numberValue('baro_rate_hz') ?? undefined"
          :min="0"
          :decimal-places="3"
          class="w-full"
          :placeholder="t('simulation.form.serviceDefault')"
          data-testid="sensor-baro-rate"
          @change="(value) => setNumber('baro_rate_hz', value)"
        />
      </label>
    </div>

    <template v-if="!simple">
      <h3 class="mt-4 text-sm font-medium text-ink">{{ t('simulation.sensors.events') }}</h3>
      <div class="mt-2 flex flex-col gap-2">
        <label class="flex flex-col gap-1">
          <span class="text-xs text-muted">{{ t('simulation.sensors.multipath') }}</span>
          <TSelect
            :value="toggleValue(modelValue.sensors.multipath_enabled)"
            :options="toggleOptions"
            data-testid="sensor-multipath"
            @change="(value) => setSensor({ multipath_enabled: toggleTo(value) })"
          />
        </label>
        <label class="flex flex-col gap-1">
          <span class="text-xs text-muted">{{ t('simulation.sensors.magneticDisturbance') }}</span>
          <TSelect
            :value="toggleValue(modelValue.sensors.magnetic_disturbance_enabled)"
            :options="toggleOptions"
            data-testid="sensor-magnetic"
            @change="(value) => setSensor({ magnetic_disturbance_enabled: toggleTo(value) })"
          />
        </label>
        <label class="flex flex-col gap-1">
          <span class="text-xs text-muted">{{ t('simulation.sensors.jitter') }}</span>
          <TSelect
            :value="toggleValue(modelValue.sensors.jitter_enabled)"
            :options="toggleOptions"
            data-testid="sensor-jitter"
            @change="(value) => setSensor({ jitter_enabled: toggleTo(value) })"
          />
        </label>
        <label class="flex flex-col gap-1">
          <span class="text-xs text-muted">{{ t('simulation.sensors.jitterSigma') }}</span>
          <TInputNumber
            :value="numberValue('jitter_sigma_m') ?? undefined"
            :min="0"
            :decimal-places="3"
            class="w-full"
            :placeholder="t('simulation.form.serviceDefault')"
            @change="(value) => setNumber('jitter_sigma_m', value)"
          />
        </label>
        <label class="flex flex-col gap-1">
          <span class="text-xs text-muted">{{ t('simulation.sensors.mount') }}</span>
          <TSelect
            :value="modelValue.sensors.mount ?? 'default'"
            :options="mountOptions"
            data-testid="sensor-mount"
            @change="
              (value) => setSensor({ mount: value === 'body' || value === 'head' ? value : null })
            "
          />
        </label>
        <label class="flex flex-col gap-1">
          <span class="text-xs text-muted">{{ t('simulation.sensors.forceDeterministic') }}</span>
          <TSelect
            :value="toggleValue(modelValue.sensors.force_deterministic_events)"
            :options="toggleOptions"
            data-testid="sensor-deterministic"
            @change="(value) => setSensor({ force_deterministic_events: toggleTo(value) })"
          />
          <span class="text-xs text-muted">{{
            t('simulation.sensors.forceDeterministicHint')
          }}</span>
        </label>
        <label class="flex flex-col gap-1">
          <span class="text-xs text-muted">{{ t('simulation.sensors.referencePressure') }}</span>
          <TInputNumber
            :value="numberValue('reference_pressure_pa') ?? undefined"
            :min="0"
            :decimal-places="2"
            class="w-full"
            :placeholder="t('simulation.form.serviceDefault')"
            @change="(value) => setNumber('reference_pressure_pa', value)"
          />
        </label>
      </div>
    </template>
  </section>
</template>
