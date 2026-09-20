/*
 * WebSocket session with a job.
 *
 * The socket carries the same progress events as SSE plus request/reply traffic,
 * so one connection serves both a live view and the paged result fetches the view
 * makes as the timeline scrolls. Messages queued before the connection opens are
 * flushed on open, and a dropped connection is re-established with the same
 * doubling backoff the event stream uses.
 *
 * The socket's frame shapes are the ones in doc §4.5; a chunk reply is matched to
 * the request that asked for it by channel and offset, which is what makes
 * `fetch` usable as a promise while the connection stays shared. One reply may
 * arrive as several frames — the service slices it at `stream_frame_samples` items
 * per frame — so `fetch` accumulates them before it resolves.
 */
import { backoffDelay, MAX_PAGE_LIMIT } from './client'
import {
  chunkKey,
  isChunkMessage,
  isJobEvent,
  type ChunkMessage,
  type ClientMessage,
  type JobEvent,
} from '@/types/events'

/** Status of the connection, as a view reports it. */
export type SocketStatus = 'connecting' | 'open' | 'retrying' | 'closed'

/** The subset of `WebSocket` this module uses, so a fake can stand in for one. */
export interface SocketLike {
  /** Sends one text frame. */
  send(data: string): void
  /** Closes the connection. */
  close(): void
  /** Registers a listener; `message` and `close` are the ones used here. */
  addEventListener(type: string, listener: (event: Event) => void): void
}

/** Builds a socket for a URL; injectable so the unit lane needs no browser. */
export type SocketFactory = (url: string) => SocketLike

/** Tuning of a {@link JobSocket}. */
export interface JobSocketOptions {
  /** Socket factory, defaulting to the browser's `WebSocket`. */
  factory?: SocketFactory
  /** Delay of the first reconnect, milliseconds. */
  retryBaseMs?: number
  /** Ceiling of the doubling delay, milliseconds. */
  retryMaxMs?: number
  /** Attempts before the session reports failure; `Infinity` keeps trying. */
  maxRetries?: number
  /** Whether a dropped connection is re-established. */
  reconnect?: boolean
  /** Schedules the reconnect; injectable so a test runs without waiting. */
  schedule?: (callback: () => void, delayMs: number) => void
  /** Topics subscribed as soon as the connection opens. */
  topics?: string[]
}

/**
 * One request waiting for its reply.
 *
 * The service answers a single `fetch` with several `chunk` frames, each carrying
 * `stream_frame_samples` items or fewer, so a request is not a single frame to match
 * but a run of frames to accumulate until the reply is complete.
 */
interface PendingFetch {
  /** Channel the reply arrives on. */
  channel: string
  /** Offset the request was made for, and the offset reported back to the caller. */
  offset: number
  /** Items the caller asked for, capped at the page limit the service documents. */
  limit: number
  /** Items accumulated from the frames received so far. */
  items: unknown[]
  /** Offset the next frame of this reply must start at. */
  nextOffset: number
  /** Item count of the first non-empty frame, or null before one arrived. */
  frameSize: number | null
  /** Channel length the service reported, when it did. */
  total: number | null
  /** Called when the reply is complete. */
  resolve: (message: ChunkMessage) => void
  /** Called when the session closes before the reply is complete. */
  reject: (error: Error) => void
}

/** A live session with one job. */
export class JobSocket {
  private readonly url: string
  private readonly options: JobSocketOptions
  private socket: SocketLike | null = null
  private readonly queue: string[] = []
  private readonly pending = new Map<string, PendingFetch>()
  private readonly eventHandlers = new Set<(event: JobEvent) => void>()
  private readonly statusHandlers = new Set<(status: SocketStatus) => void>()
  private attempt = 0
  private delivered = false
  private closed = false
  private connected = false

  constructor(url: string, options: JobSocketOptions = {}) {
    this.url = url
    this.options = options
    this.open()
  }

  /** True while the session can still send and receive. */
  get active(): boolean {
    return !this.closed
  }

  /** Subscribes to job events; returns a function that removes the handler again. */
  onEvent(handler: (event: JobEvent) => void): () => void {
    this.eventHandlers.add(handler)
    return () => this.eventHandlers.delete(handler)
  }

  /** Subscribes to connection status changes; returns a function that removes the handler. */
  onStatus(handler: (status: SocketStatus) => void): () => void {
    this.statusHandlers.add(handler)
    return () => this.statusHandlers.delete(handler)
  }

