<script setup lang="ts">
/*
 * The background task tray.
 *
 * A count in the header while anything is running, and a list on click: what each
 * task is, how long it has been going, and a cancel button. This is the counterpart of
 * the modal loader — work the user does not have to wait for still has to be visible,
 * or a build that failed ten minutes ago is discovered only when its result is
 * missing.
 *
 * The list also holds finished entries until they are cleared, so a task that ended
 * while the user was reading another page is still there to be noticed.
 */
import { computed, ref } from 'vue'
import { useI18n } from 'vue-i18n'
import { Button as TButton } from 'tdesign-vue-next'
import { useTasksStore } from '@/stores/tasks'

const { t, locale } = useI18n({ useScope: 'global' })
const tasks = useTasksStore()
const open = ref(false)

/** Entries worth showing: the running ones first, then the recent history. */
const visible = computed(() => tasks.entries)

/** How many are still running. */
const running = computed(() => tasks.active.length)

/** Seconds an entry has been going, from the service's count or the local clock. */
function elapsedOf(entry: { elapsed_s: number | null; startedAtMs: number }): number {
  return entry.elapsed_s ?? Math.max(0, (Date.now() - entry.startedAtMs) / 1000)
}

/** Duration of an entry, formatted in the interface locale. */
function formatDuration(seconds: number, digits = 0): string {
  return `${new Intl.NumberFormat(locale.value, { maximumFractionDigits: digits }).format(seconds)} s`
}

/** Cancels one entry, unless it already ended. */
async function cancel(id: string): Promise<void> {
  await tasks.cancel(id)
}
</script>

<template>
  <div class="relative">
    <TButton
      variant="text"
      :aria-label="t('tasks.tray', { count: running })"
      data-testid="task-tray"
      @click="open = !open"
    >
      <span class="flex items-center gap-2">
        <span
          v-if="running > 0"
          class="inline-block h-2 w-2 rounded-full bg-brand"
          aria-hidden="true"
        />
        <span class="text-sm">{{ t('tasks.tray', { count: running }) }}</span>
      </span>
    </TButton>

    <div
      v-if="open"
      class="glass absolute right-0 top-full z-[1500] mt-2 flex w-[24rem] flex-col gap-2 p-3"
      data-testid="task-tray-list"
    >
      <p v-if="visible.length === 0" class="px-1 py-2 text-sm text-muted">
        {{ t('tasks.empty') }}
      </p>
      <template v-else>
        <div
          v-for="entry in visible"
          :key="entry.id"
          class="flex items-start gap-3 rounded-control px-1 py-2"
          :data-testid="`task-entry-${entry.id}`"
        >
          <span
            class="mt-1 inline-block h-2 w-2 flex-none rounded-full"
            :class="
              entry.state === 'succeeded'
                ? 'bg-success'
                : entry.state === 'failed'
                  ? 'bg-danger'
                  : entry.state === 'cancelled'
                    ? 'bg-muted'
                    : 'bg-brand'
            "
            aria-hidden="true"
          />
          <div class="min-w-0 flex-1">
            <p class="truncate text-sm text-ink">{{ t(entry.labelKey) }}</p>
            <p class="text-xs text-muted" :data-testid="`task-entry-${entry.id}-state`">
              {{ t(`tasks.state.${entry.state}`) }} ·
              {{ formatDuration(elapsedOf(entry), 0) }}
            </p>
            <p v-if="entry.error !== null" class="mt-1 text-xs text-danger">
              {{ entry.error }}
            </p>
          </div>
          <TButton
            v-if="entry.state === 'queued' || entry.state === 'running'"
            size="small"
            variant="text"
            :data-testid="`task-entry-${entry.id}-cancel`"
            @click="cancel(entry.id)"
          >
            {{ t('common.cancel') }}
          </TButton>
        </div>
        <div class="flex justify-end">
          <TButton
            size="small"
            variant="text"
            data-testid="task-tray-clear"
            @click="tasks.clearFinished()"
          >
            {{ t('tasks.clearFinished') }}
          </TButton>
        </div>
      </template>
    </div>
  </div>
</template>
