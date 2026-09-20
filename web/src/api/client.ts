/*
 * Transport for the service API.
 *
 * One place builds URLs, sends requests and normalises failures, so every
 * resource module is a thin, typed wrapper over it. The base URL is empty in
 * production: the page is served by the same process that answers `/api/v1`, so
 * a relative request is already correct and same-origin. `VITE_API_BASE` exists
 * for the dev server and for pointing a preview build at another instance.
 *
 * Nothing here touches the DOM beyond `fetch`, which keeps the module usable
 * from the node-side test lanes.
 */
import { apiErrorFromBody, apiErrorFromUnknown } from './errors'
import type { Page } from './types'

/** Path prefix of the API, as the service mounts it. */
export const API_PREFIX = '/api/v1'

/** Largest page a list endpoint serves, per the documented `limit` cap. */
export const MAX_PAGE_LIMIT = 100_000

/** Default page size; the service's own default is 1000, which is small for a viewer. */
export const DEFAULT_PAGE_LIMIT = 5_000

/** A query value the client can send: a missing value omits the parameter. */
export type QueryValue = string | number | boolean | null | undefined

/** Options of an {@link ApiClient}. */
export interface ApiClientOptions {
  /** Origin and prefix the requests are built from; empty means same-origin. */
  baseUrl?: string
  /** `fetch` implementation, injectable so a test never touches the network. */
  fetchImpl?: typeof fetch
}

/**
 * Reads the API base URL of this build.
 *
 * `import.meta.env` is replaced by the bundler; the guarded read keeps the module
 * importable from a plain node context, where it is absent.
 */
export function envApiBase(): string {
  const meta = import.meta as unknown as { env?: Record<string, unknown> }
  const value = meta.env?.VITE_API_BASE
  return typeof value === 'string' ? value : ''
}

/**
 * Resolves the effective base URL: the argument first, then the build's
 * `VITE_API_BASE`, then same-origin.
 */
export function resolveBaseUrl(configured?: string): string {
  const base = configured ?? envApiBase()
  return trimTrailingSlash(base)
}

/** Drops a trailing slash so joining an endpoint path never doubles it. */
function trimTrailingSlash(value: string): string {
  return value.replace(/\/+$/, '')
}

/**
 * Joins a base URL and an endpoint path and appends the query.
 *
 * A `path` without a leading slash is relative to the API prefix; a
 * root-relative or absolute one is used as it stands, which is what the URL of a
 * resource the service named in a body (a `summary_url`, an export link) is.
 */
export function buildUrl(
  baseUrl: string,
  path: string,
  query?: Record<string, QueryValue>,
): string {
  const base = trimTrailingSlash(baseUrl)
  const endpoint = path.startsWith('/') || path.startsWith('http') ? path : `${API_PREFIX}/${path}`
  const url = endpoint.startsWith('http') || base === '' ? endpoint : `${base}${endpoint}`
  const search = new URLSearchParams()
  for (const [key, value] of Object.entries(query ?? {})) {
    if (value === undefined || value === null) {
      continue
    }
    search.append(key, String(value))
  }
  const suffix = search.toString()
  return suffix === '' ? url : `${url}${url.includes('?') ? '&' : '?'}${suffix}`
}

/** Builds the `offset`/`limit` query of a page request, clamping the limit. */
export function pageQuery(offset = 0, limit = DEFAULT_PAGE_LIMIT): Record<string, number> {
  return {
    offset: Math.max(0, Math.trunc(offset)),
    limit: Math.min(MAX_PAGE_LIMIT, Math.max(1, Math.trunc(limit))),
  }
}

/** True when a status is worth retrying without changing the request. */
export function isRetryableStatus(status: number): boolean {
  return status === 429 || status === 503
}

/**
 * Delay before reconnect attempt `attempt`, counted from zero.
 *
 * Doubling without jitter: the caller is a single page talking to a local
 * service, so a thundering herd is not the failure mode to design against, while
 * a test needs a delay it can predict.
 */
export function backoffDelay(attempt: number, baseMs = 500, maxMs = 15_000): number {
  const exponent = Math.max(0, Math.trunc(attempt))
  return Math.min(maxMs, baseMs * 2 ** exponent)
}

/**
 * Fetches the whole of a paged resource.
 *
 * The loop is driven by the page the service returns rather than by a local
 * counter, so a resource that shrinks between two requests still terminates.
 */
export async function collectPages<T>(
  fetchPage: (offset: number, limit: number) => Promise<Page<T>>,
  options: { limit?: number; maxPages?: number } = {},
): Promise<T[]> {
  const limit = options.limit ?? DEFAULT_PAGE_LIMIT
  const maxPages = options.maxPages ?? 100
  const items: T[] = []
  let offset = 0
  for (let page = 0; page < maxPages; page += 1) {
    // Pages are read one after another: the next offset depends on the page count
    // the service reported for this one.
    // oxlint-disable-next-line no-await-in-loop
    const reply = await fetchPage(offset, limit)
    items.push(...reply.items)
    offset += reply.items.length
    if (reply.items.length === 0 || offset >= reply.total) {
      break
    }
  }
  return items
}

