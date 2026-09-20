<script setup lang="ts">
/*
 * The route: mode, points, waypoints with their passing semantics, checkpoints.
 *
 * The component is pure form: it edits the draft, and a "pick on map" button only
 * reports which field the map should fill. That keeps the canvas in the views that
 * own one (the route studio) while the same form serves the plain submission page.
 */
import { computed, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import { Button as TButton, InputNumber as TInputNumber, Select as TSelect } from 'tdesign-vue-next'
import type { MapSummary } from '@/api/types'
import {
  newCheckpoint,
  newWaypoint,
  setPointCoordinate,
  type CheckpointDraft,
  type PickTarget,
  type SemanticsKind,
  type SimulationFormState,
  type WaypointDraft,
} from './request'

const props = defineProps<{
  /** The form state this component edits. */
  modelValue: SimulationFormState
  /** Maps the run may be planned against. */
  maps: MapSummary[]
  /** Whether a map canvas is on the page, which is what the pick buttons need. */
  pickable?: boolean
}>()

const emit = defineEmits<{
  /** The form state after an edit. */
  'update:modelValue': [state: SimulationFormState]
  /** A request to place a field from the map surface. */
  pick: [target: PickTarget]
}>()

const { t } = useI18n({ useScope: 'global' })

const modeOptions = computed(() => [
  { value: 'standard', label: t('simulation.mode.standard') },
  { value: 'loop', label: t('simulation.mode.loop') },
  { value: 'dynamic', label: t('simulation.mode.dynamic') },
])

/**
 * Maps the run may be planned against.
 *
 * The first map of the library is preselected and shown by name, because the
 * service only accepts a request that names no map while the library holds exactly
 * one — a form that submitted an unnamed map would collect a 400 from a service a
 * page away. The select is filterable so a long library is usable.
 */
const mapOptions = computed(() => props.maps.map((map) => ({ value: map.id, label: map.name })))

/** Preselects a map as soon as the library is known, once. */
watch(
  () => [props.maps.length, props.modelValue.mapId] as const,
  ([count, current]) => {
    if (current === null && count > 0) {
      const first = props.maps[0]
      if (first !== undefined) {
        patch({ mapId: first.id })
      }
    }
  },
  { immediate: true },
)

const semanticsOptions = computed(() => [
  { value: 'pass', label: t('simulation.route.semanticsPass') },
  { value: 'slow', label: t('simulation.route.semanticsSlow') },
  { value: 'dwell', label: t('simulation.route.semanticsDwell') },
])

/** Applies a patch to the form state. */
function patch(changes: Partial<SimulationFormState>): void {
  emit('update:modelValue', { ...props.modelValue, ...changes })
}

/** Replaces one point of the state. */
function setPoint(
  field: 'start' | 'goal' | 'reference',
  axis: 'x' | 'y',
  value: number | null,
): void {
  patch({
    [field]: setPointCoordinate(props.modelValue[field], axis, value),
  } as Partial<SimulationFormState>)
}

/** Replaces one waypoint, leaving the others in place. */
function setWaypoint(index: number, changes: Partial<WaypointDraft>): void {
  const waypoints = props.modelValue.waypoints.map((waypoint, position) =>
    position === index ? { ...waypoint, ...changes } : waypoint,
  )
  patch({ waypoints })
}

/** Replaces one waypoint's position coordinate. */
function setWaypointCoordinate(index: number, axis: 'x' | 'y', value: number | null): void {
  const waypoint = props.modelValue.waypoints[index]
  if (waypoint === undefined) {
    return
  }
  setWaypoint(index, { position: setPointCoordinate(waypoint.position, axis, value) })
}

/** Appends a waypoint, or removes the one at `index` when the second argument is given. */
function mutateWaypoints(index?: number): void {
  if (index === undefined) {
    patch({ waypoints: [...props.modelValue.waypoints, newWaypoint()] })
    return
  }
  patch({ waypoints: props.modelValue.waypoints.filter((_, position) => position !== index) })
}

/** Replaces one checkpoint. */
function setCheckpoint(index: number, changes: Partial<CheckpointDraft>): void {
  const checkpoints = props.modelValue.checkpoints.map((checkpoint, position) =>
    position === index ? { ...checkpoint, ...changes } : checkpoint,
  )
  patch({ checkpoints })
}

/** Replaces one checkpoint's position coordinate. */
function setCheckpointCoordinate(index: number, axis: 'x' | 'y', value: number | null): void {
  const checkpoint = props.modelValue.checkpoints[index]
  if (checkpoint === undefined) {
    return
  }
  setCheckpoint(index, { position: setPointCoordinate(checkpoint.position, axis, value) })
}

/** Appends a checkpoint, or removes the one at `index` when given. */
function mutateCheckpoints(index?: number): void {
  if (index === undefined) {
    patch({ checkpoints: [...props.modelValue.checkpoints, newCheckpoint()] })
    return
  }
  patch({ checkpoints: props.modelValue.checkpoints.filter((_, position) => position !== index) })
}

/** Reports a pick request for one field. */
function requestPick(target: PickTarget): void {
  emit('pick', target)
}
</script>

<template>
  <section data-testid="route-form">
    <div class="grid grid-cols-4 gap-3">
      <label>
        <span class="text-sm text-muted">{{ t('simulation.route.mode') }}</span>
        <TSelect
          :value="modelValue.mode"
          :options="modeOptions"
          data-testid="route-mode"
          @change="(value) => patch({ mode: String(value) as SimulationFormState['mode'] })"
        />
      </label>
      <label class="col-span-2">
        <span class="text-sm text-muted">{{ t('simulation.form.map') }}</span>
        <TSelect
          :value="modelValue.mapId ?? ''"
          :options="mapOptions"
          :disabled="maps.length === 0"
          filterable
          :placeholder="t('simulation.form.map')"
          data-testid="route-map"
          @change="(value) => patch({ mapId: String(value) === '' ? null : String(value) })"
        />
        <span class="text-xs text-muted">{{ t('simulation.form.mapHint') }}</span>
      </label>
      <label>
        <span class="text-sm text-muted">{{ t('simulation.form.seed') }}</span>
        <TInputNumber
          :value="modelValue.seed"
          :min="0"
          data-testid="route-seed"
          @change="(value) => patch({ seed: Number(value) })"
        />
      </label>
    </div>

    <div class="mt-3 grid grid-cols-2 gap-3">
      <div class="rounded-card border border-line px-3 py-2">
        <div class="flex items-center justify-between">
          <span class="text-sm font-medium text-ink">{{ t('simulation.route.start') }}</span>
          <TButton
            v-if="pickable === true"
            variant="text"
            data-testid="pick-start"
            @click="requestPick({ kind: 'start' })"
          >
            {{ t('simulation.route.pick') }}
          </TButton>
        </div>
        <div class="mt-1 flex gap-2">
          <TInputNumber
            :value="modelValue.start.x ?? undefined"
            :decimal-places="3"
            :placeholder="t('simulation.route.x')"
            data-testid="start-x"
            @change="(value) => setPoint('start', 'x', Number(value))"
          />
          <TInputNumber
            :value="modelValue.start.y ?? undefined"
            :decimal-places="3"
            :placeholder="t('simulation.route.y')"
            data-testid="start-y"
            @change="(value) => setPoint('start', 'y', Number(value))"
          />
        </div>
      </div>

      <div v-if="modelValue.mode !== 'loop'" class="rounded-card border border-line px-3 py-2">
        <div class="flex items-center justify-between">
          <span class="text-sm font-medium text-ink">{{ t('simulation.route.goal') }}</span>
          <TButton
            v-if="pickable === true"
            variant="text"
            data-testid="pick-goal"
            @click="requestPick({ kind: 'goal' })"
          >
            {{ t('simulation.route.pick') }}
          </TButton>
        </div>
        <div class="mt-1 flex gap-2">
          <TInputNumber
            :value="modelValue.goal.x ?? undefined"
            :decimal-places="3"
            :placeholder="t('simulation.route.x')"
            data-testid="goal-x"
            @change="(value) => setPoint('goal', 'x', Number(value))"
          />
          <TInputNumber
            :value="modelValue.goal.y ?? undefined"
            :decimal-places="3"
            :placeholder="t('simulation.route.y')"
            data-testid="goal-y"
            @change="(value) => setPoint('goal', 'y', Number(value))"
          />
        </div>
      </div>

      <div v-else class="rounded-card border border-line px-3 py-2">
        <div class="flex items-center justify-between">
          <span class="text-sm font-medium text-ink">{{ t('simulation.route.reference') }}</span>
          <div class="flex items-center gap-2">
            <span class="text-xs text-muted">{{ t('simulation.route.referenceHint') }}</span>
            <TButton
              v-if="pickable === true"
              variant="text"
              data-testid="pick-reference"
              @click="requestPick({ kind: 'reference' })"
            >
              {{ t('simulation.route.pick') }}
            </TButton>
          </div>
        </div>
        <div class="mt-1 flex items-end gap-2">
          <TInputNumber
            :value="modelValue.reference.x ?? undefined"
            :decimal-places="3"
            :placeholder="t('simulation.route.x')"
            @change="(value) => setPoint('reference', 'x', Number(value))"
          />
          <TInputNumber
            :value="modelValue.reference.y ?? undefined"
            :decimal-places="3"
            :placeholder="t('simulation.route.y')"
            @change="(value) => setPoint('reference', 'y', Number(value))"
          />
          <label class="w-32">
            <span class="text-sm text-muted">{{ t('simulation.route.laps') }}</span>
            <TInputNumber
              :value="modelValue.laps"
              :min="1"
              data-testid="route-laps"
              @change="(value) => patch({ laps: Number(value) })"
            />
          </label>
        </div>
      </div>
    </div>

    <div v-if="modelValue.mode === 'standard'" class="mt-4" data-testid="waypoint-list">
      <div class="flex items-center justify-between">
        <span class="text-sm font-medium text-ink">{{ t('simulation.route.waypoints') }}</span>
        <TButton
          variant="outline"
          size="small"
          data-testid="add-waypoint"
          @click="mutateWaypoints()"
        >
          {{ t('simulation.route.addWaypoint') }}
        </TButton>
      </div>
      <p v-if="modelValue.waypoints.length === 0" class="mt-1 text-sm text-muted">
        {{ t('common.empty') }}
      </p>
      <div
        v-for="(waypoint, index) in modelValue.waypoints"
        :key="index"
        class="mt-2 grid grid-cols-12 items-end gap-2 rounded-card border border-line px-3 py-2"
        :data-testid="`waypoint-${index}`"
      >
        <span class="col-span-2 text-sm text-ink">
          {{ t('simulation.route.waypoint', { index: index + 1 }) }}
        </span>
        <label class="col-span-3">
          <span class="text-xs text-muted">{{ t('simulation.route.x') }}</span>
          <TInputNumber
            :value="waypoint.position.x ?? undefined"
            :decimal-places="3"
            @change="(value) => setWaypointCoordinate(index, 'x', Number(value))"
          />
        </label>
        <label class="col-span-3">
          <span class="text-xs text-muted">{{ t('simulation.route.y') }}</span>
          <TInputNumber
            :value="waypoint.position.y ?? undefined"
            :decimal-places="3"
            @change="(value) => setWaypointCoordinate(index, 'y', Number(value))"
          />
        </label>
        <label class="col-span-2">
          <span class="text-xs text-muted">{{ t('simulation.route.semantics') }}</span>
          <TSelect
            :value="waypoint.semantics"
            :options="semanticsOptions"
            size="small"
            @change="(value) => setWaypoint(index, { semantics: String(value) as SemanticsKind })"
          />
        </label>
        <label v-if="waypoint.semantics === 'dwell'" class="col-span-1">
          <span class="text-xs text-muted">{{ t('simulation.route.duration') }}</span>
          <TInputNumber
            :value="waypoint.duration_s"
            :min="0"
            size="small"
            @change="(value) => setWaypoint(index, { duration_s: Number(value) })"
          />
        </label>
        <label v-else-if="waypoint.semantics === 'slow'" class="col-span-1">
          <span class="text-xs text-muted">{{ t('simulation.route.radius') }}</span>
          <TInputNumber
            :value="waypoint.radius_m"
            :min="0"
            size="small"
            @change="(value) => setWaypoint(index, { radius_m: Number(value) })"
          />
        </label>
        <div class="col-span-1 flex items-center justify-end gap-1">
          <TButton
            v-if="pickable === true"
            variant="text"
            size="small"
            @click="requestPick({ kind: 'waypoint', index })"
          >
            {{ t('simulation.route.pick') }}
          </TButton>
          <TButton variant="text" size="small" @click="mutateWaypoints(index)">
            {{ t('simulation.route.remove') }}
          </TButton>
        </div>
      </div>
    </div>

    <div v-if="modelValue.mode === 'dynamic'" class="mt-4" data-testid="checkpoint-list">
      <div class="flex items-center justify-between">
        <span class="text-sm font-medium text-ink">{{ t('simulation.route.checkpoints') }}</span>
        <TButton
          variant="outline"
          size="small"
          data-testid="add-checkpoint"
          @click="mutateCheckpoints()"
        >
          {{ t('simulation.route.addCheckpoint') }}
        </TButton>
      </div>
      <p v-if="modelValue.checkpoints.length === 0" class="mt-1 text-sm text-muted">
        {{ t('common.empty') }}
      </p>
      <div
        v-for="(checkpoint, index) in modelValue.checkpoints"
        :key="index"
        class="mt-2 flex items-end gap-2 rounded-card border border-line px-3 py-2"
        :data-testid="`checkpoint-${index}`"
      >
        <span class="w-32 text-sm text-ink">
          {{ t('simulation.route.checkpoint', { index: index + 1 }) }}
        </span>
        <TInputNumber
          :value="checkpoint.position.x ?? undefined"
          :decimal-places="3"
          :placeholder="t('simulation.route.x')"
          @change="(value) => setCheckpointCoordinate(index, 'x', Number(value))"
        />
        <TInputNumber
          :value="checkpoint.position.y ?? undefined"
          :decimal-places="3"
          :placeholder="t('simulation.route.y')"
          @change="(value) => setCheckpointCoordinate(index, 'y', Number(value))"
        />
        <label class="flex-1">
          <span class="text-xs text-muted">{{ t('simulation.route.issuedAt') }}</span>
          <TInputNumber
            :value="checkpoint.issued_at_s"
            :min="0"
            @change="(value) => setCheckpoint(index, { issued_at_s: Number(value) })"
          />
        </label>
        <TButton
          v-if="pickable === true"
          variant="text"
          size="small"
          @click="requestPick({ kind: 'checkpoint', index })"
        >
          {{ t('simulation.route.pick') }}
        </TButton>
        <TButton variant="text" size="small" @click="mutateCheckpoints(index)">
          {{ t('simulation.route.remove') }}
        </TButton>
      </div>
    </div>
  </section>
</template>
