/*
 * Transport, paging, streams and chunk decoding.
 *
 * Everything here runs without a browser: the client takes its `fetch`, the event
 * stream takes its `EventSource`, the socket takes its `WebSocket`, so each
 * failure mode — a documented error body, a body that is not one, a stream that
 * breaks halfway through a line, a reconnecting transport — is a case this file
 * can produce on purpose.
 */
import { expect, test } from '@playwright/test'
import {
  API_PREFIX,
  ApiClient,
  MAX_PAGE_LIMIT,
  backoffDelay,
  buildUrl,
  collectPages,
  mapWithConcurrency,
  pageQuery,
  resolveBaseUrl,
} from '../../src/api/client'
import { ApiError, apiErrorFromBody, apiErrorFromUnknown, isApiError } from '../../src/api/errors'
import { collectNdjson, readNdjson, splitNdjson, textStream } from '../../src/api/ndjson'
import { subscribeJobEvents, type SseSource } from '../../src/api/sse'
import { JobSocket, type SocketLike, type SocketStatus } from '../../src/api/ws'
import { fromBase64, floatsFromBase64, toBase64 } from '../../src/types/bytes'
import {
  chunkBounds,
  chunkKey,
  chunksInBounds,
  decodeChunk,
  dequantise,
  mortonDecodeChunk,
  mortonEncodeChunk,
  selectLevel,
} from '../../src/types/map'
import type { ChunkPayload, LayerGrid } from '../../src/api/types'

/** A JSON response the client can parse, built without a network. */
function jsonResponse(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { 'content-type': 'application/json' },
  })
}

test.describe('url building', () => {
  test('relative paths are prefixed and queries are encoded', () => {
    expect(buildUrl('', 'maps')).toBe(`${API_PREFIX}/maps`)
    expect(buildUrl('http://host:8080', 'maps/abc/layers/1/grid')).toBe(
      `http://host:8080${API_PREFIX}/maps/abc/layers/1/grid`,
    )
    expect(buildUrl('', 'maps', { offset: 0, limit: 10 })).toBe(
      `${API_PREFIX}/maps?offset=0&limit=10`,
    )
    expect(buildUrl('', 'maps', { name: 'campus run', offset: 0 })).toBe(
      `${API_PREFIX}/maps?name=campus+run&offset=0`,
    )
  })

  test('a null or undefined query value omits its parameter', () => {
    expect(buildUrl('', 'maps', { name: null, batch: undefined, offset: 5 })).toBe(
      `${API_PREFIX}/maps?offset=5`,
    )
  })

  test('a root-relative or absolute path is used as it stands', () => {
    expect(buildUrl('http://host:8080', '/api/v1/simulations/x/summary')).toBe(
      'http://host:8080/api/v1/simulations/x/summary',
    )
    expect(buildUrl('', 'https://example.test/health')).toBe('https://example.test/health')
  })

  test('a trailing slash on the base never doubles', () => {
    expect(buildUrl('http://host:8080/', 'maps')).toBe(`http://host:8080${API_PREFIX}/maps`)
    expect(resolveBaseUrl('http://host:8080//')).toBe('http://host:8080')
  })

  test('the page query clamps the limit the service documents', () => {
    expect(pageQuery(0, 1_000_000)).toEqual({ offset: 0, limit: MAX_PAGE_LIMIT })
    expect(pageQuery(-5, 0)).toEqual({ offset: 0, limit: 1 })
    expect(pageQuery(2.7, 10.4)).toEqual({ offset: 2, limit: 10 })
  })
})

