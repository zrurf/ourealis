/*
 * Job progress events, as SSE, WebSocket and the gRPC watch stream carry them.
 *
 * One shape for all three transports (doc §4.5), tagged by `type`, so a client
 * can switch transport without changing its parser. The service's Rust enum
 * carries the same tags and fields; the field names are its own.
 *
 * The carriers differ, and the difference matters: **SSE wraps each event in an
 * envelope** (`{ at_ms, event }`) because Server-Sent Events have no field of their
 * own to carry the publication time, while a **WebSocket frame is the bare event**
 * (the frame already delimits it) and gRPC sends `type` plus `data_json`. Only
 * `api/sse.ts` has to know about the envelope.
 */

/** One event with the time the service published it, as SSE carries it. */
export interface EventEnvelope {
  /** Publication time, Unix milliseconds. */
  at_ms: number
  /** The event. */
  event: JobEvent
}

/** The job changed state. */
export interface StateEvent {
  /** Discriminator. */
  type: 'state'
  /** State after the change. */
  state: string
  /** Stage the job is in. */
  stage: string
  /** Progress, 0 to 1, or absent while the service cannot say. */
  progress?: number | null
  /** Wall-clock time the job has been running, seconds. */
  elapsed_s: number
}

/** The job entered a new pipeline stage. */
export interface StageEvent {
  /** Discriminator. */
  type: 'stage'
  /** Stage name. */
  stage: string
  /** Progress, 0 to 1, or absent while the service cannot say. */
  progress?: number | null
  /** Wall-clock time the job has been running, seconds. */
  elapsed_s: number
}

/** A log line produced by the job. */
export interface LogEvent {
  /** Discriminator. */
  type: 'log'
  /** Level: `trace`, `debug`, `info`, `warn` or `error`. */
  level: string
  /** Message, English, as the service wrote it. */
  message: string
  /** Wall-clock time the job has been running, seconds. */
  elapsed_s: number
}

/** The job finished successfully. */
export interface DoneEvent {
  /** Discriminator. */
  type: 'done'
  /** Final state. */
  state: string
  /** URL of the summary resource. */
  summary_url: string
}

/** The job failed. */
export interface ErrorEvent {
  /** Discriminator. */
  type: 'error'
  /** Failure classification, one of the service's `ErrorKind` names. */
  kind: string
  /** Failure message, English, as the service wrote it. */
  message: string
}

/** A progress or log event of a job. */
export type JobEvent = StateEvent | StageEvent | LogEvent | DoneEvent | ErrorEvent

/** Event names, matching the SSE `event:` field. */
export const JOB_EVENT_NAMES = ['state', 'stage', 'log', 'done', 'error'] as const

/** Narrows an untrusted payload to a {@link JobEvent}. */
export function isJobEvent(value: unknown): value is JobEvent {
  if (typeof value !== 'object' || value === null) {
    return false
  }
  const type: unknown = 'type' in value ? value.type : undefined
  return typeof type === 'string' && (JOB_EVENT_NAMES as readonly string[]).includes(type)
}

/** True once an event says the job can no longer change. */
export function isTerminalEvent(event: JobEvent): boolean {
  return event.type === 'done' || event.type === 'error'
}

/** A chunk request. */
export interface FetchRequest {
  /** Message discriminator. */
  type: 'fetch'
  /** Channel to read: `truth`, `gnss`, `accel`, `gyro`, `mag` or `baro`. */
  channel: string
  /** Index of the first sample to return. */
  offset: number
  /** Maximum number of samples to return. */
  limit: number
}

/** A chunk reply. */
export interface ChunkMessage {
  /** Message discriminator. */
  type: 'chunk'
  /** Channel the items belong to. */
  channel: string
  /** Index of the first item in `items`. */
  offset: number
  /** Items of this chunk. */
  items: unknown[]
}

/** A message the WebSocket client sends. */
export type ClientMessage =
  | { type: 'subscribe'; topics: string[] }
  | FetchRequest
  | { type: 'cancel' }
  | { type: 'ping' }

/** A message the service sends. */
export type ServerMessage = JobEvent | ChunkMessage

/** Key a chunk reply is matched against the request that asked for it. */
export function chunkKey(channel: string, offset: number): string {
  return `${channel}@${offset}`
}

/** Narrows an untrusted frame to a chunk reply. */
export function isChunkMessage(value: unknown): value is ChunkMessage {
  if (typeof value !== 'object' || value === null) {
    return false
  }
  const record = value as Record<string, unknown>
  return (
    record.type === 'chunk' &&
    typeof record.channel === 'string' &&
    typeof record.offset === 'number' &&
    Array.isArray(record.items)
  )
}
