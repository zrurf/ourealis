<script setup lang="ts">
/*
 * The blocking loader.
 *
 * Shown while a task the interface cannot continue without is running — a map being
 * generated, a plan the next step needs. It is deliberately not a progress bar: the
 * service's task body is a single call, so there is no fraction to show and a bar
 * would be an invention. What it shows instead is what is measurable — the operation's
 * name, the elapsed time the service reports, the log lines it publishes, and a
 * cancel button that works.
 *
 * The ring is CSS: a rotating arc in the brand colour, which respects
 * `prefers-reduced-motion` by pulsing instead of spinning.
 */
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'
import { Button as TButton, Tag as TTag } from 'tdesign-vue-next'
import { useTasksStore } from '@/stores/tasks'

const { t } = useI18n({ useScope: 'global' })
const tasks = useTasksStore()

/** The task the page is waiting on, if any. */
const task = computed(() => tasks.blocking ?? null)

/** Seconds shown in the timer: the service's own count, or the local clock. */
const elapsed = computed(() => {
  const current = task.value
  if (current === null) {
    return 0
  }
  if (current.elapsed_s !== null) {
    return current.elapsed_s
  }
  return Math.max(0, (Date.now() - current.startedAtMs) / 1000)
})

/** The wait long enough to be worth explaining rather than merely showing. */
const isSlow = computed(() => elapsed.value > 3)

/** Last few log lines, newest last. */
const lines = computed(() => (task.value?.log ?? []).slice(-5))

/** Asks the service to stop the task. */
async function cancel(): Promise<void> {
  const current = task.value
  if (current === null) {
    return
  }
  await tasks.cancel(current.id)
}
</script>

<template>
  <div
    v-if="task !== null"
    class="fixed inset-0 z-[2000] flex items-center justify-center bg-black/35"
    role="dialog"
    aria-modal="true"
    :aria-label="t(task.labelKey)"
    data-testid="task-modal"
  >
    <div class="glass flex w-[30rem] max-w-[90vw] flex-col gap-4 p-6">
      <div class="flex items-center gap-4">
        <span class="task-ring" aria-hidden="true" />
        <div class="min-w-0 flex-1">
          <h2 class="text-base font-medium text-ink">{{ t(task.labelKey) }}</h2>
          <p class="mt-1 text-sm text-muted" data-testid="task-modal-elapsed">
            {{ t('tasks.elapsed', { seconds: elapsed.toFixed(1) }) }}
          </p>
        </div>
        <TTag variant="light" :theme="task.state === 'queued' ? 'default' : 'success'">
          {{ t(`tasks.state.${task.state}`) }}
        </TTag>
      </div>

      <p v-if="isSlow" class="text-sm text-muted" data-testid="task-modal-slow">
        {{ t('tasks.slowHint') }}
      </p>

      <ul v-if="lines.length > 0" class="flex flex-col gap-1 font-mono text-xs text-muted">
        <li v-for="(line, index) in lines" :key="`${index}:${line.atMs}`" class="truncate">
          {{ line.message }}
        </li>
      </ul>

      <div class="flex items-center justify-end gap-2">
        <TButton variant="outline" data-testid="task-modal-cancel" @click="cancel()">
          {{ t('common.cancel') }}
        </TButton>
      </div>
    </div>
  </div>
</template>

<style scoped>
/*
 * The ring is one element with a transparent border and one coloured edge, rotated
 * by the animation. `prefers-reduced-motion` swaps the rotation for a pulse so the
 * loader still reads as "working" without moving.
 */
.task-ring {
  display: inline-block;
  width: 2rem;
  height: 2rem;
  flex: none;
  border: 3px solid color-mix(in srgb, var(--ourealis-line) 70%, transparent);
  border-top-color: var(--ourealis-brand);
  border-radius: 9999px;
  animation: task-ring-spin 900ms linear infinite;
}

@keyframes task-ring-spin {
  to {
    transform: rotate(360deg);
  }
}

@media (prefers-reduced-motion: reduce) {
  .task-ring {
    animation: task-ring-pulse 1.6s ease-in-out infinite;
  }

  @keyframes task-ring-pulse {
    50% {
      opacity: 0.35;
    }
  }
}
</style>
