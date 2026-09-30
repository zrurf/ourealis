/*
 * Base64 and little-endian float decoding.
 *
 * Two wire representations carry binary payloads: a chunk fetched with
 * `?format=base64` is a base64 block of little-endian `f32` values, and the OMF
 * edit endpoints carry a whole image inside a JSON body. Both are decoded here
 * rather than through `atob`, which takes binary strings and exists only in a
 * browser — these functions run in the node-side test lanes too.
 */

/** Alphabet of standard base64, in wire order. */
const ALPHABET = 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/'

/** Standard base64 of a byte array, padded to a multiple of four. */
export function toBase64(bytes: Uint8Array): string {
  let out = ''
  for (let index = 0; index < bytes.length; index += 3) {
    const b0 = bytes[index] ?? 0
    const b1 = bytes[index + 1] ?? 0
    const b2 = bytes[index + 2] ?? 0
    const triple = (b0 << 16) | (b1 << 8) | b2
    out += ALPHABET.charAt((triple >> 18) & 0x3f)
    out += ALPHABET.charAt((triple >> 12) & 0x3f)
    out += index + 1 < bytes.length ? ALPHABET.charAt((triple >> 6) & 0x3f) : '='
    out += index + 2 < bytes.length ? ALPHABET.charAt(triple & 0x3f) : '='
  }
  return out
}

/**
 * Character code to six-bit value, with `-1` for everything outside the alphabet.
 *
 * A lookup over character codes rather than a scan of the alphabet with `indexOf`:
 * a viewer decodes a base64 block per chunk per layer, so this runs tens of
 * thousands of times over a large map and the linear scan of a 64-character string
 * was the single hottest step in loading one.
 */
const VALUES = (() => {
  const table = new Int8Array(128).fill(-1)
  for (let index = 0; index < ALPHABET.length; index += 1) {
    table[ALPHABET.charCodeAt(index)] = index
  }
  // The URL-safe alphabet swaps the last two symbols for `-` and `_`.
  table['-'.charCodeAt(0)] = 62
  table['_'.charCodeAt(0)] = 63
  return table
})()

/**
 * Decodes standard or URL-safe base64, ignoring padding and whitespace.
 *
 * A character outside the alphabet throws: a payload that is not base64 is a
 * broken reply, and silently returning a short array would show the caller a
 * chunk that looks valid but holds garbage.
 */
export function fromBase64(text: string): Uint8Array {
  const bytes = new Uint8Array(Math.floor((text.length * 6) / 8) + 1)
  let accumulator = 0
  let bits = 0
  let written = 0
  for (let index = 0; index < text.length; index += 1) {
    const code = text.charCodeAt(index)
    // Padding and whitespace are noise between groups, not content.
    if (code === 0x3d /* = */ || code <= 0x20) {
      continue
    }
    const value = code < 128 ? (VALUES[code] ?? -1) : -1
    if (value < 0) {
      throw new Error(`value is not base64: unexpected character ${JSON.stringify(text[index])}`)
    }
    accumulator = (accumulator << 6) | value
    bits += 6
    if (bits >= 8) {
      bits -= 8
      bytes[written] = (accumulator >> bits) & 0xff
      written += 1
    }
  }
  return bytes.subarray(0, written)
}

/**
 * Reads a base64 block of little-endian `f32` values.
 *
 * The service writes the values with `f32::to_le_bytes`, so the byte order is
 * read explicitly instead of being taken from the host's own endianness.
 */
export function floatsFromBase64(text: string): Float32Array {
  const bytes = fromBase64(text)
  const count = Math.floor(bytes.length / 4)
  // Taken as one block rather than read value by value: a per-element `getFloat32`
  // crosses into the DataView for every number, and one chunk carries thousands of them.
  // The wire is little-endian, so a host that stores numbers that way can adopt the
  // bytes as they are; the other order reverses each group of four.
  const values = new Float32Array(count)
  if (count === 0) {
    return values
  }
  if (LITTLE_ENDIAN_HOST) {
    values.set(new Float32Array(bytes.buffer, bytes.byteOffset, count))
    return values
  }
  const native = new Uint8Array(values.buffer, 0, count * 4)
  for (let index = 0; index < count; index += 1) {
    const at = index * 4
    native[at] = bytes[at + 3] ?? 0
    native[at + 1] = bytes[at + 2] ?? 0
    native[at + 2] = bytes[at + 1] ?? 0
    native[at + 3] = bytes[at] ?? 0
  }
  return values
}

/** Whether this host stores the least significant byte of a number first. */
const LITTLE_ENDIAN_HOST = new Uint8Array(new Uint16Array([1]).buffer)[0] === 1
