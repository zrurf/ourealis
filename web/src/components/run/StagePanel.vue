<script setup lang="ts">
/*
 * The stage panels: the controls of whichever stage is open.
 *
 * Simple mode shows the decisions that change the result and hides the rest behind a
 * count of what has been changed from the recipe; expert mode shows the same form the
 * older pages did, grouped by meaning. The forms themselves are the existing
 * components — the point of the workspace is that the draft has one home, not that the
 * controls were rewritten.
 */
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'
import {
  Input as TInput,
  InputNumber as TInputNumber,
  Select as TSelect,
  Switch as TSwitch,
  Tag as TTag,
} from 'tdesign-vue-next'
import { useMapsStore } from '@/stores/maps'
import { useWorkspaceStore } from '@/stores/workspace'
import RouteForm from '@/components/forms/RouteForm.vue'
import PersonForm from '@/components/forms/PersonForm.vue'
import SensorForm from '@/components/forms/SensorForm.vue'
import { RECIPES } from '@/components/forms/recipes'
import { listPresets, type Preset } from '@/api/presets'
import { onMounted, ref } from 'vue'

const { t } = useI18n({ useScope: 'global' })
const workspace = useWorkspaceStore()
const maps = useMapsStore()

const presets = ref<Preset[]>([])
const presetStatus = ref<'idle' | 'loading' | 'ready' | 'failed'>('idle')

/** The run stages the panel can show. */
const stage = computed(() => workspace.stage)

/** Recipes as selectable cards. */
const recipeCards = computed(() =>
  RECIPES.map((recipe) => ({
    id: recipe.id,
    name: t(recipe.nameKey),
    description: t(recipe.descriptionKey),
    active: workspace.recipe === recipe.id,
  })),
)

onMounted(async () => {
  presetStatus.value = 'loading'
  try {
    presets.value = (await listPresets()).items
    presetStatus.value = 'ready'
  } catch {
    presetStatus.value = 'failed'
  }
})

/**
 * What is wrong with the route, in words.
 *
 * The map refuses a point for one of three reasons, and a reader who has just dropped
 * it needs to know which: a wall, a position off the map, or a spot too close to an
 * obstacle for a runner to pass.
 */
const refusalText = computed(() => {
  const refused = workspace.illegalPoints
  const first = workspace.pointChecks[refused[0]?.key ?? '']
  if (refused.length === 0 || first === undefined) {
    return null
  }
  return t('run.route.refused', {
    count: refused.length,
    reason: t(`run.route.reason.${first.reason}`, {
      distance: (first.distanceM ?? 0).toFixed(1),
    }),
  })
})

/** Motion modes whose cost weights the planner may use, as select options. */
const motionModes = computed(() => [
  { value: 'default', label: t('simulation.form.serviceDefault') },
  { value: 'jog', label: t('simulation.form.modeJog') },
  { value: 'moderate', label: t('simulation.form.modeModerate') },
  { value: 'race', label: t('simulation.form.modeRace') },
])

/** Compute backends the run may ask for. */
const backendOptions = computed(() => [
  { value: 'auto', label: t('simulation.form.backendAuto') },
  { value: 'cpu', label: t('simulation.form.backendCpu') },
  { value: 'gpu', label: t('simulation.form.backendGpu') },
])

/** Arms the pointer to place a field from the map. */
function arm(target: Parameters<typeof workspace.arm>[0]): void {
  workspace.arm(target)
}
</script>

