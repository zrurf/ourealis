/*
 * Panning the map by dragging with a mouse button.
 *
 * Babylon's orbit camera pans on one button and rotates on any other, which leaves the middle
 * button *rotating* — the opposite of what every map and every 3D editor does, where the middle
 * button pans and the left one picks. This attaches that behaviour: a drag with a chosen button
 * slides the camera target across the *ground plane*, so the ground follows the pointer instead
 * of swinging around the target. The camera's own pointer input is restricted to the left
 * button, so the two never fight over the same gesture.
 *
 * The arithmetic is a pure function, so "which way the map moves for a given drag" is testable
 * without an engine: dragging right moves the ground right, dragging down moves it down, and the
 * vertical axis is divided by `sin(beta)` because a ground plane seen edge-on compresses.
 */
import type { ArcRotateCamera } from '@babylonjs/core/Cameras/arcRotateCamera'
import { Vector3 } from '@babylonjs/core/Maths/math.vector'
import type { Scene } from '@babylonjs/core/scene'
import { panTargetByPixels } from './panTarget'

export { metresPerPixelAt, panTargetByPixels, type GroundTarget } from './panTarget'

/** Options of {@link attachGroundPan}. */
export interface GroundPanOptions {
  /** Buttons that pan, as DOM button numbers; `1` is the middle one. `null` restores Babylon's. */
  buttons?: readonly number[]
  /** Screen-space slop before a press counts as a drag, pixels. */
  slopPx?: number
  /** Metres per screen pixel, as the camera sees it right now. */
  metresPerPixel: () => number
  /** Called when a pan finished, so a view can re-read what it now sees. */
  onPanned?: () => void
}

/** Attaches ground panning to a camera; returns a detach function. */
export function attachGroundPan(
  scene: Scene,
  camera: ArcRotateCamera,
  options: GroundPanOptions,
): () => void {
  // Only the left button rotates: the rest of the buttons belong to panning (and, in the
  // workspace, to the context menu).
  const pointers = camera.inputs.attached['pointers'] as
    | { buttons?: number[]; panningMouseButton?: number }
    | undefined
  if (pointers !== undefined) {
    pointers.buttons = [0]
    // No button pans through Babylon's own input: panning is what this module does.
    pointers.panningMouseButton = -1
  }
  const buttons = new Set(options.buttons ?? [1])
  const slop = options.slopPx ?? 2
  let from: { x: number; y: number; button: number } | null = null
  let moved = false

  const onDown = (event: PointerEvent): void => {
    if (!buttons.has(event.button)) {
      return
    }
    from = { x: event.clientX, y: event.clientY, button: event.button }
    moved = false
  }
  const onMove = (event: PointerEvent): void => {
    const start = from
    if (start === null || event.buttons === 0) {
      return
    }
    const dx = event.clientX - start.x
    const dy = event.clientY - start.y
    if (!moved && Math.hypot(dx, dy) < slop) {
      return
    }
    moved = true
    const target = camera.target
    const next = panTargetByPixels(
      { x: target.x, z: target.z },
      dx,
      dy,
      options.metresPerPixel(),
      camera.alpha,
      camera.beta,
    )
    camera.setTarget(new Vector3(next.x, target.y, next.z))
    from = { x: event.clientX, y: event.clientY, button: start.button }
  }
  const onUp = (): void => {
    if (from === null) {
      return
    }
    const panned = moved
    from = null
    moved = false
    if (panned) {
      options.onPanned?.()
    }
  }

  const element = scene.getEngine().getRenderingCanvas() ?? undefined
  if (element === undefined) {
    return () => undefined
  }
  element.addEventListener('pointerdown', onDown, true)
  element.addEventListener('pointermove', onMove, true)
  element.addEventListener('pointerup', onUp, true)
  return () => {
    element.removeEventListener('pointerdown', onDown, true)
    element.removeEventListener('pointermove', onMove, true)
    element.removeEventListener('pointerup', onUp, true)
  }
}
