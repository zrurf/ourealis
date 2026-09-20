/*
 * Liveness and capability endpoints.
 *
 * `/system/info` is what the shell reads to say which service it is talking to
 * and what this build can do; nothing here is needed before the first request,
 * so the store that holds it is loaded lazily.
 */
import { api, ApiClient } from './client'

/** Reply of the liveness probe. */
export interface Health {
  /** `ok` while the process serves requests. */
  status: string
}

/** Build facts and capability bits of the running service. */
export interface SystemInfo {
  /** Version of the service binary. */
  version: string
  /** Version of the whole workspace. */
  workspace_version: string
  /** Version of the linked simulator crate. */
  core_version: string
  /** Version of the linked map-format crate. */
  map_format_version: string
  /** API path version, `v1`. */
  api_version: string
  /** Path prefix every API route lives under. */
  api_prefix: string
  /** Build time, RFC 3339. */
  build_time: string
  /** Whether the gRPC facade is enabled. */
  rpc_enabled: boolean
  /** Whether the HTTP facade is enabled. */
  http_enabled: boolean
  /** Whether the static page is enabled. */
  web_enabled: boolean
  /** Compute backend policy applied to a run that does not name one. */
  backend: string
  /** Threads the process may run simulation stages on. */
  worker_threads: number
  /** Person parameter presets this build accepts. */
  presets: string[]
  /** Whether the embedded page is a real front-end build. */
  web_assets_built: boolean
  /** Number of embedded page files. */
  web_asset_files: number
  /** Total size of the embedded page files, bytes. */
  web_asset_bytes: number
  /** Storage mode in effect: `memory` or `disk`. */
  storage_mode: string
  /** Maps in the library at the time of the reply. */
  maps: number
  /** Jobs queued or running at the time of the reply. */
  simulations_in_flight: number
}

/** Liveness probe; the e2e lane uses it to decide whether a service is there. */
export function fetchHealth(client: ApiClient = api, signal?: AbortSignal): Promise<Health> {
  return client.get<Health>('health', { signal })
}

/** Build facts and capability bits. */
export function fetchSystemInfo(
  client: ApiClient = api,
  signal?: AbortSignal,
): Promise<SystemInfo> {
  return client.get<SystemInfo>('system/info', { signal })
}

/** Effective configuration, as nested JSON with the configuration's own field names. */
export function fetchSystemConfig(
  client: ApiClient = api,
  signal?: AbortSignal,
): Promise<Record<string, unknown>> {
  return client.get<Record<string, unknown>>('system/config', { signal })
}