/**
 * Runs `task` over `items` with at most `limit` in flight.
 *
 * The viewer needs it for chunk loads: fetching a screenful one at a time wastes
 * the connection, and fetching all of them at once buries a local service in
 * hundreds of simultaneous requests.
 */
export async function mapWithConcurrency<T, R>(
  items: readonly T[],
  limit: number,
  task: (item: T, index: number) => Promise<R>,
): Promise<R[]> {
  const results = Array.from<R>({ length: items.length })
  let next = 0
  const workerCount = Math.min(Math.max(1, Math.trunc(limit)), Math.max(1, items.length))
  const workers = Array.from({ length: workerCount }, async () => {
    for (;;) {
      const index = next
      next += 1
      const item = items[index]
      if (index >= items.length || item === undefined) {
        return
      }
      // Each worker takes the next free index, so the slot cannot be filled in
      // advance with Promise.all over a fixed partition.
      // oxlint-disable-next-line no-await-in-loop
      results[index] = await task(item, index)
    }
  })
  await Promise.all(workers)
  return results
}

/** JSON-speaking client over one base URL. */
export class ApiClient {
  /** Base URL every request is built from. */
  readonly baseUrl: string

  private readonly fetchImpl: typeof fetch

  constructor(options: ApiClientOptions = {}) {
    this.baseUrl = resolveBaseUrl(options.baseUrl)
    this.fetchImpl = options.fetchImpl ?? globalThis.fetch.bind(globalThis)
  }

  /** Absolute URL of an endpoint path plus its query. */
  url(path: string, query?: Record<string, QueryValue>): string {
    return buildUrl(this.baseUrl, path, query)
  }

  /** Sends a JSON request and validates the reply is JSON when one is expected. */
  async request<T>(method: string, path: string, options: RequestOptions = {}): Promise<T> {
    const url = this.url(path, options.query)
    let response: Response
    try {
      response = await this.fetchImpl(url, {
        method,
        headers: requestHeaders(options),
        body: options.body,
        signal: options.signal,
      })
    } catch (error) {
      throw apiErrorFromUnknown(error)
    }
    const body = await readBody(response)
    if (!response.ok) {
      throw apiErrorFromBody(response.status, body)
    }
    return body as T
  }

  /** GETs a JSON resource. */
  get<T>(path: string, options: RequestOptions = {}): Promise<T> {
    return this.request<T>('GET', path, options)
  }

  /** POSTs a JSON body. */
  post<T>(path: string, options: RequestOptions = {}): Promise<T> {
    return this.request<T>('POST', path, options)
  }

  /** DELETEs a resource. */
  delete<T>(path: string, options: RequestOptions = {}): Promise<T> {
    return this.request<T>('DELETE', path, options)
  }

  /** Fetches a raw response body, for exports and streams whose reader is wanted. */
  async raw(path: string, options: RequestOptions = {}): Promise<Response> {
    const url = this.url(path, options.query)
    let response: Response
    try {
      response = await this.fetchImpl(url, {
        method: options.method ?? 'GET',
        headers: requestHeaders(options),
        body: options.body,
        signal: options.signal,
      })
    } catch (error) {
      throw apiErrorFromUnknown(error)
    }
    if (!response.ok) {
      throw apiErrorFromBody(response.status, await readBody(response))
    }
    return response
  }
}

/** Options of a single request. */
export interface RequestOptions {
  /** Query parameters; a missing value omits the parameter. */
  query?: Record<string, QueryValue>
  /** Request body. A `Uint8Array` or `Blob` is sent as-is. */
  body?: BodyInit | null
  /** Content type of a JSON body; omitted for binary uploads. */
  contentType?: string
  /** Aborts the request when the caller unmounts or supersedes it. */
  signal?: AbortSignal
  /** HTTP method of {@link ApiClient.raw}. */
  method?: string
}

/** Headers of one request: JSON in, JSON out, unless the body is binary. */
function requestHeaders(options: RequestOptions): HeadersInit {
  const headers: Record<string, string> = { Accept: 'application/json' }
  if (options.body !== undefined && options.body !== null) {
    headers['Content-Type'] = options.contentType ?? 'application/json'
  }
  return headers
}

/** Parses a reply body, tolerating the empty body of a 204 and of a plain-text failure. */
async function readBody(response: Response): Promise<unknown> {
  const text = await response.text()
  if (text === '') {
    return null
  }
  try {
    return JSON.parse(text) as unknown
  } catch {
    return text
  }
}

/** The client the resource modules share. */
export const api = new ApiClient()
