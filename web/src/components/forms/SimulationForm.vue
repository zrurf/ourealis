<script setup lang="ts">
/*
 * The whole run: route, individual, sensors and the simulator settings, validated
 * before anything is submitted.
 *
 * The form owns the draft state and the preset request; the views own what happens
 * on submit. Validation runs on the assembled state — not on the widget events —
 * so a state restored from a URL or a batch page is checked the same way.
 */
import { computed, onMounted, ref } from 'vue'
import { useI18n } from 'vue-i18n'
import {
  Alert as TAlert,
  Button as TButton,
  Input as TInput,
  InputNumber as TInputNumber,
  Select as TSelect,
  Switch as TSwitch,
} from 'tdesign-vue-next'
import { isApiError } from '@/api/errors'
import { listPresets, type Preset } from '@/api/presets'
import type { MapSummary } from '@/api/types'
import type { SimulationRequest } from '@/types/simulation'
import PersonForm from './PersonForm.vue'
import RouteForm from './RouteForm.vue'
import SensorForm from './SensorForm.vue'
import {
  buildSimulationRequest,
  defaultFormState,
  validateForm,
  type PickTarget,
  type SimulationFormState,
} from './request'

const props = withDefaults(
  defineProps<{
    /** Maps the run may be planned against. */
    maps: MapSummary[]
    /** Whether a map canvas on the page can place points. */
    pickable?: boolean
    /** Label of the submit button; the batch page starts a sweep instead of a run. */
    submitLabel?: string
    /** Whether a submission is in flight. */
    busy?: boolean
  }>(),
  { pickable: false, submitLabel: undefined, busy: false },
)

const emit = defineEmits<{
  /** A validated request the caller submits. */
  submit: [request: SimulationRequest, state: SimulationFormState]
  /** A request to place a field from the map surface. */
  pick: [target: PickTarget]
}>()

const { t } = useI18n({ useScope: 'global' })

const state = ref<SimulationFormState>(defaultFormState())
const presets = ref<Preset[]>([])
const presetStatus = ref<'idle' | 'loading' | 'ready' | 'failed'>('idle')
const presetError = ref<string | null>(null)
const submitted = ref(false)

/** Validation issues of the current state, shown once the user has tried to submit. */
const issues = computed(() => validateForm(state.value))

/** Whether the form can be submitted. */
const valid = computed(() => issues.value.length === 0)

const backendOptions = computed(() => [
  { value: 'default', label: t('simulation.form.serviceDefault') },
  { value: 'auto', label: t('simulation.form.backendAuto') },
  { value: 'cpu', label: t('simulation.form.backendCpu') },
  { value: 'gpu', label: t('simulation.form.backendGpu') },
])

const motionModeOptions = computed(() => [
  { value: 'default', label: t('simulation.form.serviceDefault') },
  { value: 'jog', label: t('simulation.form.modeJog') },
  { value: 'moderate', label: t('simulation.form.modeModerate') },
  { value: 'race', label: t('simulation.form.modeRace') },
])

/** Reads the presets the form is generated from. */
onMounted(async () => {
  presetStatus.value = 'loading'
  try {
    const page = await listPresets()
    presets.value = page.items
    if (page.items.length > 0 && page.items[0] !== undefined) {
      // A preset the service does not know would be a 400 on submit, so the first
      // reported one becomes the selection.
      const known = page.items.some((entry) => entry.preset === state.value.preset)
      if (!known) {
        state.value = { ...state.value, preset: page.items[0].preset }
      }
    }
    presetStatus.value = 'ready'
  } catch (error) {
    presetError.value = isApiError(error) ? error.message : String(error)
    presetStatus.value = 'failed'
  }
})

/** Validates and hands the assembled request to the caller. */
function submit(): void {
  if (props.busy) {
    // The form's submit event fires again on Enter while a POST is in flight, which
    // the button's loading state does not suppress.
    return
  }
  submitted.value = true
  if (!valid.value) {
    return
  }
  try {
    emit('submit', buildSimulationRequest(state.value), state.value)
  } catch {
    // `buildSimulationRequest` throws only for an incomplete route, which
    // `validateForm` has already reported.
    submitted.value = true
  }
}

/** Puts the form back to its initial state. */
function reset(): void {
  state.value = defaultFormState()
  submitted.value = false
}

/** Exposes the current draft to a caller that needs it without a submit. */
defineExpose({ state })
</script>

