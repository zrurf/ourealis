/*
 * Incremental NDJSON reader.
 *
 * A line is only a line once its newline has arrived, and a network chunk can end
 * anywhere, so the reader keeps the tail between pushes. The splitter is a pure
 * function so the unit lane can exercise every boundary case without a stream.
 */

/** Result of splitting an NDJSON buffer: the complete lines and the unterminated tail. */
export interface NdjsonSplit {
  /** Complete lines, without their terminator; blank lines are dropped. */
  lines: string[]
  /** Text after the last newline, to be prepended to the next chunk. */
  rest: string
}

/**
 * Splits a buffer on newlines.
 *
 * A carriage return before the newline is dropped, which is what a stream from a
 * Windows-side proxy looks like. An empty line carries no record and is skipped
 * rather than reported as a parse failure.
 */
export function splitNdjson(buffer: string): NdjsonSplit {
  const parts = buffer.split('\n')
  const rest = parts.pop() ?? ''
  const lines: string[] = []
  for (const part of parts) {
    const line = part.endsWith('\r') ? part.slice(0, -1) : part
    if (line.trim() !== '') {
      lines.push(line)
    }
  }
  return { lines, rest }
}

/** Parses one complete line, reporting a malformed record instead of throwing. */
export function parseNdjsonLine(
  line: string,
): { ok: true; value: unknown } | { ok: false; line: string } {
  try {
    return { ok: true, value: JSON.parse(line) as unknown }
  } catch {
    return { ok: false, line }
  }
}

/**
 * Reads a byte stream as NDJSON.
 *
 * Every yielded value is one parsed line; a line that is not JSON is yielded as
 * its raw text, because a stream that stops at the first bad line loses the rest
 * of a timeline and the caller can decide what to do with the text.
 */
export async function* readNdjson(
  stream: ReadableStream<Uint8Array>,
): AsyncGenerator<unknown, void, undefined> {
  const reader = stream.getReader()
  const decoder = new TextDecoder()
  let buffer = ''
  try {
    for (;;) {
      // A stream is read one chunk at a time; the next read only makes sense once
      // the last one has arrived.
      // oxlint-disable-next-line no-await-in-loop
      const { done, value } = await reader.read()
      if (done) {
        break
      }
      buffer += decoder.decode(value, { stream: true })
      const { lines, rest } = splitNdjson(buffer)
      buffer = rest
      for (const line of lines) {
        const parsed = parseNdjsonLine(line)
        yield parsed.ok ? parsed.value : parsed.line
      }
    }
    buffer += decoder.decode()
    const tail = buffer.trim()
    if (tail !== '') {
      const parsed = parseNdjsonLine(tail)
      yield parsed.ok ? parsed.value : parsed.line
    }
  } finally {
    reader.releaseLock()
  }
}

/** Collects a whole NDJSON stream into an array; a caller with a long stream should iterate instead. */
export async function collectNdjson(stream: ReadableStream<Uint8Array>): Promise<unknown[]> {
  const items: unknown[] = []
  for await (const item of readNdjson(stream)) {
    items.push(item)
  }
  return items
}

/** Wraps a string in a one-chunk stream, for a test or a cached body. */
export function textStream(text: string): ReadableStream<Uint8Array> {
  const bytes = new TextEncoder().encode(text)
  return new ReadableStream<Uint8Array>({
    start(controller) {
      controller.enqueue(bytes)
      controller.close()
    },
  })
}
