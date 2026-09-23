<script setup lang="ts">
/*
 * The plan: what the planner found, and what it costs.
 *
 * One panel replaces the two buttons and the two tables the studio had. It reports the
 * measured planning time (a debug service takes tens of seconds on a large map, and a
 * reader is owed that number rather than a spinner), lists the candidates with the
 * planner's own choice marked, and carries the summary strip.
 *
 * The strip is the point of the whole workspace: distance, estimated time and the path
 * ratio update as the route changes, so a change's consequence is visible without
 * running anything.
 */
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'
import { Button as TButton, Tag as TTag } from 'tdesign-vue-next'
import { useWorkspaceStore } from '@/stores/workspace'

const { t, locale } = useI18n({ useScope: 'global' })
const workspace = useWorkspaceStore()

/** Number in the interface locale. */
function number(value: number, digits = 0): string {
  return new Intl.NumberFormat(locale.value, { maximumFractionDigits: digits }).format(value)
}

/** Duration as `m:ss`, or `—` while the profile is not in yet. */
const duration = computed(() => {
  const seconds = workspace.summary?.durationS ?? null
  if (seconds === null) {
    return '—'
  }
  const whole = Math.round(seconds)
  return `${Math.floor(whole / 60)}:${String(whole % 60).padStart(2, '0')}`
})

/** Rows of the candidate table. */
const rows = computed(() =>
  (workspace.plan.preview?.candidates ?? []).map((candidate, index) => ({
    index,
    key: String(index),
    length: `${number(candidate.length_m, 0)} m`,
    cost: `${number(candidate.cost_equiv_m, 0)}`,
    probability: `${number(candidate.probability * 100, 1)} %`,
    chosen: index === workspace.plan.chosen,
    inspected: index === workspace.plan.inspected,
  })),
)
</script>

<template>
  <section class="flex flex-col gap-3" data-testid="plan-panel">
    <header class="flex items-center justify-between gap-2">
      <h2 class="text-sm font-medium text-ink">{{ t('run.plan.title') }}</h2>
      <div class="flex items-center gap-2">
        <TTag v-if="workspace.plan.busy" size="small" variant="light" data-testid="plan-busy">
          {{ t('run.plan.planning') }}
        </TTag>
        <TTag
          v-else-if="workspace.plan.durationMs !== null"
          size="small"
          variant="light"
          theme="success"
          data-testid="plan-status"
        >
          {{ t('run.plan.took', { ms: number(workspace.plan.durationMs, 0) }) }}
        </TTag>
        <TButton
          v-if="workspace.plan.busy"
          size="small"
          variant="outline"
          data-testid="plan-cancel"
          @click="workspace.cancelPlan()"
        >
          {{ t('common.cancel') }}
        </TButton>
      </div>
    </header>

    <p v-if="workspace.plan.error !== null" class="text-sm text-danger" data-testid="plan-error">
      {{ workspace.plan.error }}
    </p>

    <dl
      v-if="workspace.summary !== null"
      class="grid grid-cols-2 gap-x-3 gap-y-1 text-sm"
      data-testid="plan-summary"
    >
      <div class="flex justify-between gap-2">
        <dt class="text-muted">{{ t('run.plan.length') }}</dt>
        <dd class="font-mono text-ink" data-testid="summary-length">
          {{ number(workspace.summary.lengthM, 0) }} m
        </dd>
      </div>
      <div class="flex justify-between gap-2">
        <dt class="text-muted">{{ t('run.plan.duration') }}</dt>
        <dd class="font-mono text-ink" data-testid="summary-duration">{{ duration }}</dd>
      </div>
      <div class="flex justify-between gap-2">
        <dt class="text-muted">{{ t('run.plan.pathRatio') }}</dt>
        <dd class="font-mono text-ink">{{ number(workspace.summary.pathRatio, 2) }}</dd>
      </div>
      <div class="flex justify-between gap-2">
        <dt class="text-muted">{{ t('run.plan.candidates') }}</dt>
        <dd class="font-mono text-ink">{{ workspace.summary.candidates }}</dd>
      </div>
    </dl>

    <p v-else-if="!workspace.plan.busy" class="text-sm text-muted" data-testid="plan-empty">
      {{ t('run.plan.empty') }}
    </p>

    <ul v-if="rows.length > 0" class="flex flex-col gap-1">
      <li
        v-for="row in rows"
        :key="row.key"
        class="flex cursor-pointer items-center gap-2 rounded-control px-2 py-1 text-sm"
        :class="row.inspected ? 'bg-surface text-ink' : 'text-muted'"
        :data-testid="`candidate-${row.index}`"
        @click="workspace.inspectCandidate(row.index)"
      >
        <span class="w-4 font-mono">{{ row.index }}</span>
        <span class="flex-1 font-mono">{{ row.length }}</span>
        <span class="w-16 text-right font-mono">{{ row.cost }}</span>
        <span class="w-16 text-right font-mono">{{ row.probability }}</span>
        <TTag v-if="row.chosen" size="small" variant="light" theme="success">
          {{ t('run.plan.chosen') }}
        </TTag>
      </li>
    </ul>

    <p v-if="rows.length > 0" class="text-xs text-muted">{{ t('run.plan.inspectHint') }}</p>
  </section>
</template>