test.describe('error mapping', () => {
  test('the documented body is read as it is written', () => {
    const error = apiErrorFromBody(404, {
      error: { kind: 'not_found', message: 'map abc is not in the library', status: 404 },
    })
    expect(error).toBeInstanceOf(ApiError)
    expect(error.kind).toBe('not_found')
    expect(error.message).toBe('map abc is not in the library')
    expect(error.status).toBe(404)
    expect(error.fromService).toBe(true)
  })

  test('every documented kind is accepted', () => {
    for (const kind of [
      'invalid',
      'unprocessable',
      'not_found',
      'conflict',
      'too_large',
      'busy',
      'unsupported',
      'core',
      'internal',
    ]) {
      expect(apiErrorFromBody(400, { error: { kind, message: 'x', status: 400 } }).kind).toBe(kind)
    }
  })

  test('a body that is not the documented shape falls back to the status', () => {
    expect(apiErrorFromBody(400, 'plain text').kind).toBe('invalid')
    expect(apiErrorFromBody(404, {}).kind).toBe('not_found')
    expect(apiErrorFromBody(503, { error: { kind: 'x', message: 'y' } }).kind).toBe('busy')
    expect(apiErrorFromBody(500, { error: { kind: 'nonsense', message: 'y' } }).kind).toBe(
      'internal',
    )
    expect(apiErrorFromBody(500, null).message).toBe('HTTP 500')
  })

  test('a failure that never reached the service is a network failure', () => {
    const error = apiErrorFromUnknown(new TypeError('Failed to fetch'))
    expect(error.kind).toBe('network')
    expect(error.status).toBeNull()
    expect(error.fromService).toBe(false)
    expect(isApiError(error)).toBe(true)
    expect(apiErrorFromUnknown(error)).toBe(error)
  })
})

test.describe('client', () => {
  test('a request carries the query and parses the reply', async () => {
    const calls: string[] = []
    const client = new ApiClient({
      baseUrl: 'http://host:8080',
      fetchImpl: async (input) => {
        calls.push(String(input))
        return jsonResponse({ items: [{ id: 'a' }], total: 1, offset: 0 })
      },
    })
    const page = await client.get<{ total: number }>('maps', { query: { offset: 0, limit: 10 } })
    expect(page.total).toBe(1)
    expect(calls[0]).toBe(`http://host:8080${API_PREFIX}/maps?offset=0&limit=10`)
  })

  test('an error reply is thrown as the service classified it', async () => {
    const client = new ApiClient({
      fetchImpl: async () =>
        jsonResponse({ error: { kind: 'busy', message: 'queue is full', status: 503 } }, 503),
    })
    await expect(client.get('maps')).rejects.toMatchObject({
      kind: 'busy',
      message: 'queue is full',
      status: 503,
    })
  })

  test('a transport failure is normalised rather than rethrown raw', async () => {
    const client = new ApiClient({
      fetchImpl: async () => {
        throw new TypeError('Failed to fetch')
      },
    })
    const error = await client.get('maps').catch((failure: unknown) => failure)
    expect(isApiError(error)).toBe(true)
    expect((error as ApiError).kind).toBe('network')
  })

  test('a 204 with no body resolves to null', async () => {
    const client = new ApiClient({ fetchImpl: async () => new Response(null, { status: 204 }) })
    await expect(client.delete('maps/a')).resolves.toBeNull()
  })

  test('paging walks the pages the service reports', async () => {
    const requested: number[] = []
    const items = await collectPages(
      async (offset, limit) => {
        requested.push(offset)
        const all = ['a', 'b', 'c', 'd', 'e']
        void limit
        return { items: all.slice(offset, offset + 2), total: all.length, offset }
      },
      { limit: 2 },
    )
    expect(items).toEqual(['a', 'b', 'c', 'd', 'e'])
    expect(requested).toEqual([0, 2, 4])
  })

  test('paging stops when a page comes back empty and never loops forever', async () => {
    const items = await collectPages(async (offset) => ({ items: [], total: 10, offset }))
    expect(items).toEqual([])
  })

  test('concurrency is bounded and order is preserved', async () => {
    let inFlight = 0
    let peak = 0
    const items = await mapWithConcurrency([1, 2, 3, 4, 5, 6, 7], 3, async (value) => {
      inFlight += 1
      peak = Math.max(peak, inFlight)
      await new Promise((resolve) => setTimeout(resolve, 1))
      inFlight -= 1
      return value * 2
    })
    expect(items).toEqual([2, 4, 6, 8, 10, 12, 14])
    expect(peak).toBeLessThanOrEqual(3)
  })

  test('the reconnect delay doubles and is capped', () => {
    expect(backoffDelay(0)).toBe(500)
    expect(backoffDelay(1)).toBe(1_000)
    expect(backoffDelay(3)).toBe(4_000)
    expect(backoffDelay(20)).toBe(15_000)
    expect(backoffDelay(0, 10, 40)).toBe(10)
  })
})

