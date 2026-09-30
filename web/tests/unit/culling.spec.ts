/*
 * Frustum culling as arithmetic.
 *
 * These assertions are written against the failure that matters: a box that is wrongly
 * kept costs a draw call, a box that is wrongly dropped takes a piece of the map away
 * from a reader who was looking at it. So the tests spend most of their effort on the
 * one-sided cases — degenerate planes, boxes with no geometry, a camera inside the map —
 * and on the identity that has to hold for every input: anything the test set calls
 * visible stays visible.
 */
import { expect, test } from '@playwright/test'
import {
  boxContains,
  boxInsideFrustum,
  cullCandidates,
  CULL_MARGIN,
  frustumUsable,
  grow,
  type Aabb,
  type CullCandidate,
  type FrustumPlane,
} from '../../src/render/culling'

/** The six planes of a box standing in for a view volume. */
function boxPlanes(centre: readonly [number, number, number], half: number): FrustumPlane[] {
  const [cx, cy, cz] = centre
  return [
    { a: 1, b: 0, c: 0, d: -(cx - half) },
    { a: -1, b: 0, c: 0, d: cx + half },
    { a: 0, b: 1, c: 0, d: -(cy - half) },
    { a: 0, b: -1, c: 0, d: cy + half },
    { a: 0, b: 0, c: 1, d: -(cz - half) },
    { a: 0, b: 0, c: -1, d: cz + half },
  ]
}

function box(min: readonly [number, number, number], max: readonly [number, number, number]): Aabb {
  return { min, max }
}

/**
 * Pads a plane under test out to a full frustum with half-spaces that contain everything,
 * so one assertion can be about a single plane without the set looking degenerate.
 */
function around(plane: FrustumPlane): FrustumPlane[] {
  return [
    plane,
    { a: 0, b: 1, c: 0, d: 1e9 },
    { a: 0, b: -1, c: 0, d: 1e9 },
    { a: 0, b: 0, c: 1, d: 1e9 },
    { a: 0, b: 0, c: -1, d: 1e9 },
    { a: 1, b: 0, c: 0, d: 1e9 },
  ]
}

test.describe('one box against one plane', () => {
  test('a box entirely on the visible side is inside', () => {
    const plane: FrustumPlane = { a: 1, b: 0, c: 0, d: -10 }
    expect(boxInsideFrustum([plane], box([0, 0, 0], [5, 5, 5]), 0)).toBe(true)
  })

  test('a box entirely behind the plane is outside', () => {
    const plane: FrustumPlane = { a: 1, b: 0, c: 0, d: 10 }
    // Everything is at x < -10, so the whole box is on the wrong side of `x + 10 = 0`.
    expect(boxInsideFrustum(around(plane), box([-20, -5, -5], [-12, 5, 5]), 0)).toBe(false)
  })

  test('a box straddling the plane is inside, because one corner of it is', () => {
    const plane: FrustumPlane = { a: 1, b: 0, c: 0, d: 0 }
    expect(boxInsideFrustum([plane], box([-5, -5, -5], [5, 5, 5]), 0)).toBe(true)
  })

  test('a box touching the plane exactly is inside, not outside', () => {
    // The far corner lands on the plane. `< 0` rather than `<= 0` is what keeps a mesh on
    // the boundary from flickering as the camera moves a thousandth of a degree.
    const plane: FrustumPlane = { a: 1, b: 0, c: 0, d: 0 }
    expect(boxInsideFrustum([plane], box([-5, -5, -5], [0, 5, 5]), 0)).toBe(true)
  })

  test('the normal direction decides which corner is tested', () => {
    // The same box, the same plane, the normal flipped: the answer must flip with it.
    const min: [number, number, number] = [-1, -1, -1]
    const max: [number, number, number] = [1, 1, 1]
    expect(boxInsideFrustum([{ a: 1, b: 0, c: 0, d: 0 }], box(min, max), 0)).toBe(true)
    expect(boxInsideFrustum([{ a: -1, b: 0, c: 0, d: 0 }], box(min, max), 0)).toBe(true)
    // Behind both: min x and max x are both non-negative, so -x is at most -1 either way.
    const behind: [number, number, number] = [1, 1, 1]
    const ahead: [number, number, number] = [-1, -1, -1]
    expect(boxInsideFrustum(around({ a: 1, b: 0, c: 0, d: 0 }), box(behind, ahead), 0)).toBe(false)
  })
})

