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
 * Decodes standard or URL-safe base64, ignoring padding and whitespace.
 *
 * A character outside the alphabet throws: a payload that is not base64 is a
 * broken reply, and silently returning a short array would show the caller a
 * chunk that looks valid but holds garbage.
 */
export function fromBase64(text: string): Uint8Array {
  const clean = text.replace(/[\s=]/g, '')
  const bytes = new Uint8Array(Math.floor((clean.length * 6) / 8))
  let accumulator = 0
  let bits = 0
  let written = 0
  for (const character of clean) {
    const value = character === '-' ? 62 : character === '_' ? 63 : ALPHABET.indexOf(character)
    if (value < 0) {
      throw new Error(`value is not base64: unexpected character ${JSON.stringify(character)}`)
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
  const values = new Float32Array(count)
  const view = new DataView(bytes.buffer, bytes.byteOffset, count * 4)
  for (let index = 0; index < count; index += 1) {
    values[index] = view.getFloat32(index * 4, true)
  }
  return values
}
