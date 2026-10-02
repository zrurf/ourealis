/*
 * Trajectory line geometry: the polyline the replay draws.
 *
 * The line is a line list — two vertices per segment, one colour each — and the height a
 * point is drawn at is the higher of the sample's own altitude and the surface under it.
 * Both are pinned here because both were defects: a line at the raw sample altitude sinks
 * under a drawn roof and disappears in patches, and a line whose segments share vertices
 * cannot carry a colour per segment.
 */
import { expect, test } from '@playwright/test'
import {
  channelValue,
  drawHeight,
  pointAtTime,
  sampleTrajectory,
  trajectoryGeometry,
  type TrajectoryPoint,
} from '../../src/render/trajectoryGeometry'

/** A run of `count` samples a metre apart in x, at a constant altitude. */
function points(count: number, z = 100): TrajectoryPoint[] {
  return Array.from({ length: count }, (_, index) => ({
    time_s: index,
    x: index * 10,
    y: 0,
    z,
    value: index,
  }))
}

test('a trajectory of n points is n-1 segments of two vertices each', () => {
  const geometry = trajectoryGeometry(points(4))
  expect(geometry.positions.length).toBe(3 * 6)
  expect(geometry.indices.length).toBe(3 * 2)
  expect(geometry.colors.length).toBe(3 * 8)
  // Each segment owns its two vertices, so no vertex is shared between segments.
  const seen = new Set<number>()
  for (const index of geometry.indices) {
    expect(seen.has(index)).toBe(false)
    seen.add(index)
  }
})

test('a point under the drawn surface is lifted to it, and one above keeps its altitude', () => {
  const point: TrajectoryPoint = { time_s: 0, x: 10, y: 20, z: 105, value: 0 }
  // Surface below the sample: the sample's own altitude wins.
  expect(drawHeight(point, { surface: () => 100, lift: 0.25 })).toBe(105)
  // Surface above the sample (a flattened roof under the drawn track): the surface wins.
  expect(drawHeight(point, { surface: () => 120, lift: 0.25 })).toBeCloseTo(120.25, 6)
  // No surface known: the sample's altitude alone.
  expect(drawHeight(point, { surface: () => null })).toBe(105)
  // Nothing below the floor is ever drawn.
  expect(drawHeight({ ...point, z: -5 }, { minZ: 0 })).toBe(0)
})

test('segment colours come from the channel value, through the shared scale', () => {
  const geometry = trajectoryGeometry(points(3, 100), {}, (point) => [point.value / 10, 0, 0])
  // Two segments: segment 0 carries points 0 and 1, segment 1 carries points 1 and 2.
  const red = (slot: number): number => geometry.colors[slot * 4] ?? 0
  expect(red(0)).toBeCloseTo(0, 6)
  expect(red(1)).toBeCloseTo(0.1, 6)
  expect(red(2)).toBeCloseTo(0.1, 6)
  expect(red(3)).toBeCloseTo(0.2, 6)
  // The alpha of every vertex is opaque; a translucent trajectory has never been drawn.
  for (const slot of [0, 1, 2, 3]) {
    expect(geometry.colors[slot * 4 + 3]).toBe(1)
  }
})

test('the nearest point to a time is found on either side of it', () => {
  const list = points(5)
  expect(pointAtTime(list, -1)).toBe(0)
  expect(pointAtTime(list, 2.2)).toBe(2)
  expect(pointAtTime(list, 3.6)).toBe(4)
  expect(pointAtTime([], 1)).toBe(-1)
})

test('a sample is read through the channel that is asked for', () => {
  const sample = {
    time_s: 1,
    position: { x: 1, y: 2 },
    z: 3,
    speed: 4,
    grade: 0.5,
    kappa_eff: 0.01,
  } as Parameters<typeof channelValue>[0]
  expect(channelValue(sample, 'speed')).toBe(4)
  expect(channelValue(sample, 'grade')).toBe(0.5)
  expect(channelValue(sample, 'curvature')).toBe(0.01)
  expect(channelValue(sample, 'height')).toBe(3)
  const mapped = sampleTrajectory([sample], 'height')
  expect(mapped[0]?.value).toBe(3)
  expect(mapped[0]?.x).toBe(1)
  expect(mapped[0]?.y).toBe(2)
})
