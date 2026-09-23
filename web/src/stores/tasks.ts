/*
 * The background task manager.
 *
 * Every long operation the page starts goes through here: the service hands back a
 * ticket, and this store tracks it until it ends, reports it in the tray, and hands
 * the result to whoever asked. Two ways to wait for one, and the difference is what
 * the user sees:
 *
 * * `run()` with `blocking: true` means the interface cannot continue without the
 *   result — the next step needs the map, the plan or the candidates — so the modal
 *   loader is shown over a dimmed page and the caller awaits the payload.
 * * `run()` without it means the work can proceed out of the way. The caller still
 *   gets a promise (so a page can react when it lands) but nothing blocks, and the
 *   tray reports progress.
 *
 * Waiting is a poll of the ticket rather than a long request: the service's own state
 * endpoint is cheap, the interval backs off, and a task that finishes while the poll
 * is in flight is read on the next tick. The live log lines come from the event
 * stream when the browser has `EventSource`, which is a nicety for the modal and never
 * the mechanism that decides the task ended.
 */
import { defineStore } from 'pinia'
import { computed, ref } from 'vue'
import { cancelTask, getTask, getTaskResult, listAllTasks, submitTask } from '@/api/tasks'
import { ApiClient, api } from '@/api/client'
import { isApiError } from '@/api/errors'
import { subscribeJobEvents } from '@/api/sse'
import { i18n } from '@/locales'
import {
  isTerminalState,
  type SubmitOptions,
  type TaskKind,
  type TaskResult,
  type TaskState,
  type TaskStateDto,
  type TaskSubmit,
} from '@/types/tasks'
import { useNotificationsStore } from './notifications'

/** First polling interval, milliseconds. */
const POLL_BASE_MS = 400

/** Interval once a task has been running for a while, milliseconds. */
const POLL_SLOW_MS = 1_500

/** How long a task may run before the interval slows down, milliseconds. */
const POLL_SLOW_AFTER_MS = 15_000

/** Entries kept in the tray after they end, newest first. */
const HISTORY_LIMIT = 20

/** A task this store is tracking. */
export interface TaskEntry {
  /** Ticket. */
  id: string
  /** Kind of work. */
  kind: TaskKind
  /** Last state the service reported. */
  state: TaskState
  /** Coarse stage the service reported. */
  stage: string
  /** Translation key of what the task does. */
  labelKey: string
  /** Seconds the service says the task has been running. */
  elapsed_s: number | null
  /** Local time the entry was created, for the live timer before the first poll. */
  startedAtMs: number
  /** Whether the interface is waiting on this task behind the modal. */
  blocking: boolean
  /** Service message, present once the task failed. */
  error: string | null
  /** Error category, present once the task failed. */
  errorKind: string | null
  /** Live log lines, newest last; only filled while the modal is open. */
  log: Array<{ atMs: number; level: string; message: string }>
}

/** Waits a task out and gives the caller its result. */
export type TaskWaiter<T> = (id: string) => Promise<T>

/** What a caller may override, for a test or an embedded use. */
export interface TaskManagerOptions {
  /** Wire client; the shared one by default. */
  client?: ApiClient
  /** Polling interval, milliseconds. */
  pollMs?: number
}

