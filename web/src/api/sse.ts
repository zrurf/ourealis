/*
 * Server-sent events for job progress.
 *
 * `EventSource` reconnects on its own, but only with the interval the service
 * suggests and without telling the page how many times it has tried; this module
 * owns the loop instead, so the delay doubles, a caller can see the attempt count,
 * and closing the subscription stops it for good. `Last-Event-ID` is not needed:
 * the service's stream is a live view of the job, and the client re-reads the
 * job's state after a reconnect.
 */
import { backoffDelay } from './client'
import { isJobEvent, isTerminalEvent, JOB_EVENT_NAMES, type JobEvent } from '@/types/events'

/** The subset of `EventSource` this module uses. */
export interface SseSource {
  /** Registers a listener for one named event. */
  addEventListener(type: string, listener: (event: MessageEvent<string>) => void): void
  /** Closes the connection and stops `EventSource`'s own reconnect. */
  close(): void
}

/** Builds a source for a URL; injectable so the unit lane needs no browser. */
export type SseSourceFactory = (url: string) => SseSource

/** Callbacks of a subscription. */
export interface SseHandlers {
  /** One parsed progress event. */
  onEvent?: (event: JobEvent) => void
  /** A payload that is JSON but not a documented event. */
  onUnknown?: (payload: unknown) => void
  /** Called after every successful open, including a reconnect. */
  onOpen?: () => void
  /** Called before a reconnect, with the attempt number and the delay chosen. */
  onRetry?: (attempt: number, delayMs: number) => void
  /** Called when the subscription gives up: an exhausted retry budget, or a job failure. */
  onError?: (error: Error) => void
  /** Called once the stream is finished, whatever the reason. */
  onClose?: () => void
}

/** Tuning of the reconnect loop. */
export interface SseOptions {
  /** Delay of the first reconnect, milliseconds. */
  retryBaseMs?: number
  /** Ceiling of the doubling delay, milliseconds. */
  retryMaxMs?: number
  /** Attempts before the subscription reports failure; `Infinity` keeps trying. */
  maxRetries?: number
  /** Source factory, defaulting to the browser's `EventSource`. */
  factory?: SseSourceFactory
  /** Schedules the reconnect; injectable so a test runs without waiting. */
  schedule?: (callback: () => void, delayMs: number) => void
}

/** A running subscription. */
export interface SseSubscription {
  /** Stops the stream; a later event is never delivered. */
  close(): void
  /** True while the stream is open and no terminal event has arrived. */
  readonly active: boolean
}

/**
 * Subscribes to a job's progress stream.
 *
 * The stream ends after a `done` or `error` event: the service closes it too, and
 * reconnecting to a finished job would replay it from the start.
 */
export function subscribeJobEvents(
  url: string,
  handlers: SseHandlers = {},
  options: SseOptions = {},
): SseSubscription {
  const factory = options.factory ?? defaultSourceFactory
  const schedule = options.schedule ?? ((callback, delayMs) => setTimeout(callback, delayMs))
  const maxRetries = options.maxRetries ?? 8

  let source: SseSource | null = null
  let attempt = 0
  let delivered = false
  let closed = false

  const open = (): void => {
    if (closed) {
      return
    }
    const current = factory(url)
    source = current
    delivered = false
    current.addEventListener('open', () => {
      handlers.onOpen?.()
    })
    for (const name of JOB_EVENT_NAMES) {
      current.addEventListener(name, (message) => {
        const payload = parseMessage(message)
        if (!isJobEvent(payload)) {
          handlers.onUnknown?.(payload)
          return
        }
        // A stream that has carried an event is worth keeping, and only that resets
        // the attempt counter: an endpoint that accepts every connection and drops it
        // again would otherwise reset the budget each time and retry forever.
        if (!delivered) {
          delivered = true
          attempt = 0
        }
        handlers.onEvent?.(payload)
        if (isTerminalEvent(payload)) {
          close()
        }
      })
    }
    // A transport failure arrives as an unnamed `error` event, while a server-sent
    // `error` event of the named listener above carries a payload.
    current.addEventListener('error', () => {
      if (closed) {
        return
      }
      current.close()
      if (attempt >= maxRetries) {
        closed = true
        handlers.onError?.(new Error(`event stream gave up after ${attempt} reconnect(s)`))
        handlers.onClose?.()
        return
      }
      const delayMs = backoffDelay(
        attempt,
        options.retryBaseMs ?? 500,
        options.retryMaxMs ?? 15_000,
      )
      attempt += 1
      handlers.onRetry?.(attempt, delayMs)
      schedule(open, delayMs)
    })
  }

  /** Ends the subscription and tells the caller once. */
  function close(): void {
    if (closed) {
      return
    }
    closed = true
    source?.close()
    source = null
    handlers.onClose?.()
  }

  open()

  return {
    close,
    get active() {
      return !closed
    },
  }
}

/**
 * Parses one SSE `data:` payload.
 *
 * The service wraps every event in an envelope (`{ at_ms, event }`), so the
 * wrapper is unwrapped here and callers only ever see a `JobEvent`. A payload that
 * is neither shape is reported as unknown rather than guessed at: a silent guess
 * would leave the view stuck at "running" with no indication why.
 */
function parseEnvelope(text: string): { atMs: number; event: JobEvent } | null {
  let parsed: unknown
  try {
    parsed = JSON.parse(text) as unknown
  } catch {
    return null
  }
  if (typeof parsed !== 'object' || parsed === null) {
    return null
  }
  const record = parsed as Record<string, unknown>
  const inner = record['event']
  if (!isJobEvent(inner)) {
    return null
  }
  const atMs = typeof record['at_ms'] === 'number' ? record['at_ms'] : Date.now()
  return { atMs, event: inner }
}

/** Parses one message, unwrapping the service's event envelope. */
function parseMessage(message: MessageEvent<string> | undefined): JobEvent | null {
  const text = message?.data
  if (typeof text !== 'string') {
    return null
  }
  return parseEnvelope(text)?.event ?? null
}

/** Builds a browser `EventSource`, failing loudly when there is none. */
function defaultSourceFactory(url: string): SseSource {
  if (typeof EventSource === 'undefined') {
    throw new Error('EventSource is not available in this environment')
  }
  return new EventSource(url)
}
