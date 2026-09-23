/*
 * The visual language of the map: patterns, materials and arrows.
 *
 * These are the parts a reader cannot check by reading the code — does a road read as a road, do
 * two cells of the same material meet without a seam, does an arrow point where the field says —
 * so the invariants they rest on are pinned here: a pattern is a bounded modulation around 1, it
 * is a function of *world* position rather than of a cell, a material table names its values, and
 * an arrow's head lies along its heading.
 */
import { expect, test } from '@playwright/test'
import {
  PATTERN_STRENGTH,
  applyPattern,
  patternAt,
  patternHasTexture,
  type SurfacePattern,
} from '../../src/render/patterns'
import {
  SURFACE_MATERIALS,
  featureDimensions,
  layerSurfacePlan,
  materialForValue,
} from '../../src/render/surfaces'
import { directionArrows, directionGrid } from '../../src/render/direction'
import { metresPerPixelAt, panTargetByPixels } from '../../src/render/panTarget'
import { arcTable, pointAtArc } from '../../src/render/flow'
import { meaningOf } from '../../src/render/inspect'
import {
  LAYER_DIRECTION,
  LAYER_ELEVATION,
  LAYER_HARD_FORBIDDEN,
  LAYER_EDT,
  LAYER_SLOPE,
} from '../../src/types/map'

/** A flat ground ten metres up, for the arrow and drape tests. */
function flatSurface(): number {
  return 10
}

/** Every pattern the viewer can draw. */
const PATTERNS: SurfacePattern[] = [
  'flat',
  'asphalt',
  'grass',
  'water',
  'building',
  'gravel',
  'hatch',
  'contour',
]

test.describe('patterns', () => {
  test('a pattern only modulates its colour, and only by a little', () => {
    for (const pattern of PATTERNS) {
      for (let index = 0; index < 200; index += 1) {
        const factor = patternAt(pattern, index * 0.37, index * 0.91, (index % 10) / 10)
        // The raw factor is a bounded shape (a hatch is 0.5 or 1.5); what a reader sees is the
        // *applied* shift, which stays inside the pattern's strength.
        expect(factor).toBeGreaterThanOrEqual(0.4)
        expect(factor).toBeLessThanOrEqual(1.6)
        const colour = applyPattern([120, 120, 120], pattern, index * 0.37, index * 0.91, 0.4)
        for (const channel of colour) {
          expect(Math.abs(channel - 120)).toBeLessThanOrEqual(
            Math.ceil(120 * PATTERN_STRENGTH * 0.65),
          )
        }
      }
    }
  })

  test('the pattern is a function of the world position, not of a cell', () => {
    // The same point is the same texture wherever it is asked from: that is what makes the
    // pattern run across cell and chunk boundaries instead of restarting in each of them.
    const a = patternAt('grass', 12.34, 56.78)
    const b = patternAt('grass', 12.34 + 500, 56.78 - 300)
    expect(patternAt('grass', 12.34, 56.78)).toBe(a)
    expect(Number.isFinite(b)).toBe(true)
    // A millimetre apart is a different sample, but a metre apart is still the same *kind* of
    // value: the pattern is continuous, not white noise per call.
    expect(
      Math.abs(patternAt('grass', 12.34, 56.78) - patternAt('grass', 12.35, 56.78)),
    ).toBeLessThan(0.4)
  })

  test('a flat pattern is exactly neutral, so a plain layer is a plain colour', () => {
    expect(patternAt('flat', 3, 4)).toBe(1)
    expect(patternHasTexture('flat')).toBe(false)
    expect(patternHasTexture('grass')).toBe(true)
  })

  test('applying a pattern keeps the hue and stays inside the byte range', () => {
    const colour = applyPattern([120, 180, 90], 'grass', 4.5, 2.5)
    expect(colour[1]).toBeGreaterThan(colour[0])
    expect(colour[1]).toBeGreaterThan(colour[2])
    for (const channel of applyPattern([250, 250, 250], 'contour', 1, 0.99)) {
      expect(channel).toBeLessThanOrEqual(255)
      expect(channel).toBeGreaterThanOrEqual(0)
    }
  })

  test('a hatch is a stripe, so its two states alternate', () => {
    const states = new Set<number>()
    for (let x = 0; x < 6; x += 0.2) {
      states.add(patternAt('hatch', x, 0) > 1 ? 1 : 0)
    }
    expect([...states].toSorted()).toEqual([0, 1])
  })

  test('a contour band depends on the value it is given', () => {
    // A twelfth of the range is the band; two values on either side of it differ.
    expect(patternAt('contour', 0, 0, 0)).toBeLessThan(1)
    expect(patternAt('contour', 0, 0, 0.06)).toBe(1)
  })
})