test.describe('ndjson', () => {
  test('a buffer splits on newlines and keeps the tail', () => {
    expect(splitNdjson('{"a":1}\n{"b":2}\n{"c"')).toEqual({
      lines: ['{"a":1}', '{"b":2}'],
      rest: '{"c"',
    })
    expect(splitNdjson('{"a":1}\r\n')).toEqual({ lines: ['{"a":1}'], rest: '' })
    expect(splitNdjson('\n\n')).toEqual({ lines: [], rest: '' })
  })

  test('a record split across two chunks is only reported once it is complete', async () => {
    const collected: unknown[] = []
    const stream = new ReadableStream<Uint8Array>({
      start(controller) {
        const encoder = new TextEncoder()
        controller.enqueue(encoder.encode('{"time_s":0,'))
        controller.enqueue(encoder.encode('"speed":3.1}\n{"time_s":1,"speed"'))
        controller.enqueue(encoder.encode(':3.2}\n'))
        controller.close()
      },
    })
    for await (const record of readNdjson(stream)) {
      collected.push(record)
    }
    expect(collected).toEqual([
      { time_s: 0, speed: 3.1 },
      { time_s: 1, speed: 3.2 },
    ])
  })

  test('a line that is not JSON is yielded as text rather than ending the stream', async () => {
    const items = await collectNdjson(textStream('{"a":1}\n<broken>\n{"b":2}\n'))
    expect(items).toEqual([{ a: 1 }, '<broken>', { b: 2 }])
  })

  test('a trailing record without a newline is still read', async () => {
    expect(await collectNdjson(textStream('{"a":1}\n{"b":2}'))).toEqual([{ a: 1 }, { b: 2 }])
  })
})

