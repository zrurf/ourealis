/*
 * Tasks as the service reports them.
 *
 * Every long operation is a task: a run, a route preview, a route plan, a synthetic
 * map build. Submitting one returns a ticket immediately and the client reads the
 * state, watches the events or fetches the result under that ticket, so nothing on
 * this surface blocks a request for the length of the work.
 *
 * The kinds and states are the service's own spellings; a mismatch here is a 400
 * there, which is why they are spelled out rather than inferred.
 */
import type { MapSummary } from '@/api/types'
import type { RoutePreview } from '@/types/result'
import type { SimulationRequest } from './simulation'

/** Kind of work a task does. */
export type TaskKind = 'simulation' | 'route_preview' | 'route_plan' | 'synthetic_map'

/** Lifecycle state of a task. */
export type TaskState = 'queued' | 'running' | 'succeeded' | 'failed' | 'cancelled'

/** States a task never leaves. */
export const TERMINAL_STATES: readonly TaskState[] = ['succeeded', 'failed', 'cancelled']

/** True once a task cannot change state again. */
export function isTerminalState(state: TaskState): boolean {
  return TERMINAL_STATES.includes(state)
}

/** Body of a submission, tagged by the kind it names. */
export type TaskSubmit =
  | { kind: 'route_preview'; request: SimulationRequest }
  | { kind: 'route_plan'; request: SimulationRequest }
  | { kind: 'synthetic_map'; spec: unknown; name?: string }

/** Reply to a submission. */
export interface TaskReply {
  /** Ticket the client polls, watches and cancels. */
  id: string
  /** Kind of the submitted task. */
  kind: TaskKind
  /** State right after submission, normally `queued`. */
  state: TaskState
}

/** One task's state. */
export interface TaskStateDto {
  id: string
  kind: TaskKind
  state: TaskState
  /** Coarse stage: `queued`, `running`, `done`, `failed` or `cancelled`. */
  stage: string
  /** Always null: the service does not invent a fraction for a single-call body. */
  progress: number | null
  /** Seconds since the task started, absent while queued. */
  elapsed_s: number | null
  name: string | null
  map_id: string
  mode: string
  error: string | null
  error_kind: string | null
  created_at: string
  started_at: string | null
  finished_at: string | null
}

/** Where a finished task's result lives. */
export interface TaskResultRef {
  id: string
  kind: TaskKind
  url: string
}

/** A task's result, tagged by the kind that produced it. */
export type TaskResult = { route: RoutePreview } | { map: MapSummary }

/** Options of a submission through the task manager. */
export interface SubmitOptions {
  /** Translation key of what the task does, for the tray and the modal. */
  labelKey: string
  /**
   * Whether the interface waits on this task behind the modal loader.
   *
   * A blocking task is one the user asked for and cannot proceed without — a map
   * being generated, a plan the next step needs. Everything else is background: it
   * runs on, the tray reports it, and the user keeps working.
   */
  blocking?: boolean
}
