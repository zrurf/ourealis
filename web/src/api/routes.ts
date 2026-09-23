/*
 * Route planning, through the task manager.
 *
 * Planning runs on the service as a task: the submission returns a ticket and the
 * result arrives under it, so a request never holds a connection open for the tens of
 * seconds the planner can take. Both calls go through the store, which tracks the
 * ticket, reports it in the tray and hands back the payload — a caller writes the same
 * `await` it used to write against the synchronous endpoints.
 *
 * The two differ only in how far the pipeline runs: a preview stops after the
 * candidate search, a plan also smooths the chosen path and samples its speed limits.
 * They are separate kinds rather than one call with a flag because the second stage is
 * much more expensive, and a client editing a route needs the first long before it
 * needs the second.
 */
import { useTasksStore } from '@/stores/tasks'
import type { RoutePreview } from '@/types/result'
import type { SimulationRequest } from '@/types/simulation'

/** Whether the wait blocks the interface, and what to call it in the tray. */
export interface PlanOptions {
  /** Translation key of the operation, for the tray and the modal. */
  labelKey?: string
  /**
   * Whether the page cannot continue without the answer.
   *
   * A studio that redraws as the route changes plans in the background; a step that
   * needs the profile before it can continue blocks behind the modal loader.
   */
  blocking?: boolean
}

/** Plans candidate routes only: no motion, no sensors. */
export function previewRoutes(
  request: SimulationRequest,
  options: PlanOptions = {},
): Promise<RoutePreview> {
  return useTasksStore().run<RoutePreview>(
    { kind: 'route_preview', request },
    {
      labelKey: options.labelKey ?? 'tasks.labels.preview',
      blocking: options.blocking === true,
    },
  )
}

/**
 * Plans, smooths and samples the speed profile.
 *
 * The reply is a preview shape as well — the service fills `path` and the speed limit
 * arrays — so the studio can draw both answers with one renderer.
 */
export function planRoute(
  request: SimulationRequest,
  options: PlanOptions = {},
): Promise<RoutePreview> {
  return useTasksStore().run<RoutePreview>(
    { kind: 'route_plan', request },
    {
      labelKey: options.labelKey ?? 'tasks.labels.plan',
      blocking: options.blocking === true,
    },
  )
}