test.describe('sse', () => {
  /** A source that records the listeners the subscription registered. */
  class FakeSource implements SseSource {
    readonly listeners = new Map<string, Array<(event: MessageEvent<string>) => void>>()
    closed = false

    addEventListener(type: string, listener: (event: MessageEvent<string>) => void): void {
      const existing = this.listeners.get(type) ?? []
      existing.push(listener)
      this.listeners.set(type, existing)
    }

    close(): void {
      this.closed = true
    }

    /** Fires one event at the listeners of that name. */
    emit(type: string, data: unknown): void {
      for (const listener of this.listeners.get(type) ?? []) {
        listener(new MessageEvent(type, { data: JSON.stringify(data) }))
      }
    }
  }

  test('events are parsed and delivered in order', () => {
    const source = new FakeSource()
    const received: string[] = []
    const subscription = subscribeJobEvents(
      '/api/v1/simulations/x/events',
      { onEvent: (event) => received.push(event.type) },
      { factory: () => source },
    )
    // The service wraps each event in an envelope that carries its publication time,
    // so the envelope is what a real stream sends.
    source.emit('state', {
      at_ms: 1_758_000_000_000,
      event: { type: 'state', state: 'running', stage: 'running', elapsed_s: 1 },
    })
    source.emit('log', {
      at_ms: 1_758_000_000_120,
      event: { type: 'log', level: 'info', message: 'route planned', elapsed_s: 1.2 },
    })
    expect(received).toEqual(['state', 'log'])
    expect(subscription.active).toBe(true)
  })

  test('a bare event without the envelope is ignored rather than guessed at', () => {
    // Guessing would hide a wire-format change: the view would sit at "running" with
    // nothing applied and no indication why.
    const source = new FakeSource()
    const received: string[] = []
    subscribeJobEvents(
      '/events',
      { onEvent: (event) => received.push(event.type) },
      { factory: () => source },
    )
    source.emit('log', { type: 'log', level: 'info', message: 'raw', elapsed_s: 0.1 })
    expect(received).toEqual([])
  })

  test('a done event ends the stream', () => {
    const source = new FakeSource()
    const closed: string[] = []
    const subscription = subscribeJobEvents(
      '/events',
      { onClose: () => closed.push('closed') },
      { factory: () => source },
    )
    source.emit('done', {
      at_ms: 1_758_000_000_500,
      event: { type: 'done', state: 'succeeded', summary_url: '/summary' },
    })
    expect(closed).toEqual(['closed'])
    expect(subscription.active).toBe(false)
    expect(source.closed).toBe(true)
  })

  test('a transport error reconnects with the doubled delay', () => {
    const sources: FakeSource[] = []
    const retries: Array<[number, number]> = []
    const scheduled: Array<() => void> = []
    subscribeJobEvents(
      '/events',
      { onRetry: (attempt, delay) => retries.push([attempt, delay]) },
      {
        factory: () => {
          const source = new FakeSource()
          sources.push(source)
          return source
        },
        schedule: (callback) => scheduled.push(callback),
      },
    )
    for (const listener of sources[0]?.listeners.get('error') ?? []) {
      listener(new MessageEvent('error'))
    }
    expect(retries).toEqual([[1, 500]])
    expect(sources[0]?.closed).toBe(true)
    expect(scheduled).toHaveLength(1)
    scheduled[0]?.()
    expect(sources).toHaveLength(2)
  })

  test('the retry budget is finite and reports a failure', () => {
    const failures: string[] = []
    const scheduled: Array<() => void> = []
    const sources: FakeSource[] = []
    subscribeJobEvents(
      '/events',
      { onError: (error) => failures.push(error.message) },
      {
        maxRetries: 2,
        factory: () => {
          const source = new FakeSource()
          sources.push(source)
          return source
        },
        schedule: (callback) => scheduled.push(callback),
      },
    )
    for (let attempt = 0; attempt < 3; attempt += 1) {
      const source = sources[sources.length - 1]
      for (const listener of source?.listeners.get('error') ?? []) {
        listener(new MessageEvent('error'))
      }
      scheduled.pop()?.()
    }
    expect(failures).toHaveLength(1)
    expect(failures[0]).toContain('gave up')
  })

  test('closing stops a pending reconnect', () => {
    const sources: FakeSource[] = []
    const scheduled: Array<() => void> = []
    const subscription = subscribeJobEvents(
      '/events',
      {},
      {
        factory: () => {
          const source = new FakeSource()
          sources.push(source)
          return source
        },
        schedule: (callback) => scheduled.push(callback),
      },
    )
    for (const listener of sources[0]?.listeners.get('error') ?? []) {
      listener(new MessageEvent('error'))
    }
    subscription.close()
    expect(scheduled).toHaveLength(1)
    scheduled[0]?.()
    expect(sources).toHaveLength(1)
  })

  test('a stream that opens and then fails does not reset the retry budget', () => {
    // A proxy that accepts the connection and drops it again would otherwise reset
    // the budget on every open and retry at the first backoff step forever.
    const failures: string[] = []
    const sources: FakeSource[] = []
    const scheduled: Array<() => void> = []
    subscribeJobEvents(
      '/events',
      { onError: (error) => failures.push(error.message) },
      {
        maxRetries: 2,
        factory: () => {
          const source = new FakeSource()
          sources.push(source)
          return source
        },
        schedule: (callback) => scheduled.push(callback),
      },
    )
    for (let round = 0; round < 3; round += 1) {
      const source = sources[sources.length - 1]
      for (const listener of source?.listeners.get('open') ?? []) {
        listener(new MessageEvent('open'))
      }
      for (const listener of source?.listeners.get('error') ?? []) {
        listener(new MessageEvent('error'))
      }
      scheduled.pop()?.()
    }
    expect(failures).toHaveLength(1)
    expect(failures[0]).toContain('gave up')
    // One connection plus the two the budget allows, and no fourth.
    expect(sources).toHaveLength(3)
  })

  test('a stream that has delivered an event keeps its full budget', () => {
    const failures: string[] = []
    const sources: FakeSource[] = []
    const scheduled: Array<() => void> = []
    subscribeJobEvents(
      '/events',
      { onError: (error) => failures.push(error.message) },
      {
        maxRetries: 1,
        factory: () => {
          const source = new FakeSource()
          sources.push(source)
          return source
        },
        schedule: (callback) => scheduled.push(callback),
      },
    )
    for (let round = 0; round < 2; round += 1) {
      const source = sources[sources.length - 1]
      for (const listener of source?.listeners.get('open') ?? []) {
        listener(new MessageEvent('open'))
      }
      source?.emit('state', {
        at_ms: 1_758_000_000_000,
        event: { type: 'state', state: 'running', stage: 'motion', elapsed_s: 1 },
      })
      for (const listener of source?.listeners.get('error') ?? []) {
        listener(new MessageEvent('error'))
      }
      scheduled.pop()?.()
    }
    expect(failures).toEqual([])
    expect(sources).toHaveLength(3)
  })
})