<template>
  <form class="flex flex-col gap-6" data-testid="simulation-form" @submit.prevent="submit()">
    <section>
      <h2 class="text-base font-medium text-ink">{{ t('simulation.form.title') }}</h2>
      <p class="mt-1 text-sm text-muted">{{ t('simulation.form.defaultsHint') }}</p>
      <div class="mt-3 grid grid-cols-4 gap-3">
        <label class="col-span-3">
          <span class="text-sm text-muted">{{ t('simulation.form.name') }}</span>
          <TInput
            :value="state.name"
            :placeholder="t('simulation.form.namePlaceholder')"
            data-testid="form-name"
            @change="(value) => (state = { ...state, name: String(value) })"
          />
        </label>
        <label>
          <span class="text-sm text-muted">{{ t('simulation.form.individual') }}</span>
          <TInputNumber
            :value="state.individual"
            :min="0"
            data-testid="form-individual"
            @change="(value) => (state = { ...state, individual: Number(value) })"
          />
          <span class="text-xs text-muted">{{ t('simulation.form.individualHint') }}</span>
        </label>
      </div>
    </section>

    <section>
      <h2 class="text-base font-medium text-ink">{{ t('simulation.form.groupRoute') }}</h2>
      <RouteForm
        v-model="state"
        class="mt-2"
        :maps="maps"
        :pickable="pickable"
        @pick="(target) => emit('pick', target)"
      />
    </section>

    <section>
      <h2 class="text-base font-medium text-ink">{{ t('simulation.form.groupPerson') }}</h2>
      <PersonForm v-model="state" class="mt-2" :presets="presets" :status="presetStatus" />
      <p v-if="presetError !== null" class="mt-2 text-sm text-danger">
        <span class="mr-2">{{ t('simulation.person.loadFailed') }}</span>
        <span class="text-muted">{{ t('simulation.live.fromService') }}: {{ presetError }}</span>
      </p>
    </section>

    <section>
      <h2 class="text-base font-medium text-ink">{{ t('simulation.form.groupSensors') }}</h2>
      <SensorForm v-model="state" class="mt-2" />
    </section>

    <section>
      <h2 class="text-base font-medium text-ink">{{ t('simulation.form.groupSettings') }}</h2>
      <div class="mt-2 grid grid-cols-4 items-end gap-3">
        <label>
          <span class="text-sm text-muted">{{ t('simulation.form.backend') }}</span>
          <TSelect
            :value="state.backend ?? 'default'"
            :options="backendOptions"
            data-testid="form-backend"
            @change="
              (value) =>
                (state = {
                  ...state,
                  backend: value === 'cpu' || value === 'gpu' || value === 'auto' ? value : null,
                })
            "
          />
        </label>
        <label>
          <span class="text-sm text-muted">{{ t('simulation.form.motionMode') }}</span>
          <TSelect
            :value="state.motionMode ?? 'default'"
            :options="motionModeOptions"
            data-testid="form-motion-mode"
            @change="
              (value) =>
                (state = {
                  ...state,
                  motionMode:
                    value === 'jog' || value === 'moderate' || value === 'race' ? value : null,
                })
            "
          />
        </label>
        <label class="flex items-center gap-2">
          <TSwitch
            :value="state.withMetrics"
            data-testid="form-with-metrics"
            @change="(value) => (state = { ...state, withMetrics: Boolean(value) })"
          />
          <span class="text-sm text-muted">{{ t('simulation.form.withMetrics') }}</span>
        </label>
        <label class="flex items-center gap-2">
          <TSwitch
            :value="state.smooth"
            data-testid="form-smooth"
            @change="(value) => (state = { ...state, smooth: Boolean(value) })"
          />
          <span class="text-sm text-muted">{{ t('simulation.form.smooth') }}</span>
        </label>
      </div>
    </section>

    <TAlert
      v-if="submitted && !valid"
      theme="error"
      :message="t('simulation.form.invalidTitle')"
      data-testid="form-issues"
    >
      <ul class="mt-1 list-inside list-disc text-sm">
        <li v-for="issue in issues" :key="`${issue.field}:${issue.key}`">
          {{ t(issue.key) }} — <span class="font-mono text-xs">{{ issue.field }}</span>
        </li>
      </ul>
    </TAlert>

    <div class="flex items-center gap-3">
      <TButton theme="primary" type="submit" :loading="busy" data-testid="form-submit">
        {{ submitLabel ?? t('simulation.form.submit') }}
      </TButton>
      <TButton variant="outline" data-testid="form-reset" @click="reset()">
        {{ t('simulation.form.reset') }}
      </TButton>
    </div>
  </form>
</template>
