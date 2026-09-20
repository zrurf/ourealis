<script setup lang="ts">
/*
 * The individual: a preset and its field-level overrides.
 *
 * The override controls are generated from `/presets` — `override_fields` names
 * the fields the service accepts and the preset's own `params` says what type each
 * one holds — so the form cannot drift from `PersonParams`: a field core adds
 * appears here as soon as the service reports it, and a field core removes
 * disappears rather than being sent and rejected.
 */
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'
import {
  Button as TButton,
  Input as TInput,
  InputNumber as TInputNumber,
  Select as TSelect,
  Tag as TTag,
} from 'tdesign-vue-next'
import type { Preset } from '@/api/presets'
import { overrideFields, type OverrideValue, type SimulationFormState } from './request'

const props = defineProps<{
  /** The form state this component edits; it emits a new one on every change. */
  modelValue: SimulationFormState
  /** Presets the service reported, with the fields each one accepts. */
  presets: Preset[]
  /** How far the preset request has come, so an empty list has an explanation. */
  status: 'idle' | 'loading' | 'ready' | 'failed'
}>()

const emit = defineEmits<{
  /** The form state after an edit. */
  'update:modelValue': [state: SimulationFormState]
}>()

const { t } = useI18n({ useScope: 'global' })

/** The selected preset, when the service reported it. */
const preset = computed<Preset | null>(
  () => props.presets.find((entry) => entry.preset === props.modelValue.preset) ?? null,
)

/** Controls to draw, one per override field the service accepts for this preset. */
const fields = computed(() =>
  overrideFields(preset.value?.override_fields ?? [], preset.value?.params),
)

/** Preset names as select options. */
const presetOptions = computed(() =>
  props.presets.map((entry) => ({ value: entry.preset, label: entry.preset })),
)

/** How many overrides currently carry a value. */
const overrideCount = computed(
  () => Object.values(props.modelValue.overrides).filter((value) => value !== null).length,
)

/** Applies a patch to the form state. */
function patch(changes: Partial<SimulationFormState>): void {
  emit('update:modelValue', { ...props.modelValue, ...changes })
}

/** Sets or clears one override; a cleared field is left to the service. */
function setOverride(name: string, value: OverrideValue): void {
  const overrides = { ...props.modelValue.overrides }
  if (value === null) {
    delete overrides[name]
  } else {
    overrides[name] = value
  }
  patch({ overrides })
}

/** Reads an override as a number, or `null` while it is unset. */
function numberValue(name: string): number | null {
  const value = props.modelValue.overrides[name]
  return typeof value === 'number' ? value : null
}

/** Reads an override as text. */
function textValue(name: string): string {
  const value = props.modelValue.overrides[name]
  return typeof value === 'string' ? value : ''
}

/** The choices of a fixed-choice field, or an empty list for the other kinds. */
function optionsFor(name: string): Array<{ value: string; label: string }> {
  const field = fields.value.find((entry) => entry.name === name)
  return (field?.options ?? []).map((option) => ({
    value: option.value,
    label: t(option.labelKey),
  }))
}
</script>

<template>
  <section data-testid="person-form">
    <div class="flex items-end gap-3">
      <label class="w-40">
        <span class="text-sm text-muted">{{ t('simulation.person.preset') }}</span>
        <TSelect
          :value="modelValue.preset"
          :options="presetOptions"
          :disabled="presets.length === 0"
          data-testid="person-preset"
          @change="(value) => patch({ preset: String(value) })"
        />
      </label>
      <div class="flex-1">
        <p class="text-sm text-muted">{{ t('simulation.person.overridesHint') }}</p>
        <div class="mt-1 flex items-center gap-2">
          <TTag size="small" variant="light" data-testid="person-override-count">
            {{ t('simulation.person.overrideCount', { count: overrideCount }) }}
          </TTag>
          <TButton
            v-if="overrideCount > 0"
            variant="text"
            data-testid="person-clear"
            @click="patch({ overrides: {} })"
          >
            {{ t('simulation.person.clear') }}
          </TButton>
        </div>
      </div>
    </div>

    <p v-if="status === 'failed'" class="mt-2 text-sm text-danger" data-testid="person-error">
      {{ t('simulation.person.loadFailed') }}
    </p>
    <p v-else-if="status === 'loading'" class="mt-2 text-sm text-muted">
      {{ t('common.loading') }}
    </p>
    <p v-else-if="fields.length === 0" class="mt-2 text-sm text-muted">
      {{ t('simulation.form.noPresets') }}
    </p>

    <div v-else class="mt-3 grid grid-cols-3 gap-3" data-testid="person-overrides">
      <label v-for="field in fields" :key="field.name" class="flex flex-col gap-1">
        <span class="font-mono text-xs text-muted">{{ field.name }}</span>
        <TInputNumber
          v-if="field.kind === 'number'"
          :value="numberValue(field.name) ?? undefined"
          :decimal-places="6"
          :allow-input-over-limit="true"
          :data-testid="`override-${field.name}`"
          @change="
            (value) => setOverride(field.name, typeof value === 'number' ? value : Number(value))
          "
        />
        <TSelect
          v-else-if="field.kind === 'choice'"
          :value="textValue(field.name)"
          :options="optionsFor(field.name)"
          :data-testid="`override-${field.name}`"
          @change="(value) => setOverride(field.name, String(value))"
        />
        <TInput
          v-else
          :value="textValue(field.name)"
          :data-testid="`override-${field.name}`"
          @change="(value) => setOverride(field.name, String(value))"
        />
      </label>
    </div>
  </section>
</template>
