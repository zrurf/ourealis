/*
 * Moving the camera target across the ground, as arithmetic.
 *
 * Panning is the gesture a reader uses most, and what it does has to be predictable: the ground
 * follows the pointer in every camera orientation, and a shallow view moves further per pixel than
 * a top-down one because a ground plane seen edge-on compresses. That is four lines of vector
 * maths, kept pure so the direction and the scaling can be tested without an engine — the engine
 * binding next door (`render/pan.ts`) only wires it to pointer events.
 */

/** A camera target on the map's ground plane. */
export interface GroundTarget {
  /** World x, metres. */
  x: number
  /** World z, metres. */
  z: number
}

/**
 * Moves a ground target so the map follows a pointer drag.
 *
 * `metresPerPixel` is the ground resolution at the target; the caller measures it once per drag
 * step, because the resolution changes as the map is panned across a perspective view.
 */
export function panTargetByPixels(
  target: GroundTarget,
  dxPx: number,
  dyPx: number,
  metresPerPixel: number,
  azimuthRad: number,
  betaRad: number,
): GroundTarget {
  // Direction from the camera toward the target, on the ground plane.
  const forwardX = -Math.cos(azimuthRad)
  const forwardZ = -Math.sin(azimuthRad)
  // Screen right, on the ground plane: the forward direction turned a quarter turn.
  const rightX = forwardZ
  const rightZ = -forwardX
  // A ground plane seen at a shallow angle is *foreshortened*: a metre along the view direction
  // covers `cos(beta)` of the vertical axis, so a pixel covers that much more ground and the
  // camera has to travel further to move the picture by one. `beta` is measured from straight
  // down, so `cos(beta)` runs from 1 overhead to 0 at the horizon; it is floored to keep a
  // grazing drag finite.
  const vertical = dyPx * metresPerPixel
  const climb = Math.max(0.2, Math.cos(betaRad))
  return {
    x: target.x - rightX * dxPx * metresPerPixel + forwardX * (vertical / climb),
    z: target.z - rightZ * dxPx * metresPerPixel + forwardZ * (vertical / climb),
  }
}

/**
 * Ground resolution a perspective camera shows at its target, metres per screen pixel.
 *
 * The visible height at the target is `2 · radius · tan(fov / 2)`, spread over the canvas; a
 * reading of the ground adds the pitch, because a tilted plane covers more of it per pixel.
 */
export function metresPerPixelAt(radius: number, canvasHeightPx: number, fovRad: number): number {
  const height = Math.max(1, canvasHeightPx)
  return (2 * radius * Math.tan(fovRad / 2)) / height
}
