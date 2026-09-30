/*
 * Frustum culling, as arithmetic.
 *
 * A large map is a thousand meshes, and the engine walks all of them on every frame to
 * decide which to draw. Babylon already answers "is this one in view" per mesh; what it
 * does not do is stop *submitting* the ones it will reject, and a chunk that has streamed
 * out of the camera's footprint stays in the scene until the map is torn down.
 *
 * This file is the decision, with no engine in it: planes in, a keep/cull answer out.
 * That matters more than it sounds. A culler that is subtly wrong does not look wrong,
 * it looks like a map with holes in it, and the only way to be sure of it is to be able
 * to test it exhaustively without a GPU.
 *
 * The rule throughout is one-sided. `boxInsideFrustum` may only ever answer "no" when
 * the box is *provably* outside: an unbounded box, a degenerate plane set or a
 * non-finite coordinate all answer "yes". A box that is wrongly kept costs one draw call;
 * a box that is wrongly dropped costs the reader a piece of the map they were looking at.
 */

/** A plane as `a·x + b·y + c·z + d = 0`, with the normal pointing into the visible half-space. */
export interface FrustumPlane {
  /** Normal's x component. */
  a: number
  /** Normal's y component. */
  b: number
  /** Normal's z component. */
  c: number
  /** Signed distance from the origin along the normal. */
  d: number
}

/** An axis-aligned box in world metres. */
export interface Aabb {
  /** Least corner. */
  min: readonly [number, number, number]
  /** Greatest corner. */
  max: readonly [number, number, number]
}

/**
 * How much bigger than itself a box is treated, as a fraction of its own size.
 *
 * A chunk's bounds are exact, but the ground under it is drawn with a terrace and a
 * grain, and the camera carries a near plane close enough that a mesh exactly on the
 * boundary can flicker. Growing the box is the cheap way to be sure neither happens.
 */
export const CULL_MARGIN = 0.05

/** Whether a box has any part in the visible half-space of one plane. */
function insidePlane(
  plane: FrustumPlane,
  min: readonly [number, number, number],
  max: readonly [number, number, number],
): boolean {
  // The corner furthest along the normal: if even that one is behind the plane, every
  // other corner is too, and the whole box is outside. One corner rather than eight
  // comparisons per plane, and exact for an axis-aligned box.
  const x = plane.a >= 0 ? max[0] : min[0]
  const y = plane.b >= 0 ? max[1] : min[1]
  const z = plane.c >= 0 ? max[2] : min[2]
  return plane.a * x + plane.b * y + plane.c * z + plane.d >= 0
}

/** Whether every coordinate of a box is finite, which a mesh with no geometry does not have. */
function finiteBox(box: Aabb): boolean {
  const { min, max } = box
  return (
    Number.isFinite(min[0]) &&
    Number.isFinite(min[1]) &&
    Number.isFinite(min[2]) &&
    Number.isFinite(max[0]) &&
    Number.isFinite(max[1]) &&
    Number.isFinite(max[2])
  )
}

/** A box grown by {@link CULL_MARGIN} of its own size on every side. */
export function grow(box: Aabb, margin = CULL_MARGIN): Aabb {
  const dx = (box.max[0] - box.min[0]) * margin
  const dy = (box.max[1] - box.min[1]) * margin
  const dz = (box.max[2] - box.min[2]) * margin
  return {
    min: [box.min[0] - dx, box.min[1] - dy, box.min[2] - dz],
    max: [box.max[0] + dx, box.max[1] + dy, box.max[2] + dz],
  }
}

/**
 * Whether a box may touch the frustum.
 *
 * Conservative in one direction only, as described at the top of the file: anything it is
 * not certain about is kept.
 */
export function boxInsideFrustum(
  planes: readonly FrustumPlane[],
  box: Aabb,
  margin = CULL_MARGIN,
): boolean {
  // A camera we cannot reason about keeps everything. The whole set is checked before the
  // first box test rather than plane by plane: a bad plane at the end of the list would
  // otherwise never be reached, because an earlier plane had already answered "outside".
  if (!frustumUsable(planes) || !finiteBox(box)) {
    return true
  }
  const grown = margin > 0 ? grow(box, margin) : box
  for (const plane of planes) {
    if (!insidePlane(plane, grown.min, grown.max)) {
      return false
    }
  }
  return true
}

/** One thing the culler decides about, with the box that decides it. */
export interface CullCandidate<T> {
  /** The box, in world metres. */
  box: Aabb
  /** What the answer is about. */
  value: T
}

/** The decision, for one camera position. */
export interface CullResult<T> {
  /** Candidates to keep drawing. */
  visible: CullCandidate<T>[]
  /** Candidates that are provably out of view. */
  culled: CullCandidate<T>[]
}

/**
 * Splits candidates into the ones to draw and the ones to drop.
 *
 * The `protectedBoxes` argument is the one that makes this safe to use with a camera that
 * can sit *inside* something: a box containing a protected point is never dropped, so the
 * ground under the camera, the target it orbits and anything a reader is looking straight
 * at survive a test their own bounds would not pass.
 */
export function cullCandidates<T>(
  planes: readonly FrustumPlane[],
  candidates: readonly CullCandidate<T>[],
  protectedPoints: readonly (readonly [number, number, number])[] = [],
  margin = CULL_MARGIN,
): CullResult<T> {
  const visible: CullCandidate<T>[] = []
  const culled: CullCandidate<T>[] = []
  for (const candidate of candidates) {
    const protectedHere = protectedPoints.some((point) => boxContains(candidate.box, point))
    if (protectedHere || boxInsideFrustum(planes, candidate.box, margin)) {
      visible.push(candidate)
    } else {
      culled.push(candidate)
    }
  }
  return { visible, culled }
}

/** Whether a point is inside a box, or on its face. */
export function boxContains(box: Aabb, point: readonly [number, number, number]): boolean {
  return (
    point[0] >= box.min[0] &&
    point[0] <= box.max[0] &&
    point[1] >= box.min[1] &&
    point[1] <= box.max[1] &&
    point[2] >= box.min[2] &&
    point[2] <= box.max[2]
  )
}

/** Whether every plane set is usable, which is the precondition for culling at all. */
export function frustumUsable(planes: readonly FrustumPlane[]): boolean {
  return (
    planes.length >= 6 &&
    planes.every(
      (plane) =>
        Number.isFinite(plane.a) &&
        Number.isFinite(plane.b) &&
        Number.isFinite(plane.c) &&
        Number.isFinite(plane.d),
    )
  )
}