test.describe('surface materials', () => {
  test('a surface type dimension names its classes in the format order', () => {
    const dims = featureDimensions({
      dims: [
        { name: 'surface_type', layer_id: 4096, kind: 'Category' },
        { name: 'traffic', layer_id: 4097, kind: 'Scalar' },
      ],
    })
    expect(dims.map((dim) => dim.name)).toEqual(['surface_type', 'traffic'])
    const plan = layerSurfacePlan(4096, 'raster', dims)
    expect(plan.materials).toBe(SURFACE_MATERIALS)
    expect(materialForValue(plan, 0)?.labelKey).toBe('map.surface.road')
    expect(materialForValue(plan, 2)?.pattern).toBe('grass')
    // A value the table does not cover reads as no material rather than as the first one.
    expect(materialForValue(plan, 99)).toBeNull()
    // A scalar dimension carries no materials: it is a field, not a set of classes.
    expect(layerSurfacePlan(4097, 'raster', dims).materials).toBeNull()
  })

  test('a restriction mask is hatched and a cost field is contoured', () => {
    expect(layerSurfacePlan(LAYER_HARD_FORBIDDEN, 'bitmap', []).pattern).toBe('hatch')
    expect(layerSurfacePlan(LAYER_EDT, 'raster', []).pattern).toBe('contour')
    expect(layerSurfacePlan(LAYER_SLOPE, 'raster', []).pattern).toBe('contour')
    expect(layerSurfacePlan(LAYER_DIRECTION, 'raster', []).pattern).toBe('flat')
  })

  test('a schema that is missing or malformed is read as no dimensions', () => {
    expect(featureDimensions(null)).toEqual([])
    expect(featureDimensions({ dims: 'nonsense' })).toEqual([])
    expect(featureDimensions({ dims: [{ name: 'x' }] })).toEqual([])
  })
})

test.describe('the direction field', () => {
  test('an arrow points along its heading', () => {
    const [arrow] = directionArrows(
      [{ at: { x: 0, y: 0 }, angleDeg: 0, strength: 255 }],
      flatSurface,
      {
        lengthM: 10,
        liftM: 0,
      },
    )
    const shaft = arrow?.paths[0]
    expect(shaft?.[0]).toEqual([0, 10, 0])
    // Due east at ten metres: the head is at (10, 10) in the map plane, which is (10, ?, 10).
    expect(shaft?.[1]?.[0]).toBeCloseTo(10, 6)
    expect(shaft?.[1]?.[2]).toBeCloseTo(0, 6)
    // Two barbs, behind the head.
    expect(arrow?.paths).toHaveLength(3)
    for (const barb of arrow?.paths.slice(1) ?? []) {
      expect(barb[1]?.[0]).toBeLessThanOrEqual(shaft?.[1]?.[0] ?? 0)
    }
  })

  test('a heading of ninety degrees points north in the map plane', () => {
    const [arrow] = directionArrows(
      [{ at: { x: 5, y: 5 }, angleDeg: 90, strength: 128 }],
      flatSurface,
      {
        lengthM: 8,
      },
    )
    expect(arrow?.paths[0]?.[1]?.[0]).toBeCloseTo(5, 6)
    expect(arrow?.paths[0]?.[1]?.[2]).toBeCloseTo(13, 6)
    expect(arrow?.strength).toBeCloseTo(128 / 255, 3)
  })

  test('a cell with no constraint gets no arrow', () => {
    expect(
      directionArrows([{ at: { x: 0, y: 0 }, angleDeg: 0, strength: 0 }], flatSurface),
    ).toEqual([])
  })

  test('the field is sampled on its own grid, skipping cells without a constraint', () => {
    const samples = directionGrid({ x: 0, y: 0 }, { width: 50, height: 50 }, 25, (x, y) =>
      x === 25 && y === 25 ? null : { at: { x, y }, angleDeg: 0, strength: 10 },
    )
    // 3x3 grid positions, minus the one that answered `null`.
    expect(samples).toHaveLength(8)
  })
})