test.describe('websocket session', () => {
  /** A socket that records what was sent and lets a test push frames back. */
  class FakeSocket implements SocketLike {
    readonly sent: string[] = []
    readonly listeners = new Map<string, Array<(event: Event) => void>>()
    closed = false

    addEventListener(type: string, listener: (event: Event) => void): void {
      const existing = this.listeners.get(type) ?? []
      existing.push(listener)
      this.listeners.set(type, existing)
    }

    send(data: string): void {
      this.sent.push(data)
    }

    close(): void {
      this.closed = true
    }

    /** Runs the listeners of one event. */
    emit(type: string, data?: unknown): void {
      for (const listener of this.listeners.get(type) ?? []) {
        listener(
          new MessageEvent(type, { data: data === undefined ? undefined : JSON.stringify(data) }),
        )
      }
    }
  }

  test('frames sent before the connection opens are queued, then flushed', () => {
    const socket = new FakeSocket()
    const session = new JobSocket('/api/v1/simulations/x/ws', { factory: () => socket })
    session.subscribe(['state', 'log'])
    session.ping()
    expect(socket.sent).toEqual([])
    socket.emit('open')
    expect(socket.sent.map((frame) => JSON.parse(frame))).toEqual([
      { type: 'subscribe', topics: ['state', 'log'] },
      { type: 'ping' },
    ])
    session.close()
  })

  test('a chunk reply resolves the request that asked for it', async () => {
    const socket = new FakeSocket()
    const session = new JobSocket('/ws', { factory: () => socket })
    socket.emit('open')
    const pending = session.fetch('truth', 0, 100)
    expect(JSON.parse(socket.sent[socket.sent.length - 1] ?? '{}')).toEqual({
      type: 'fetch',
      channel: 'truth',
      offset: 0,
      limit: 100,
    })
    socket.emit('message', { type: 'chunk', channel: 'truth', offset: 0, items: [{ time_s: 0 }] })
    await expect(pending).resolves.toMatchObject({ channel: 'truth', offset: 0 })
    session.close()
  })

  test('fetchAll follows chunks until a short page ends the run', async () => {
    const socket = new FakeSocket()
    const session = new JobSocket('/ws', { factory: () => socket })
    socket.emit('open')
    const collecting = session.fetchAll('truth', { limit: 2 })
    const replies = [
      { type: 'chunk', channel: 'truth', offset: 0, items: [1, 2] },
      { type: 'chunk', channel: 'truth', offset: 2, items: [3, 4] },
      { type: 'chunk', channel: 'truth', offset: 4, items: [5] },
    ]
    for (const reply of replies) {
      await Promise.resolve()
      socket.emit('message', reply)
    }
    await expect(collecting).resolves.toEqual([1, 2, 3, 4, 5])
    session.close()
  })

  test('events reach the handler and a close stops the session', () => {
    const socket = new FakeSocket()
    const session = new JobSocket('/ws', { factory: () => socket, reconnect: false })
    socket.emit('open')
    const seen: string[] = []
    session.onEvent((event) => seen.push(event.type))
    socket.emit('message', { type: 'state', state: 'running', stage: 'motion', elapsed_s: 2 })
    expect(seen).toEqual(['state'])
    session.close()
    expect(session.active).toBe(false)
    expect(socket.closed).toBe(true)
  })

  test('a close rejects the requests still waiting for a reply', async () => {
    const socket = new FakeSocket()
    const session = new JobSocket('/ws', { factory: () => socket, reconnect: false })
    socket.emit('open')
    const pending = session.fetch('truth', 0, 100)
    // The assertion goes through `rejects`, which narrows to the rejection itself:
    // reading `.message` off the union of a reply and an error would not typecheck,
    // and a reply is exactly what must not arrive here.
    session.close()
    await expect(pending).rejects.toThrow(/closed/)
    expect(session.active).toBe(false)
  })

  test('a reply split into several frames is reassembled in order', async () => {
    const socket = new FakeSocket()
    const session = new JobSocket('/ws', { factory: () => socket })
    socket.emit('open')
    // The service slices one reply at `stream_frame_samples` items per frame and
    // reports the channel length on every one, so the last frame is the short one.
    const pending = session.fetch('truth', 0, 5)
    const frames = [
      { type: 'chunk', channel: 'truth', offset: 0, total: 8, items: [1, 2, 3] },
      { type: 'chunk', channel: 'truth', offset: 3, total: 8, items: [4, 5] },
    ]
    for (const frame of frames) {
      socket.emit('message', frame)
      await Promise.resolve()
    }
    await expect(pending).resolves.toMatchObject({
      channel: 'truth',
      offset: 0,
      items: [1, 2, 3, 4, 5],
    })
    session.close()
  })

  test('a second request for the same key is rejected instead of hanging the first', async () => {
    const socket = new FakeSocket()
    const session = new JobSocket('/ws', { factory: () => socket })
    socket.emit('open')
    const first = session.fetch('truth', 0, 10)
    const duplicate = session
      .fetch('truth', 0, 10)
      .then(() => 'resolved')
      .catch((error: unknown) => (error as Error).message)
    expect(await duplicate).toContain('already in flight')
    // Only the first request went on the wire, and it is still the one the reply
    // is matched to.
    expect(socket.sent).toHaveLength(1)
    socket.emit('message', { type: 'chunk', channel: 'truth', offset: 0, items: [{ time_s: 0 }] })
    await expect(first).resolves.toMatchObject({ offset: 0, items: [{ time_s: 0 }] })
    session.close()
  })

  test('a socket that opens and then drops does not reset the retry budget', () => {
    const sockets: FakeSocket[] = []
    const statuses: SocketStatus[] = []
    const scheduled: Array<() => void> = []
    const session = new JobSocket('/ws', {
      maxRetries: 2,
      factory: () => {
        const socket = new FakeSocket()
        sockets.push(socket)
        return socket
      },
      schedule: (callback) => scheduled.push(callback),
    })
    session.onStatus((status) => statuses.push(status))
    for (let round = 0; round < 3; round += 1) {
      const socket = sockets[sockets.length - 1]
      socket?.emit('open')
      socket?.emit('close')
      scheduled.pop()?.()
    }
    // Opening is not proof of stability: the budget must run out after the two
    // retries rather than resetting on every accepted connection.
    expect(sockets).toHaveLength(3)
    expect(statuses.filter((status) => status === 'closed')).toHaveLength(1)
    expect(statuses.at(-1)).toBe('closed')
    expect(session.active).toBe(false)
  })

  test('a socket that has delivered a frame keeps its full budget', () => {
    const sockets: FakeSocket[] = []
    const scheduled: Array<() => void> = []
    const session = new JobSocket('/ws', {
      maxRetries: 1,
      factory: () => {
        const socket = new FakeSocket()
        sockets.push(socket)
        return socket
      },
      schedule: (callback) => scheduled.push(callback),
    })
    for (let round = 0; round < 2; round += 1) {
      const socket = sockets[sockets.length - 1]
      socket?.emit('open')
      socket?.emit('message', { type: 'state', state: 'running', stage: 'motion', elapsed_s: 1 })
      socket?.emit('close')
      scheduled.pop()?.()
    }
    expect(session.active).toBe(true)
    expect(sockets).toHaveLength(3)
  })
})