  /**
   * Asks for the given topics; they are re-subscribed automatically after a reconnect.
   *
   * Before the connection opens the topics are only remembered: the open handler
   * sends them, so asking twice would put two subscribe frames on the wire.
   */
  subscribe(topics: string[] = ['state', 'log', 'stage']): void {
    this.options.topics = topics
    if (this.connected) {
      this.send({ type: 'subscribe', topics })
    }
  }

  /** Sends a keep-alive frame. */
  ping(): void {
    this.send({ type: 'ping' })
  }

  /** Asks the service to cancel the job; equivalent to the `DELETE` endpoint. */
  cancel(): void {
    this.send({ type: 'cancel' })
  }

  /**
   * Requests one chunk and resolves with the reply that matches it.
   *
   * A reply that arrives in several frames is reassembled before the promise
   * resolves, so `items` is the requested slice and never its first frame. The
   * promise rejects when the session closes before the reply is complete, so a
   * caller is never left waiting on a socket that is gone, and a second request for
   * the same `channel@offset` is rejected rather than replacing the first.
   */
  fetch(channel: string, offset: number, limit: number): Promise<ChunkMessage> {
    return new Promise<ChunkMessage>((resolve, reject) => {
      if (this.closed) {
        reject(new Error('the job socket is closed'))
        return
      }
      const key = chunkKey(channel, offset)
      if (this.pending.has(key)) {
        reject(new Error(`a fetch for ${key} is already in flight`))
        return
      }
      this.pending.set(key, {
        channel,
        offset,
        // The service caps one reply at the page limit the client documents, so a
        // larger request is complete once that many items have arrived rather than
        // waiting for frames the service will never send.
        limit: Math.min(limit, MAX_PAGE_LIMIT),
        items: [],
        nextOffset: offset,
        frameSize: null,
        total: null,
        resolve,
        reject,
      })
      this.send({ type: 'fetch', channel, offset, limit })
    })
  }

  /**
   * Reads a whole channel by following chunks until a short page ends the run.
   *
   * `maxItems` bounds the walk: a service that keeps answering full pages would
   * otherwise fill memory, and a caller that wants everything should say so with
   * a limit it chose rather than discover one by running out of heap.
   */
  async fetchAll(
    channel: string,
    options: { limit?: number; maxItems?: number } = {},
  ): Promise<unknown[]> {
    const limit = options.limit ?? 20_000
    const maxItems = options.maxItems ?? 200_000
    const items: unknown[] = []
    let offset = 0
    while (items.length < maxItems) {
      // Chunks are requested one at a time: the next offset is the end of the last
      // chunk, which is only known once it arrives.
      // oxlint-disable-next-line no-await-in-loop
      const chunk = await this.fetch(channel, offset, limit)
      items.push(...chunk.items)
      offset += chunk.items.length
      if (chunk.items.length < limit) {
        break
      }
    }
    return items
  }

  /** Closes the session, rejects what it still owes and stops reconnecting. */
  close(): void {
    if (this.closed) {
      return
    }
    this.closed = true
    this.socket?.close()
    this.socket = null
    this.rejectPending(new Error('the job socket is closed'))
    this.setStatus('closed')
  }

  /** Opens a connection and wires its listeners. */
  private open(): void {
    if (this.closed) {
      return
    }
    this.setStatus(this.attempt === 0 ? 'connecting' : 'retrying')
    const socket = (this.options.factory ?? defaultSocketFactory)(this.url)
    this.socket = socket
    this.delivered = false
    socket.addEventListener('open', () => {
      this.connected = true
      this.setStatus('open')
      if (this.options.topics !== undefined) {
        socket.send(JSON.stringify({ type: 'subscribe', topics: this.options.topics }))
      }
      for (const message of this.queue.splice(0)) {
        socket.send(message)
      }
    })
    socket.addEventListener('message', (event) => {
      // A frame that arrived over this connection is the proof it is worth keeping:
      // resetting the budget on `open` alone would let an endpoint that accepts and
      // immediately drops connections retry at the first backoff step forever.
      if (!this.delivered) {
        this.delivered = true
        this.attempt = 0
      }
      this.receive((event as MessageEvent<unknown>).data)
    })
    socket.addEventListener('close', () => {
      this.connected = false
      this.reconnect()
    })
  }