<template>
  <div class="flex flex-col gap-4">
    <!-- ① Map -->
    <section v-if="stage === 'map'" class="flex flex-col gap-2" data-testid="stage-panel-map">
      <label class="flex flex-col gap-1">
        <span class="text-sm text-muted">{{ t('simulation.form.map') }}</span>
        <TSelect
          :value="workspace.draft.mapId ?? ''"
          :options="maps.summaries.map((map) => ({ value: map.id, label: map.name }))"
          :placeholder="t('simulation.form.mapAuto')"
          data-testid="run-map"
          @change="(value) => workspace.patch({ mapId: String(value) })"
        />
      </label>
      <p class="text-xs text-muted">{{ t('simulation.form.mapHint') }}</p>
    </section>

    <!-- ② Route -->
    <section
      v-else-if="stage === 'route'"
      class="flex flex-col gap-3"
      data-testid="stage-panel-route"
    >
      <div class="flex flex-wrap gap-2">
        <button
          v-for="card in recipeCards"
          :key="card.id"
          type="button"
          class="flex-1 rounded-control border px-2 py-2 text-left text-xs transition-colors"
          :class="
            card.active
              ? 'border-brand bg-surface text-ink'
              : 'border-line text-muted hover:text-ink'
          "
          :data-testid="`recipe-${card.id}`"
          @click="workspace.applyRecipe(card.id)"
        >
          <span class="block font-medium">{{ card.name }}</span>
          <span class="mt-0.5 block">{{ card.description }}</span>
        </button>
      </div>

      <!-- The mode decides how much of the route form is on screen, so the stage looks
           different in the two modes instead of looking identical. -->
      <RouteForm
        v-model="workspace.draft"
        :maps="maps.summaries"
        :simple="workspace.mode === 'simple'"
        pickable
        @pick="arm"
        @update:model-value="(next) => workspace.setDraft(next)"
      />

      <p class="text-xs text-muted" data-testid="route-hint">{{ t('run.route.hint') }}</p>
      <!-- A point the map refuses is said in words as well as drawn in the error
           colour: the colour says which point, the sentence says why. -->
      <p v-if="refusalText !== null" class="text-sm text-danger" data-testid="route-refused">
        {{ refusalText }}
      </p>
      <TTag
        v-if="workspace.armed !== null"
        size="small"
        variant="light"
        theme="warning"
        data-testid="route-armed"
      >
        {{ t('run.route.armed') }}
      </TTag>
    </section>

    <!-- ③ Runner -->
    <section
      v-else-if="stage === 'runner'"
      class="flex flex-col gap-3"
      data-testid="stage-panel-runner"
    >
      <div class="flex items-center justify-between gap-2">
        <span class="text-sm text-muted">{{ t('simulation.person.preset') }}</span>
        <TTag v-if="workspace.changedIn('runner') > 0" size="small" variant="light">
          {{ t('run.changed', { count: workspace.changedIn('runner') }) }}
        </TTag>
      </div>
      <PersonForm
        v-model="workspace.draft"
        :presets="presets"
        :status="presetStatus"
        :simple="workspace.mode === 'simple'"
        @update:model-value="(next) => workspace.setDraft(next)"
      />
      <!-- One control per row: two number fields side by side in a 26 rem panel is how the
           earlier layout ended up overlapping and running off the right edge. -->
      <div class="flex flex-col gap-2">
        <label class="flex flex-col gap-1">
          <span class="text-xs text-muted">{{ t('simulation.form.individual') }}</span>
          <TInputNumber
            :value="workspace.draft.individual"
            :min="0"
            class="w-full"
            data-testid="run-individual"
            @change="(value) => workspace.patch({ individual: Number(value) })"
          />
        </label>
        <label class="flex flex-col gap-1">
          <span class="text-xs text-muted">{{ t('simulation.form.seed') }}</span>
          <TInputNumber
            :value="workspace.draft.seed"
            :min="0"
            class="w-full"
            data-testid="run-seed"
            @change="(value) => workspace.patch({ seed: Number(value) })"
          />
        </label>
      </div>
    </section>

    <!-- ④ Sensors -->
    <section
      v-else-if="stage === 'sensors'"
      class="flex flex-col gap-3"
      data-testid="stage-panel-sensors"
    >
      <div class="flex items-center justify-between gap-2">
        <span class="text-sm text-muted">{{ t('simulation.form.groupSensors') }}</span>
        <TTag v-if="workspace.changedIn('sensors') > 0" size="small" variant="light">
          {{ t('run.changed', { count: workspace.changedIn('sensors') }) }}
        </TTag>
      </div>
      <SensorForm
        v-model="workspace.draft"
        :simple="workspace.mode === 'simple'"
        @update:model-value="(next) => workspace.setDraft(next)"
      />
    </section>

    <!-- ⑤ Run -->
    <section v-else class="flex flex-col gap-3" data-testid="stage-panel-run">
      <label class="flex flex-col gap-1">
        <span class="text-sm text-muted">{{ t('simulation.form.name') }}</span>
        <TInput
          :value="workspace.draft.name"
          :placeholder="t('simulation.form.namePlaceholder')"
          data-testid="run-name"
          @change="(value) => workspace.patch({ name: String(value) })"
        />
      </label>
      <div class="flex flex-col gap-2 text-sm">
        <label class="flex items-center gap-2">
          <TSwitch
            :value="workspace.draft.withMetrics"
            data-testid="run-with-metrics"
            @change="(value) => workspace.patch({ withMetrics: Boolean(value) })"
          />
          <span class="text-muted">{{ t('simulation.form.withMetrics') }}</span>
        </label>
        <label class="flex items-center gap-2">
          <TSwitch
            :value="workspace.draft.smooth"
            data-testid="run-smooth"
            @change="(value) => workspace.patch({ smooth: Boolean(value) })"
          />
          <span class="text-muted">{{ t('simulation.form.smooth') }}</span>
        </label>
      </div>
      <template v-if="workspace.mode === 'expert'">
        <label class="flex flex-col gap-1">
          <span class="text-xs text-muted">{{ t('simulation.form.backend') }}</span>
          <TSelect
            :value="workspace.draft.backend ?? 'auto'"
            :options="backendOptions"
            data-testid="run-backend"
            @change="
              (value) =>
                workspace.patch({
                  backend:
                    value === 'cpu' || value === 'gpu' ? value : value === 'auto' ? 'auto' : null,
                })
            "
          />
        </label>
        <label class="flex flex-col gap-1">
          <span class="text-xs text-muted">{{ t('simulation.form.motionMode') }}</span>
          <TSelect
            :value="workspace.draft.motionMode ?? 'default'"
            :options="motionModes"
            data-testid="run-motion-mode"
            @change="
              (value) =>
                workspace.patch({
                  motionMode:
                    value === 'jog' || value === 'moderate' || value === 'race' ? value : null,
                })
            "
          />
        </label>
      </template>

      <ul
        v-if="!workspace.runnable"
        class="flex flex-col gap-1 text-sm text-danger"
        data-testid="run-issues"
      >
        <li v-for="issue in workspace.issues" :key="`${issue.field}:${issue.key}`">
          {{ t(issue.key) }}
        </li>
      </ul>
    </section>
  </div>
</template>