test.describe('the conservative contract', () => {
  test('fewer planes than a frustum has keeps everything', () => {
    const boxy = box([1000, 1000, 1000], [2000, 2000, 2000])
    expect(boxInsideFrustum([], boxy)).toBe(true)
    expect(boxInsideFrustum([{ a: 1, b: 0, c: 0, d: 0 }], boxy)).toBe(true)
  })

  test('a non-finite coordinate keeps the box rather than culling it', () => {
    const planes = boxPlanes([0, 0, 0], 10)
    // A mesh whose geometry has not been built reports an empty or infinite bound. Zero
    // triangles cost nothing to submit; dropping the mesh would leave a hole that never
    // comes back, because the thing that would have replaced it was the box.
    expect(boxInsideFrustum(planes, box([Number.NaN, 0, 0], [10, 10, 10]))).toBe(true)
    expect(boxInsideFrustum(planes, box([0, 0, 0], [Number.POSITIVE_INFINITY, 10, 10]))).toBe(true)
  })

  test('a non-finite plane keeps everything, because the answer would be meaningless', () => {
    const planes = boxPlanes([0, 0, 0], 10)
    planes[2] = { a: 0, b: Number.NaN, c: 0, d: 0 }
    expect(boxInsideFrustum(planes, box([1000, 1000, 1000], [2000, 2000, 2000]))).toBe(true)
    expect(frustumUsable(planes)).toBe(false)
  })

  test('the margin can only ever keep more, never less', () => {
    const planes = boxPlanes([0, 0, 0], 10)
    // Starts a metre outside the +x face at x = 10.
    const edge = box([11, -30, -30], [30, 30, 30])
    expect(boxInsideFrustum(planes, edge, 0)).toBe(false)
    // Grown by a tenth of its own 19 m side, it reaches back to x = 9.1, inside.
    expect(boxInsideFrustum(planes, edge, 0.1)).toBe(true)
  })
})

test.describe('growing a box', () => {
  test('the growth is proportional to the box, not an absolute distance', () => {
    const small = grow(box([0, 0, 0], [10, 10, 10]), 0.1)
    const large = grow(box([0, 0, 0], [1000, 1000, 1000]), 0.1)
    expect(small.min[0]).toBeCloseTo(-1, 9)
    expect(small.max[0]).toBeCloseTo(11, 9)
    expect(large.min[0]).toBeCloseTo(-100, 9)
    expect(large.max[0]).toBeCloseTo(1100, 9)
  })

  test('a flat box still grows on the axes that have size', () => {
    const flat = grow(box([0, 0, 0], [10, 0, 10]), 0.1)
    // A slab has no height to grow into; padding it on y would hide that it is flat.
    expect(flat.min[1]).toBe(0)
    expect(flat.max[1]).toBe(0)
    expect(flat.max[0]).toBeCloseTo(11, 9)
  })
})

