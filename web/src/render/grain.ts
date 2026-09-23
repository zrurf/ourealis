/*
 * The fine grain of the surface.
 *
 * A height field read from a metre-scale grid has no texture of its own, so a close view of it
 * is a smooth sheet of vertex colours — the "blurry" a reader sees when they zoom in. This is
 * the detail that fixes it: a near-white noise texture that repeats every few metres in *world
 * space*, mipmapped so a distant view averages it back to flat colour.
 *
 * Generated rather than loaded: a tiling grain is the same arithmetic as the layer patterns, so
 * it needs no asset, no fetch and no new dependency, and its mean is known exactly, which is
 * what lets the material compensate for it.
 */
import { RawTexture } from '@babylonjs/core/Materials/Textures/rawTexture'
import { Texture } from '@babylonjs/core/Materials/Textures/texture'
import type { Scene } from '@babylonjs/core/scene'
import { hash2 } from './noise'

/** Side of the grain texture, texels. */
const GRAIN_TEXELS = 128

/** Mean brightness of the grain; the material scales its albedo by the reciprocal. */
export const GRAIN_MEAN = 0.94

/** Albedo scale that cancels the grain's own darkening. */
export const GRAIN_COMPENSATION = 1 / GRAIN_MEAN

/** How far the grain runs from its mean, as a fraction. */
const GRAIN_AMPLITUDE = 0.055

/**
 * Builds the grain texture.
 *
 * Two octaves, wrapping at the texture edge: a texture that wraps has no seam where it
 * repeats, which a reader would otherwise see as a grid of lines across the ground.
 */
export function grainTexture(scene: Scene): RawTexture {
  const pixels = new Uint8ClampedArray(GRAIN_TEXELS * GRAIN_TEXELS * 4)
  for (let y = 0; y < GRAIN_TEXELS; y += 1) {
    for (let x = 0; x < GRAIN_TEXELS; x += 1) {
      const value =
        wrapNoise(x / 8, y / 8, GRAIN_TEXELS / 8) * 0.6 +
        wrapNoise(x / 3, y / 3, GRAIN_TEXELS / 3) * 0.4
      const level = Math.max(
        0,
        Math.min(255, Math.round((GRAIN_MEAN + (value - 0.5) * 2 * GRAIN_AMPLITUDE) * 255)),
      )
      const pixel = (y * GRAIN_TEXELS + x) * 4
      pixels[pixel] = level
      pixels[pixel + 1] = level
      pixels[pixel + 2] = level
      pixels[pixel + 3] = 255
    }
  }
  const texture = RawTexture.CreateRGBATexture(
    pixels,
    GRAIN_TEXELS,
    GRAIN_TEXELS,
    scene,
    true,
    false,
    Texture.TRILINEAR_SAMPLINGMODE,
  )
  texture.wrapU = Texture.WRAP_ADDRESSMODE
  texture.wrapV = Texture.WRAP_ADDRESSMODE
  return texture
}

/**
 * Value noise that wraps on a period, so the texture tiles.
 *
 * The lattice is folded modulo the period before hashing, which makes opposite edges sample
 * the same lattice points and the blend across the seam continuous.
 */
function wrapNoise(x: number, y: number, period: number): number {
  const x0 = Math.floor(x)
  const y0 = Math.floor(y)
  const fx = smooth(x - x0)
  const fy = smooth(y - y0)
  const wrap = (value: number): number => ((value % period) + period) % period
  const top = mix(hash2(wrap(x0), wrap(y0)), hash2(wrap(x0 + 1), wrap(y0)), fx)
  const bottom = mix(hash2(wrap(x0), wrap(y0 + 1)), hash2(wrap(x0 + 1), wrap(y0 + 1)), fx)
  return mix(top, bottom, fy)
}

/** Smoothstep of a lattice fraction. */
function smooth(t: number): number {
  return t * t * (3 - 2 * t)
}

/** Linear blend. */
function mix(a: number, b: number, t: number): number {
  return a + (b - a) * t
}