/** The task manager. */
export const useTasksStore = defineStore('tasks', () => {
  const notifications = useNotificationsStore()

  const entries = ref<TaskEntry[]>([])
  const modalTicket = ref<string | null>(null)
  let client: ApiClient = api
  let pollMs = 0

  /** Tasks that have not ended. */
  const active = computed(() => entries.value.filter((entry) => !isTerminalState(entry.state)))

  /** The ticket the modal is blocking on, when there is one. */
  const blocking = computed(() =>
    entries.value.find((entry) => entry.id === modalTicket.value && !isTerminalState(entry.state)),
  )

  /** True while any task is running, for the tray indicator. */
  const busy = computed(() => active.value.length > 0)

  /** Overrides the wire client and poll interval; used by the unit lane. */
  function configure(options: TaskManagerOptions): void {
    client = options.client ?? api
    pollMs = options.pollMs ?? 0
  }

  /**
   * Submits a task and starts tracking it.
   *
   * The ticket is in the table before the reply is parsed any further, so even a task
   * that finishes instantly is reported in order: created, running, done.
   */
  async function submit(body: TaskSubmit, options: SubmitOptions): Promise<TaskEntry> {
    const reply = await submitTask(body, client)
    const created: TaskEntry = {
      id: reply.id,
      kind: reply.kind,
      state: reply.state,
      stage: reply.state,
      labelKey: options.labelKey,
      elapsed_s: null,
      startedAtMs: Date.now(),
      blocking: options.blocking === true,
      error: null,
      errorKind: null,
      log: [],
    }
    entries.value = [created, ...entries.value].slice(0, HISTORY_LIMIT)
    if (options.blocking === true) {
      modalTicket.value = reply.id
    }
    return created
  }

  /**
   * Submits a task, waits for it and returns the payload it produced.
   *
   * This is the one call a page needs: no ticket handling, no polling, no result
   * plumbing. The payload is picked by the kind the service reported rather than by
   * the one the caller asked for, so a mismatch would be a visible error instead of a
   * silently wrong object. `blocking` only decides what the user sees while it runs.
   */
  async function run<T>(body: TaskSubmit, options: SubmitOptions): Promise<T> {
    const created = await submit(body, options)
    try {
      const result = await wait(created.id)
      return payloadOf(result, created.kind) as T
    } finally {
      if (modalTicket.value === created.id) {
        modalTicket.value = null
      }
    }
  }

  /**
   * Waits for a ticket to end and returns its result.
   *
   * The poll is what decides the outcome; the event stream only fills the log lines
   * for the modal, so a browser without `EventSource` loses the live lines and nothing
   * else.
   */
  async function wait(id: string): Promise<TaskResult> {
    attachLog(id)
    const startedAt = Date.now()
    for (;;) {
      // A poll is a sequence of waits by construction: the next request depends on
      // the previous answer.
      // oxlint-disable-next-line no-await-in-loop
      const dto = await getTask(id, client)
      apply(id, dto)
      if (isTerminalState(dto.state)) {
        detachLog(id)
        if (dto.state !== 'succeeded') {
          throw failure(dto)
        }
        // oxlint-disable-next-line no-await-in-loop
        return await getTaskResult(id, client)
      }
      // A poll is a sequence of waits by construction: the next request depends on
      // the previous answer.
      // oxlint-disable-next-line no-await-in-loop
      await delay(intervalFor(Date.now() - startedAt))
    }
  }

  /** Refreshes one tracked task and reports the change. */
  async function refresh(id: string): Promise<void> {
    const dto = await getTask(id, client)
    apply(id, dto)
  }

  /** Cancels a tracked task. */
  async function cancel(id: string): Promise<void> {
    await cancelTask(id, client)
    await refresh(id)
  }

  /**
   * Reads the tasks the service is still holding.
   *
   * The tray is not a log viewer: this is what a reloaded page calls so a task that
   * was running before the reload is still visible and cancellable.
   */
  async function sync(): Promise<void> {
    const tasks = await listAllTasks(client)
    const known = new Map(entries.value.map((candidate) => [candidate.id, candidate]))
    const merged: TaskEntry[] = []
    for (const dto of tasks) {
      const existing = known.get(dto.id)
      if (existing === undefined) {
        merged.push({
          id: dto.id,
          kind: dto.kind,
          state: dto.state,
          stage: dto.stage,
          labelKey: `tasks.kind.${dto.kind}`,
          elapsed_s: dto.elapsed_s,
          startedAtMs: Date.now(),
          blocking: false,
          error: dto.error,
          errorKind: dto.error_kind,
          log: [],
        })
        continue
      }
      merged.push({
        ...existing,
        state: dto.state,
        stage: dto.stage,
        elapsed_s: dto.elapsed_s,
        error: dto.error,
        errorKind: dto.error_kind,
      })
    }
    entries.value = merged.slice(0, HISTORY_LIMIT)
  }

  /** Clears the finished entries from the tray. */
  function clearFinished(): void {
    entries.value = entries.value.filter((entry) => !isTerminalState(entry.state))
  }

  /** Folds one reported state into the table and notifies on a terminal transition. */
  function apply(id: string, dto: TaskStateDto): void {
    const index = entries.value.findIndex((candidate) => candidate.id === id)
    const existing = entries.value[index]
    if (existing === undefined) {
      return
    }
    const wasLive = !isTerminalState(existing.state)
    entries.value[index] = {
      ...existing,
      state: dto.state,
      stage: dto.stage,
      elapsed_s: dto.elapsed_s,
      error: dto.error,
      errorKind: dto.error_kind,
    }
    if (wasLive && isTerminalState(dto.state)) {
      detachLog(id)
      // A background task finishes while the user is looking elsewhere, so its
      // outcome has to be announced; a blocking one is already on screen and the
      // caller reports it.
      if (existing.blocking) {
        return
      }
      const t = i18n.global.t
      const label = t(existing.labelKey)
      if (dto.state === 'succeeded') {
        notifications.push({ kind: 'success', message: t('tasks.done', { what: label }) })
      } else if (dto.state === 'failed') {
        notifications.push({
          kind: 'error',
          message: t('tasks.failed', { what: label }),
          fromService: dto.error ?? undefined,
        })
      }
    }
  }

  /** Interval for the next poll, slowing down once a task runs long. */
  function intervalFor(elapsedMs: number): number {
    if (pollMs > 0) {
      return pollMs
    }
    return elapsedMs > POLL_SLOW_AFTER_MS ? POLL_SLOW_MS : POLL_BASE_MS
  }

  const subscriptions = new Map<string, { close(): void }>()

  /** Subscribes to a ticket's log lines while something is showing them. */
  function attachLog(id: string): void {
    if (subscriptions.has(id)) {
      return
    }
    try {
      // A task's events are a run's events: one shape, one parser, three transports.
      const subscription = subscribeJobEvents(
        client.url(`tasks/${encodeURIComponent(id)}/events`),
        {
          onEvent: (event) => {
            if (event.type !== 'log') {
              return
            }
            const index = entries.value.findIndex((candidate) => candidate.id === id)
            const target = entries.value[index]
            if (target !== undefined) {
              entries.value[index] = {
                ...target,
                log: [
                  ...target.log.slice(-49),
                  { atMs: Date.now(), level: event.level, message: event.message },
                ],
              }
            }
          },
        },
      )
      subscriptions.set(id, subscription)
    } catch {
      // No `EventSource` in this environment: the poll still reports the outcome, and
      // the modal simply shows no live lines.
    }
  }

  /** Stops the log subscription of a ticket. */
  function detachLog(id: string): void {
    subscriptions.get(id)?.close()
    subscriptions.delete(id)
  }

  return {
    entries,
    active,
    busy,
    blocking,
    configure,
    submit,
    wait,
    run,
    refresh,
    cancel,
    sync,
    clearFinished,
  }
})

/**
 * The payload inside a tagged result.
 *
 * The service tags a result by the kind that produced it, so the kind selects the
 * field. A run has no payload here: its data is read from the simulation endpoints in
 * pages, because a whole run does not fit in one response.
 */
export function payloadOf(result: TaskResult, kind: TaskKind): unknown {
  switch (kind) {
    case 'route_preview':
    case 'route_plan':
      return 'route' in result ? result.route : undefined
    case 'synthetic_map':
      return 'map' in result ? result.map : undefined
    case 'simulation':
      return undefined
  }
}

/** The error a failed task throws. */
function failure(dto: TaskStateDto): Error {
  return new Error(dto.error ?? `task ${dto.id} ended as ${dto.state}`)
}

/** Waits for a delay. */
function delay(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms))
}

/** True when an error is the service refusing a cancellation of a finished task. */
export function isCancellationConflict(error: unknown): boolean {
  return isApiError(error) && error.status === 409
}