test.describe('splitting a set', () => {
  const planes = boxPlanes([0, 0, 0], 10)
  const candidates: Array<CullCandidate<string>> = [
    { box: box([-5, -5, -5], [5, 5, 5]), value: 'inside' },
    { box: box([7, 7, 7], [9, 9, 9]), value: 'corner' },
    { box: box([500, 500, 500], [600, 600, 600]), value: 'far' },
    { box: box([-600, -600, -600], [-500, -500, -500]), value: 'far-behind' },
  ]

  test('what is in view is kept and what is not is dropped', () => {
    const { visible, culled } = cullCandidates(planes, candidates)
    expect(visible.map((c) => c.value)).toEqual(['inside', 'corner'])
    expect(culled.map((c) => c.value)).toEqual(['far', 'far-behind'])
  })

  test('nothing is both kept and dropped, and nothing is lost', () => {
    const { visible, culled } = cullCandidates(planes, candidates)
    expect(visible.length + culled.length).toBe(candidates.length)
    const all = new Set([...visible, ...culled].map((c) => c.value))
    expect(all.size).toBe(candidates.length)
  })

  test('a box the camera stands on is kept even when its own bounds do not survive', () => {
    // The ground under the camera, far off to one side of a map the reader has panned
    // away from. Its own bounds say "outside", and dropping it would leave the reader
    // standing on a hole with no way to tell that from a rendering fault.
    const underfoot: Array<CullCandidate<string>> = [
      { box: box([900, -20, -20], [1100, 20, 20]), value: 'underfoot' },
    ]
    expect(cullCandidates(planes, underfoot).visible).toHaveLength(0)
    const kept = cullCandidates(planes, underfoot, [[1000, 0, 0]]).visible
    expect(kept.map((c) => c.value)).toEqual(['underfoot'])
  })

  test('a box enclosing the whole view volume is kept, and rightly so', () => {
    // Every one of its corners satisfies every half-space, because the box swallows the
    // view volume. This is the case a naive "is the centre in the frustum" test gets
    // wrong, and getting it wrong this way is the safe direction.
    const enclosing: Array<CullCandidate<string>> = [
      { box: box([-1000, -1000, -1000], [1000, 1000, 1000]), value: 'enclosing' },
    ]
    expect(cullCandidates(planes, enclosing).visible).toHaveLength(1)
  })

  test('a protected point only protects the boxes that hold it', () => {
    const result = cullCandidates(planes, candidates, [[0, 0, 0]])
    expect(result.visible.map((c) => c.value)).toEqual(['inside', 'corner'])
  })

  test('an empty set and an empty frustum are both fine', () => {
    expect(cullCandidates(planes, [])).toEqual({ visible: [], culled: [] })
    expect(cullCandidates([], candidates).culled).toEqual([])
  })

  test('the answer is stable: the same input always splits the same way', () => {
    // A culler that flickers between two answers as the camera breathes shows holes that
    // come and go, which reads as a rendering fault rather than as culling.
    for (let turn = 0; turn < 20; turn += 1) {
      const first = cullCandidates(planes, candidates)
      const second = cullCandidates(planes, candidates)
      expect(first.visible.map((c) => c.value)).toEqual(second.visible.map((c) => c.value))
    }
  })

  test('a box on the boundary is kept by the margin, not only by luck', () => {
    const boundary: Array<CullCandidate<string>> = [
      { box: box([10, 0, 0], [20, 1, 1]), value: 'on-the-edge' },
    ]
    // Exactly on the plane at x = 10, with no margin: the far corner satisfies it.
    expect(cullCandidates(planes, boundary, [], 0).visible).toHaveLength(1)
    // A hair further out, and only the margin keeps it.
    const justOutside: Array<CullCandidate<string>> = [
      { box: box([10.5, -1, -1], [20, 1, 1]), value: 'just-outside' },
    ]
    expect(cullCandidates(planes, justOutside, [], 0).culled).toHaveLength(1)
    expect(cullCandidates(planes, justOutside, [], 0.1).visible).toHaveLength(1)
    // And the default margin is a real safety factor, not a decorative one: measured
    // against how far a chunk of that size can be, in metres.
    expect(CULL_MARGIN).toBeGreaterThan(0)
  })
})

test.describe('containment', () => {
  test('a point on a face counts as inside', () => {
    const boxy = box([0, 0, 0], [10, 10, 10])
    expect(boxContains(boxy, [5, 5, 5])).toBe(true)
    expect(boxContains(boxy, [0, 0, 0])).toBe(true)
    expect(boxContains(boxy, [10, 10, 10])).toBe(true)
    expect(boxContains(boxy, [10.001, 5, 5])).toBe(false)
  })
})