test.describe('panning the ground', () => {
  test('dragging right moves the ground right, for any camera azimuth', () => {
    // Azimuth -90 degrees: the camera looks north, so screen-right is east.
    const east = panTargetByPixels({ x: 0, z: 0 }, 10, 0, 2, -Math.PI / 2, Math.PI / 4)
    expect(east.x).toBeCloseTo(-20, 6)
    expect(east.z).toBeCloseTo(0, 6)
    // Turning the camera a quarter turn turns the pan with it: from the east the screen's right
    // hand points along +z, so the same drag moves the ground that way.
    const turned = panTargetByPixels({ x: 0, z: 0 }, 10, 0, 2, 0, Math.PI / 4)
    expect(turned.x).toBeCloseTo(0, 6)
    expect(turned.z).toBeCloseTo(-20, 6)
  })

  test('a shallow view moves further per pixel than a top-down one', () => {
    const flatish = panTargetByPixels({ x: 0, z: 0 }, 0, 10, 1, -Math.PI / 2, Math.PI / 2.2)
    const overhead = panTargetByPixels({ x: 0, z: 0 }, 0, 10, 1, -Math.PI / 2, 0.2)
    expect(Math.abs(flatish.z)).toBeGreaterThan(Math.abs(overhead.z))
  })

  test('the ground resolution follows the radius and the field of view', () => {
    const near = metresPerPixelAt(100, 500, Math.PI / 4)
    const far = metresPerPixelAt(200, 500, Math.PI / 4)
    expect(far).toBeCloseTo(near * 2, 6)
    const narrow = metresPerPixelAt(100, 500, Math.PI / 8)
    expect(narrow).toBeLessThan(near)
  })
})

test.describe('what a cell means', () => {
  test('the layers the format defines are named, and the rest are not guessed at', () => {
    expect(meaningOf(LAYER_ELEVATION, 'raster')).toBe('elevation')
    expect(meaningOf(LAYER_SLOPE, 'raster')).toBe('slope')
    expect(meaningOf(LAYER_EDT, 'raster')).toBe('distance')
    expect(meaningOf(LAYER_HARD_FORBIDDEN, 'bitmap')).toBe('blocked')
    expect(meaningOf(LAYER_DIRECTION, 'raster')).toBe('direction')
    expect(meaningOf(0x1001, 'raster')).toBe('feature')
    expect(meaningOf(0x2501, 'region')).toBe('section')
    expect(meaningOf(0x9999, 'raster')).toBe('unknown')
  })
})

test.describe('travelling along a route', () => {
  const square = [
    { x: 0, y: 0 },
    { x: 10, y: 0 },
    { x: 10, y: 10 },
    { x: 0, y: 10 },
  ]

  test('the arc table measures the path, not the point count', () => {
    const table = arcTable(square)
    expect(table.lengthM).toBeCloseTo(30, 6)
    expect(table.arcs).toEqual([0, 10, 20, 30])
  })

  test('a marker is placed by distance, so it keeps its speed through short segments', () => {
    const table = arcTable([
      { x: 0, y: 0 },
      { x: 1, y: 0 },
      { x: 11, y: 0 },
    ])
    const quarter = pointAtArc(table, 3)
    // Three metres along a path whose first segment is one metre long.
    expect(quarter?.at.x).toBeCloseTo(3, 6)
    expect(quarter?.at.y).toBeCloseTo(0, 6)
    // Halfway through the second segment: 1 + 5 metres.
    const half = pointAtArc(table, 6)
    expect(half?.at.x).toBeCloseTo(6, 6)
  })

  test('the marker turns with the path', () => {
    const table = arcTable(square)
    expect(pointAtArc(table, 5)?.headingRad).toBeCloseTo(0, 6)
    expect(pointAtArc(table, 15)?.headingRad).toBeCloseTo(Math.PI / 2, 6)
    expect(pointAtArc(table, 25)?.headingRad).toBeCloseTo(Math.PI, 6)
  })

  test('it wraps, so the marker keeps flowing instead of vanishing at the goal', () => {
    const table = arcTable(square)
    // 30 is the whole path, so it is the start again; 35 is five metres into the first side.
    expect(pointAtArc(table, 30)?.at.x).toBeCloseTo(0, 6)
    expect(pointAtArc(table, 35)?.at).toEqual({ x: 5, y: 0 })
    // A negative arc — a marker running slightly behind its path — still lands on it: five metres
    // before the goal is the last side, where y is ten.
    expect(pointAtArc(table, -5)?.at).toEqual({ x: 5, y: 10 })
  })

  test('the segment a marker is on is reported, so its own data can be interpolated', () => {
    const table = arcTable(square)
    const sample = pointAtArc(table, 12)
    expect(sample?.index).toBe(1)
    expect(sample?.t).toBeCloseTo(0.2, 6)
  })

  test('a degenerate path has no direction to give', () => {
    expect(pointAtArc(arcTable([]), 1)).toBeNull()
    const single = pointAtArc(arcTable([{ x: 4, y: 5 }]), 3)
    expect(single?.at).toEqual({ x: 4, y: 5 })
  })
})
