<script setup lang="ts">
/*
 * The menu a right-click opens on the map.
 *
 * A route is edited by pointing at the map, and until now every action needed a
 * modifier or a second gesture: drag to move, double-click to delete, click twice to
 * place the ends. Right-click is where a map's actions belong, and it is also the only
 * way to say "this point is a dwell" without hunting for a control in the panel.
 *
 * The menu is positioned at the pointer, dismisses on Escape or on a click outside, and
 * keeps its items in the order a route is built: ends first, then the waypoint rows.
 */
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'
import type { HandleRole } from '@/render/handles'
import type { SemanticsKind } from '@/components/forms/request'
import { useWorkspaceStore } from '@/stores/workspace'

/** A waypoint the menu may retag. */
export interface MenuHandle {
  /** Field the point belongs to. */
  role: HandleRole
  /** Index inside `waypoints` or `checkpoints`; zero for the single-point roles. */
  index: number
}

const props = defineProps<{
  /** Where the menu is drawn, in client coordinates. */
  at: { x: number; y: number }
  /** The handle under the pointer when it was opened, or `null` for the ground. */
  handle: MenuHandle | null
  /** A point still to be placed, so "set start here" knows what it would overwrite. */
  hasStart: boolean
  /** A goal still to be placed. */
  hasGoal: boolean
}>()

const emit = defineEmits<{
  /** The menu is finished with. */
  close: []
  /** A point should be placed at the pointer's ground position. */
  place: [target: { kind: HandleRole; index: number }]
  /** A waypoint's passing behaviour should change. */
  semantics: [index: number, semantics: SemanticsKind]
}>()

const { t } = useI18n({ useScope: 'global' })
const workspace = useWorkspaceStore()

/** Whether the menu is about a point that exists. */
const onHandle = computed(() => props.handle !== null)

/** The three passing behaviours, with the one in force marked. */
const semantics = computed<Array<{ value: SemanticsKind; labelKey: string }>>(() => [
  { value: 'pass', labelKey: 'simulation.route.semanticsPass' },
  { value: 'slow', labelKey: 'simulation.route.semanticsSlow' },
  { value: 'dwell', labelKey: 'simulation.route.semanticsDwell' },
])

/** Which behaviour the waypoint under the menu carries. */
const current = computed<SemanticsKind | null>(() => {
  const handle = props.handle
  if (handle === null || handle.role !== 'waypoint') {
    return null
  }
  return workspace.draft.waypoints[handle.index]?.semantics ?? null
})

/** Removes the point the menu was opened on. */
function remove(): void {
  const handle = props.handle
  if (handle === null) {
    return
  }
  workspace.removePoint({ kind: handle.role, index: handle.index })
  emit('close')
}
</script>

<template>
  <div
    class="glass fixed z-[1800] flex min-w-48 flex-col py-1"
    :style="{ left: `${at.x}px`, top: `${at.y}px` }"
    role="menu"
    data-testid="map-menu"
  >
    <template v-if="onHandle">
      <button
        v-if="handle?.role !== 'start' && handle?.role !== 'goal'"
        type="button"
        role="menuitem"
        class="px-3 py-1.5 text-left text-sm text-ink hover:bg-surface"
        data-testid="menu-remove"
        @click="remove()"
      >
        {{ t('run.menu.remove', { what: t(`run.handle.role.${handle?.role}`) }) }}
      </button>
      <template v-else>
        <button
          type="button"
          role="menuitem"
          class="px-3 py-1.5 text-left text-sm text-ink hover:bg-surface"
          data-testid="menu-clear"
          @click="remove()"
        >
          {{ t('run.menu.clear', { what: t(`run.handle.role.${handle?.role}`) }) }}
        </button>
      </template>
      <template v-if="handle?.role === 'waypoint'">
        <div class="my-1 border-t border-line" />
        <button
          v-for="entry in semantics"
          :key="entry.value"
          type="button"
          role="menuitem"
          class="flex items-center justify-between gap-3 px-3 py-1.5 text-left text-sm hover:bg-surface"
          :class="entry.value === current ? 'text-brand' : 'text-ink'"
          :data-testid="`menu-semantics-${entry.value}`"
          @click="emit('semantics', handle?.index ?? 0, entry.value)"
        >
          {{ t(entry.labelKey) }}
          <span v-if="entry.value === current" aria-hidden="true">•</span>
        </button>
      </template>
    </template>

    <template v-else>
      <button
        type="button"
        role="menuitem"
        class="px-3 py-1.5 text-left text-sm text-ink hover:bg-surface"
        data-testid="menu-set-start"
        @click="emit('place', { kind: 'start', index: 0 })"
      >
        {{ t('run.menu.setStart', { replace: hasStart ? t('run.menu.replace') : '' }) }}
      </button>
      <button
        v-if="workspace.draft.mode !== 'loop'"
        type="button"
        role="menuitem"
        class="px-3 py-1.5 text-left text-sm text-ink hover:bg-surface"
        data-testid="menu-set-goal"
        @click="emit('place', { kind: 'goal', index: 0 })"
      >
        {{ t('run.menu.setGoal', { replace: hasGoal ? t('run.menu.replace') : '' }) }}
      </button>
      <button
        v-else
        type="button"
        role="menuitem"
        class="px-3 py-1.5 text-left text-sm text-ink hover:bg-surface"
        data-testid="menu-set-reference"
        @click="emit('place', { kind: 'reference', index: 0 })"
      >
        {{ t('run.menu.setReference') }}
      </button>
      <div class="my-1 border-t border-line" />
      <button
        type="button"
        role="menuitem"
        class="px-3 py-1.5 text-left text-sm text-ink hover:bg-surface"
        data-testid="menu-add-waypoint"
        @click="emit('place', { kind: 'waypoint', index: -1 })"
      >
        {{ t('run.menu.addWaypoint') }}
      </button>
    </template>
  </div>
</template>
