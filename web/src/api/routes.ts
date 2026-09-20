/*
 * Route planning without a simulation.
 *
 * Both endpoints plan against a stored or inline map and answer with drawable
 * geometry: `preview` runs the candidate search, `plan` additionally smooths the
 * path and samples the speed limit along it. Results are long-running enough that
 * the service answers them from a background task; the request shape is the same
 * `SimulationRequest` the job endpoint takes.
 */
import { api, ApiClient } from './client'
import type { RoutePreview } from '@/types/result'
import type { SimulationRequest } from '@/types/simulation'

/** Plans candidate routes only: no motion, no sensors. */
export function previewRoutes(
  request: SimulationRequest,
  client: ApiClient = api,
  signal?: AbortSignal,
): Promise<RoutePreview> {
  return client.post<RoutePreview>('routes/preview', {
    body: JSON.stringify(request),
    signal,
  })
}

/**
 * Plans, smooths and samples the speed profile.
 *
 * The reply is a preview shape as well — the service fills `path` and the speed
 * limit arrays — so the studio can draw both answers with one renderer.
 */
export function planRoute(
  request: SimulationRequest,
  client: ApiClient = api,
  signal?: AbortSignal,
): Promise<RoutePreview> {
  return client.post<RoutePreview>('routes/plan', {
    body: JSON.stringify(request),
    signal,
  })
}