  /** Handles one frame: a chunk reply feeds its request, everything else is an event. */
  private receive(data: unknown): void {
    const payload = typeof data === 'string' ? parseJson(data) : data
    if (isChunkMessage(payload)) {
      this.acceptChunk(payload)
      return
    }
    if (isJobEvent(payload)) {
      for (const handler of this.eventHandlers) {
        handler(payload)
      }
    }
  }

  /** Adds one reply frame to its request and resolves the request when the reply is whole. */
  private acceptChunk(payload: ChunkMessage): void {
    const entry = this.findPending(payload)
    if (entry === undefined) {
      return
    }
    const items = payload.items
    // Pushed one by one: a frame may hold more items than a spread can take as
    // arguments, and the service's own frame size is not the client's to assume.
    for (const item of items) {
      entry.items.push(item)
    }
    entry.nextOffset = payload.offset + items.length
    if (entry.frameSize === null && items.length > 0) {
      entry.frameSize = items.length
    }
    const total = reportedTotal(payload)
    if (total !== null) {
      entry.total = total
    }
    if (this.replyComplete(entry, items.length)) {
      const key = chunkKey(entry.channel, entry.offset)
      this.pending.delete(key)
      entry.resolve({
        type: 'chunk',
        channel: entry.channel,
        offset: entry.offset,
        items: entry.items,
      })
    }
  }

  /**
   * True when one more frame cannot belong to the request.
   *
   * A frame shorter than the first one, or an empty one, is the last of the reply;
   * a full frame is also the last when it fills the requested limit or reaches the
   * channel length the service reported. A reply that reports no length at all is
   * taken as a single frame, which is the shape the frame contract documents.
   */
  private replyComplete(entry: PendingFetch, frameLength: number): boolean {
    if (frameLength === 0) {
      return true
    }
    if (entry.frameSize !== null && frameLength < entry.frameSize) {
      return true
    }
    if (entry.items.length >= entry.limit) {
      return true
    }
    if (entry.total !== null) {
      return entry.nextOffset >= entry.total
    }
    return frameLength < entry.limit
  }

  /** The request a frame answers: same channel, and the frame starts where its last one ended. */
  private findPending(payload: ChunkMessage): PendingFetch | undefined {
    for (const entry of this.pending.values()) {
      if (entry.channel === payload.channel && entry.nextOffset === payload.offset) {
        return entry
      }
    }
    return undefined
  }

  /** Rejects every request still waiting for a reply. */
  private rejectPending(error: Error): void {
    const entries = [...this.pending.values()]
    this.pending.clear()
    for (const entry of entries) {
      entry.reject(error)
    }
  }

  /** Schedules another connection, or gives up and closes the session. */
  private reconnect(): void {
    if (this.closed) {
      return
    }
    const maxRetries = this.options.maxRetries ?? 8
    if (this.options.reconnect === false || this.attempt >= maxRetries) {
      this.close()
      return
    }
    const delayMs = backoffDelay(
      this.attempt,
      this.options.retryBaseMs ?? 500,
      this.options.retryMaxMs ?? 15_000,
    )
    this.attempt += 1
    this.setStatus('retrying')
    const schedule = this.options.schedule ?? ((callback, delay) => setTimeout(callback, delay))
    schedule(() => this.open(), delayMs)
  }

  /** Sends a frame now, or queues it until the connection opens. */
  private send(message: ClientMessage): void {
    const text = JSON.stringify(message)
    if (this.socket === null || !this.connected) {
      this.queue.push(text)
      return
    }
    this.socket.send(text)
  }

  /** Tells every listener about a status change. */
  private setStatus(status: SocketStatus): void {
    for (const handler of this.statusHandlers) {
      handler(status)
    }
  }
}

/**
 * Channel length a frame reports, when it carries one.
 *
 * The service sends `total` on every chunk frame, but the frame type does not
 * declare it, so it is read explicitly rather than trusted to exist.
 */
function reportedTotal(payload: ChunkMessage): number | null {
  const total = (payload as { total?: unknown }).total
  return typeof total === 'number' && Number.isFinite(total) ? total : null
}

/** Parses a text frame, yielding `undefined` for anything that is not JSON. */
function parseJson(text: string): unknown {
  try {
    return JSON.parse(text) as unknown
  } catch {
    return undefined
  }
}

/** Builds a browser `WebSocket`, failing loudly when there is none. */
function defaultSocketFactory(url: string): SocketLike {
  if (typeof WebSocket === 'undefined') {
    throw new Error('WebSocket is not available in this environment')
  }
  return new WebSocket(url)
}