test.describe('chunk decoding', () => {
  const sampleChunk: ChunkPayload = {
    layer_id: 1,
    level: 0,
    chunk_id: 0,
    width: 2,
    height: 2,
    channels: 1,
    dtype: 'f32',
    scale: 1,
    bias: 0,
    data: [1, 2, 3, 4],
  }

  test('a JSON array decodes in [y][x][channel] order', () => {
    const decoded = decodeChunk(sampleChunk)
    expect(decoded.width).toBe(2)
    expect([...decoded.values]).toEqual([1, 2, 3, 4])
    expect(decoded.values[1 * 2 + 1]).toBe(4)
  })

  test('a base64 block decodes as little-endian f32', () => {
    const floats = new Float32Array([1.5, -2.25, 3, 0])
    const encoded = toBase64(new Uint8Array(floats.buffer))
    expect([...floatsFromBase64(encoded)]).toEqual([1.5, -2.25, 3, 0])
    const decoded = decodeChunk({ ...sampleChunk, data: encoded, encoding: 'base64' })
    expect([...decoded.values]).toEqual([1.5, -2.25, 3, 0])
  })

  test('base64 round-trips and rejects a character outside the alphabet', () => {
    const bytes = new Uint8Array([0, 1, 2, 250, 251, 252, 253, 254, 255])
    expect([...fromBase64(toBase64(bytes))]).toEqual([...bytes])
    expect(() => fromBase64('****')).toThrow(/not base64/)
  })

  test('the quantisation contract is applied when the caller says the samples are raw', () => {
    expect(dequantise(2, 0.5, 10)).toBe(11)
    const quantised = decodeChunk({ ...sampleChunk, scale: 0.5, bias: 10 }, { quantised: true })
    expect([...quantised.values]).toEqual([10.5, 11, 11.5, 12])
    // The service dequantises at its boundary, so the default leaves the samples alone.
    expect([...decodeChunk({ ...sampleChunk, scale: 0.5, bias: 10 }).values]).toEqual([1, 2, 3, 4])
  })

  test('chunk ids decode as 16-bit Morton codes: even bits are x, odd bits y', () => {
    expect(mortonDecodeChunk(0)).toEqual({ ix: 0, iy: 0 })
    expect(mortonDecodeChunk(1)).toEqual({ ix: 1, iy: 0 })
    expect(mortonDecodeChunk(2)).toEqual({ ix: 0, iy: 1 })
    expect(mortonDecodeChunk(3)).toEqual({ ix: 1, iy: 1 })
    for (const [ix, iy] of [
      [0, 0],
      [3, 5],
      [17, 42],
      [255, 255],
    ]) {
      const code = mortonEncodeChunk(ix ?? 0, iy ?? 0)
      expect(mortonDecodeChunk(code)).toEqual({ ix, iy })
    }
  })

  test('chunk geometry follows the level resolution and the Morton id', () => {
    const grid: LayerGrid = {
      layer_id: 1,
      level_res_m: [1, 2, 4],
      level_dims: [
        [512, 512],
        [256, 256],
        [128, 128],
      ],
      chunk_dim: [2, 2],
      chunks: [[0, 1, 2, 3], [0], [0]],
    }
    // Morton 3 is chunk (1, 1): its span starts one chunk in on both axes.
    expect(chunkBounds(grid, 256, 0, 3)).toEqual({
      min_x: 256,
      min_y: 256,
      max_x: 512,
      max_y: 512,
    })
    expect(chunkBounds(grid, 256, 1, 1)).toEqual({
      min_x: 512,
      min_y: 0,
      max_x: 1024,
      max_y: 512,
    })
  })

  test('the level follows the ground resolution and clamps to the stored levels', () => {
    const grid: LayerGrid = {
      layer_id: 1,
      level_res_m: [1, 2, 4],
      level_dims: [
        [512, 512],
        [256, 256],
        [128, 128],
      ],
      chunk_dim: [2, 2],
      chunks: [[0], [0], [0]],
    }
    expect(selectLevel(grid, 0.5)).toBe(0)
    expect(selectLevel(grid, 1)).toBe(0)
    expect(selectLevel(grid, 3)).toBe(1)
    expect(selectLevel(grid, 100)).toBe(2)
    expect(selectLevel(grid, 100, { maxLevel: 1 })).toBe(1)
  })

  test('only chunks the layer stores and that intersect the area come back', () => {
    const grid: LayerGrid = {
      layer_id: 1,
      level_res_m: [1],
      level_dims: [[512, 512]],
      chunk_dim: [2, 2],
      chunks: [[0, 3]],
    }
    // Chunk 3 covers (256, 256) to (512, 512); chunk 0 touches an area that starts
    // just inside its edge, so the area is placed fully inside chunk 3.
    const area = { min_x: 300, min_y: 300, max_x: 400, max_y: 400 }
    expect(chunksInBounds(grid, 256, 0, area)).toEqual([3])
    const straddling = { min_x: 250, min_y: 250, max_x: 300, max_y: 300 }
    expect(chunksInBounds(grid, 256, 0, straddling)).toEqual([0, 3])
    expect(chunkKey(1, 0, 3)).toBe('1/0/3')
  })
})
