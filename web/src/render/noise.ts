/*
 * A deterministic integer hash, as a unit float.
 *
 * Procedural textures need randomness that is *reproducible*: the same cell must look the
 * same after a rebuild, in another chunk, and on another machine, or a drape would flicker
 * as the viewer streams. A hash of the coordinates gives that, where `Math.random` cannot.
 */

/** Hash of two integers, uniform in `[0, 1)`. */
export function hash2(x: number, y: number): number {
  let h = Math.imul(x | 0, 0x27d4eb2d) ^ Math.imul(y | 0, 0x165667b1)
  h = Math.imul(h ^ (h >>> 15), 0x85ebca6b)
  h = Math.imul(h ^ (h >>> 13), 0xc2b2ae35)
  h ^= h >>> 16
  return (h >>> 0) / 4294967296
}
