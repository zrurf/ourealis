<script setup lang="ts">
/*
 * The stage tabs.
 *
 * Five stages, always visible, with the one in progress marked and the ones whose
 * prerequisites are missing greyed out and explained. It is the answer to "what is left
 * before I can run": previously that was spread across a form, a button's disabled state
 * and a validation list that appeared only after a failed submit.
 *
 * Horizontal, along the top of the panel that follows it: the stages are a progress
 * through one task rather than a second navigation column, and a column of five labels
 * on the left cost the map a fifth of its width to say what one row says.
 */
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'
import { useWorkspaceStore, type Stage } from '@/stores/workspace'
import { pointToVec } from '@/components/forms/request'

const { t } = useI18n({ useScope: 'global' })
const workspace = useWorkspaceStore()

/** Stages in order, with the catalog key of each short label. */
const STAGES: ReadonlyArray<{ id: Stage; labelKey: string }> = [
  { id: 'map', labelKey: 'run.stage.map' },
  { id: 'route', labelKey: 'run.stage.route' },
  { id: 'runner', labelKey: 'run.stage.runner' },
  { id: 'sensors', labelKey: 'run.stage.sensors' },
  { id: 'run', labelKey: 'run.stage.run' },
]

/** Why a stage cannot be used yet, or `null` when it can. */
function blockedBy(id: Stage): string | null {
  const draft = workspace.draft
  if (id === 'map' && draft.mapId === null && workspace.mapId === null) {
    return t('run.blocked.noMap')
  }
  if ((id === 'runner' || id === 'sensors' || id === 'run') && pointToVec(draft.start) === null) {
    return t('run.blocked.noStart')
  }
  if (id === 'run' && draft.mode !== 'loop' && pointToVec(draft.goal) === null) {
    return t('run.blocked.noGoal')
  }
  return null
}

/** How far the draft is from being runnable, for the progress line. */
const progress = computed(() => {
  const steps = STAGES.filter((stage) => blockedBy(stage.id) === null).length
  return Math.round((steps / STAGES.length) * 100)
})

/** Moves the selection along the tabs, which is what arrow keys do on a tab list. */
function step(delta: number): void {
  const index = STAGES.findIndex((entry) => entry.id === workspace.stage)
  const next = STAGES[(index + delta + STAGES.length) % STAGES.length]
  if (next !== undefined && blockedBy(next.id) === null) {
    workspace.stage = next.id
  }
}
</script>

<template>
  <div
    class="flex flex-col gap-2"
    data-testid="stage-rail"
    role="tablist"
    :aria-label="t('run.stages')"
    @keydown.left.prevent="step(-1)"
    @keydown.right.prevent="step(1)"
  >
    <div class="flex flex-wrap items-center gap-1">
      <button
        v-for="entry in STAGES"
        :key="entry.id"
        type="button"
        role="tab"
        class="flex items-center gap-1.5 rounded-control px-2 py-1 text-sm transition-colors"
        :class="
          workspace.stage === entry.id
            ? 'bg-surface font-medium text-ink'
            : 'text-muted hover:text-ink'
        "
        :aria-selected="workspace.stage === entry.id"
        :disabled="blockedBy(entry.id) !== null"
        :title="blockedBy(entry.id) ?? ''"
        :data-testid="`stage-${entry.id}`"
        @click="workspace.stage = entry.id"
      >
        <span
          class="inline-block h-1.5 w-1.5 flex-none rounded-full"
          :class="
            blockedBy(entry.id) !== null
              ? 'bg-line'
              : workspace.stage === entry.id
                ? 'bg-brand'
                : 'bg-success'
          "
          aria-hidden="true"
        />
        {{ t(entry.labelKey) }}
      </button>
    </div>

    <div :aria-label="t('run.progress')">
      <div class="h-1 w-full overflow-hidden rounded-full bg-line">
        <div class="h-full bg-brand" :style="{ width: `${progress}%` }" />
      </div>
    </div>
  </div>
</template>
