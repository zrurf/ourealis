<script setup lang="ts">
/*
 * The individual: a preset and its field-level overrides.
 *
 * The override controls are generated from `/presets` — `override_fields` names
 * the fields the service accepts and the preset's own `params` says what type each
 * one holds — so the form cannot drift from `PersonParams`: a field core adds
 * appears here as soon as the service reports it, and a field core removes
 * disappears rather than being sent and rejected.
 *
 * Every accepted field is reachable. They are grouped by what the knob is about and
 * folded away by default, because a reader who wants the preset should not scroll
 * past thirty fields to reach the run button — but a knob that is merely *offered*
 * behind a mode switch is a knob nobody finds, so nothing here is unreachable and
 * the count of what is set is always on screen.
 */
import { computed, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import {
  Button as TButton,
  Collapse as TCollapse,
  CollapsePanel as TCollapsePanel,
  Input as TInput,
  InputNumber as TInputNumber,
  Select as TSelect,
  Switch as TSwitch,
  Tag as TTag,
} from 'tdesign-vue-next'
import type { Preset } from '@/api/presets'
import {
  groupOf,
  numberOrNull,
  overrideGroups,
  SENSOR_GROUP,
  type OverrideField,
  type OverrideValue,
  type SimulationFormState,
} from './request'

const props = defineProps<{
  /** The form state this component edits; it emits a new one on every change. */
  modelValue: SimulationFormState
  /** Presets the service reported, with the fields each one accepts. */
  presets: Preset[]
  /** How far the preset request has come, so an empty list has an explanation. */
  status: 'idle' | 'loading' | 'ready' | 'failed'
  /**
   * Whether the item groups start open.
   *
   * This only decides where they start; it never decides whether they are reachable.
   */
  simple?: boolean
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

/** The controls to draw, grouped by what each field is about. */
const groups = computed<OverrideField[][]>(() =>
  overrideGroups(preset.value?.override_fields ?? [], preset.value?.params),
)

/**
 * Which groups are open.
 *
 * The simple mode opens the groups that already carry an override and folds the rest, so
 * a reader returning to a draft sees what they changed without hunting for it, while a
 * reader starting from a preset is not walked past thirty fields. Expert mode opens
 * everything. Either way every field is one click away — a knob hidden behind a mode
 * switch is a knob nobody finds.
 */
const expanded = ref<string[]>([])

function initialExpansion(): string[] {
  if (props.simple !== true) {
    return groups.value.map((_, index) => String(index))
  }
  const overrides = props.modelValue.overrides
  return groups.value
    .map((group, index) => ({ group, index }))
    .filter(({ group }) => group.some((field) => overrides[field.name] != null))
    .map(({ index }) => String(index))
}

watch(
  [() => props.simple, groups, () => props.modelValue.overrides],
  () => {
    expanded.value = initialExpansion()
  },
  { immediate: true },
)

/** Preset names as select options. */
const presetOptions = computed(() =>
  props.presets.map((entry) => ({ value: entry.preset, label: entry.preset })),
)

/**
 * How many overrides currently carry a value.
 *
 * A cleared numeric input is not a value: `TInputNumber` reports `undefined` while the
 * field is empty, and a draft that counted it would keep claiming an override that is
 * not going to be sent.
 */
const overrideCount = computed(
  () =>
    Object.values(props.modelValue.overrides).filter(
      (value) => value !== null && !(typeof value === 'number' && !Number.isFinite(value)),
    ).length,
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
  return typeof value === 'number' && Number.isFinite(value) ? value : null
}

/** Reads an override as text. */
function textValue(name: string): string {
  const value = props.modelValue.overrides[name]
  return typeof value === 'string' ? value : value === null ? '' : String(value)
}

/** Whether a field is one of the boolean sensor-noise flags, which want a switch. */
function isSwitch(group: readonly OverrideField[], name: string): boolean {
  const field = group.find((entry) => entry.name === name)
  return (field?.options?.length ?? 0) === 2
}

/** The choices of a fixed-choice field, or an empty list for the other kinds. */
function optionsFor(
  group: readonly OverrideField[],
  name: string,
): Array<{ value: string; label: string }> {
  const field = group.find((entry) => entry.name === name)
  return (field?.options ?? []).map((option) => ({
    value: option.value,
    label: t(option.labelKey),
  }))
}

/** A sensor-noise field's short name; the group prefix is carried by the heading. */
function shortName(name: string): string {
  return name.startsWith(`${SENSOR_GROUP}.`) ? name.slice(SENSOR_GROUP.length + 1) : name
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
    <p v-else-if="groups.length === 0" class="mt-2 text-sm text-muted">
      {{ t('simulation.form.noPresets') }}
    </p>

    <TCollapse v-else v-model="expanded" class="mt-3" borderless data-testid="person-overrides">
      <TCollapsePanel
        v-for="(group, index) in groups"
        :key="index"
        :value="String(index)"
        :header="t(`simulation.person.group.${groupOf(group[0]?.name ?? '')}`)"
      >
        <!--
          One control per row. A grid of three put each input in a track narrower than
          the 144px tdesign gives a number field, so the stepper buttons pushed the
          value out of sight and the labels of neighbouring fields overlapped.
        -->
        <div class="flex flex-col gap-2">
          <label v-for="field in group" :key="field.name" class="flex w-full flex-col gap-1">
            <span class="font-mono text-xs text-muted">{{ shortName(field.name) }}</span>

            <TSwitch
              v-if="field.kind === 'choice' && isSwitch(group, field.name)"
              :value="textValue(field.name) === 'true'"
              :data-testid="`override-${field.name}`"
              @change="(value) => setOverride(field.name, value ? 'true' : 'false')"
            />

            <TInputNumber
              v-else-if="field.kind === 'number'"
              class="w-full"
              :value="numberValue(field.name) ?? undefined"
              :decimal-places="6"
              :allow-input-over-limit="true"
              :data-testid="`override-${field.name}`"
              @change="(value) => setOverride(field.name, numberOrNull(value))"
            />

            <TSelect
              v-else-if="field.kind === 'choice'"
              class="w-full"
              :value="textValue(field.name)"
              :options="optionsFor(group, field.name)"
              :data-testid="`override-${field.name}`"
              @change="(value) => setOverride(field.name, String(value))"
            />

            <TInput
              v-else
              class="w-full"
              :value="textValue(field.name)"
              :data-testid="`override-${field.name}`"
              @change="(value) => setOverride(field.name, String(value))"
            />
          </label>
        </div>
      </TCollapsePanel>
    </TCollapse>
  </section>
</template>
