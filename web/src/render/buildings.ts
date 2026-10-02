/*
 * Building footprints as prisms: the geometry behind the white model's blocks.
 *
 * The elevation layer carries a building mask in its second channel, and the height field
 * under a footprint is the building's roof — but drawn as a height field a block is a
 * staircase of cells with a skirt of interpolation around it, which is what made a campus
 * read as melted wax. This module turns the mask of one chunk into *prisms*: the mask is
 * dilated by a cell, traced into closed polygons, simplified back to the straight walls
 * the survey drew, and extruded from the ground to the roof. The drawn terrain keeps its
 * own flattened plateau under each prism — the prism's roof stands a hand's width above
 * it, so the two never fight for depth.
 *
 * Pure geometry: samplers in, typed arrays out, nothing from Babylon, so the shapes can be
 * tested without an engine. `render/buildingLayer.ts` hands the arrays to the scene.
 *
 * Buildings that cross a chunk boundary are traced per chunk and closed along the seam, so
 * no prism is duplicated: the two halves share the seam plane, whose walls end up inside
 * the merged silhouette where nothing can see them. Courtyards are filled — a white model
 * without a roof hole reads better than one with a polygon-with-hole triangulation.
 */
import type { CellSampler } from './cellSampler'
import type { ChunkGrid } from './terrainMesh'
import { terrace } from './shading'

/** Vertex data of the building prisms of one chunk, in world metres with `y` up. */
export interface BuildingMeshData {
  /** Vertex positions, `x, y, z` per vertex. */
  positions: Float32Array
  /** Triangle indices. */
  indices: Uint32Array
  /** Vertex normals. */
  normals: Float32Array
}

/** Inputs of {@link buildChunkBuildings}, the samplers the surface itself was built from. */
export interface BuildingChunkInput {
  /** Where the chunk's cells stand. */
  spec: ChunkGrid
  /** Building coverage per cell, `0` to `1`; a cell at `0.5` or more is built. */
  building: CellSampler
  /** Terrain elevation per cell, metres, roof heights included. */
  elevation: CellSampler
  /** Terrace step the surface draws with, metres; `0` leaves the surveyed heights. */
  terraceM: number
  /**
   * How far the roof sits above the height field's own plateau, metres.
   *
   * The terrain flattens a footprint to its highest cell; a prism roof at exactly that
   * height would coplanar-fight the plateau it covers.
   */
  roofLiftM?: number
  /** How far the prism's walls reach below the lowest ground around the footprint, metres. */
  sinkM?: number
}

/** Coverage at which a cell counts as built, the same threshold the terrain flattens by. */
const BUILT_THRESHOLD = 0.5

/** A component smaller than this many cells is mask noise, not a block. */
const MIN_COMPONENT_CELLS = 9

/** How far the prism grows past the mask, metres, whatever the level's cell size is. */
const MARGIN_M = 1.2

/**
 * Cell size above which no prism is built, metres.
 *
 * The mask blurs across a coarse level's own cells: at eight metres a pair of blocks
 * twenty metres apart is one blob, and one prism over the pair reads as a slab the size of
 * a city quarter — worse than the flattened height field it replaces. Levels that coarse
 * are overviews; the white mesa under them still marks the built-up ground.
 */
const MAX_CELL_M = 6

/** Smooths away at least a cell of staircase, and keeps a surveyed corner a corner. */
const SIMPLIFY_TOLERANCE_CELLS = 1.1

/** Builds the prisms of one chunk's building mask, or `null` where the mask has none. */
export function buildChunkBuildings(input: BuildingChunkInput): BuildingMeshData | null {
  const { spec, building, elevation, terraceM } = input
  const roofLift = input.roofLiftM ?? 0.15
  const sink = input.sinkM ?? 2
  const { columns, rows, cell, originX, originY, firstI, firstJ } = spec
  if (cell > MAX_CELL_M) {
    return null
  }

  // The chunk's own cells as a binary field. The shore vertices the mesh adds on internal
  // sides are surface, not mask, and a prism has no business standing on them.
  const inside = new Uint8Array(columns * rows)
  for (let row = 0; row < rows; row += 1) {
    for (let column = 0; column < columns; column += 1) {
      const value = building(firstI + column, firstJ + row)
      inside[row * columns + column] = value !== null && value >= BUILT_THRESHOLD ? 1 : 0
    }
  }

  // The prism must stand *outside* everything the height field paints white: the footprint
  // cells plus the soft band the mask bleeds into the ground beside it. The growth is in
  // metres — a cell of a coarse level is not the same margin as a cell of a fine one — and
  // never rounds away to nothing.
  const growth = Math.max(1, Math.round(MARGIN_M / Math.max(cell, 0.1)))
  let grown: Uint8Array = inside
  for (let pass = 0; pass < growth; pass += 1) {
    grown = dilate(grown, columns, rows)
  }
  fillHoles(grown, columns, rows)

  // One prism per connected component; specks the dilation cannot make legible are noise.
  const label = new Int32Array(columns * rows).fill(-1)
  const components: number[][] = []
  for (let at = 0; at < grown.length; at += 1) {
    if (grown[at] !== 1 || label[at] !== -1) {
      continue
    }
    const cells: number[] = []
    const id = components.length
    const stack = [at]
    label[at] = id
    while (stack.length > 0) {
      const current = stack.pop() ?? 0
      cells.push(current)
      const x = current % columns
      const y = Math.floor(current / columns)
      for (const [dx, dy] of [
        [1, 0],
        [-1, 0],
        [0, 1],
        [0, -1],
      ] as const) {
        const nx = x + dx
        const ny = y + dy
        if (nx < 0 || ny < 0 || nx >= columns || ny >= rows) {
          continue
        }
        const next = ny * columns + nx
        if (grown[next] === 1 && label[next] === -1) {
          label[next] = id
          stack.push(next)
        }
      }
    }
    if (cells.length >= MIN_COMPONENT_CELLS) {
      components.push(cells)
    }
  }
  if (components.length === 0) {
    return null
  }

  const heightOf = (column: number, row: number): number => {
    const value = elevation(firstI + column, firstJ + row)
    return terrace(value ?? 0, terraceM)
  }

  const positions: number[] = []
  const normals: number[] = []
  const indices: number[] = []
  for (const cells of components) {
    for (const ring of componentRings(cells, columns, rows)) {
      const simplified = simplifyRing(ring, cell * SIMPLIFY_TOLERANCE_CELLS)
      if (simplified.length < 3) {
        continue
      }
      let roof = Number.NEGATIVE_INFINITY
      let ground = Number.POSITIVE_INFINITY
      for (const at of cells) {
        const height = heightOf(at % columns, Math.floor(at / columns))
        roof = Math.max(roof, height)
        ground = Math.min(ground, height)
      }
      if (!Number.isFinite(roof) || !Number.isFinite(ground)) {
        continue
      }
      emitPrism(
        positions,
        normals,
        indices,
        simplified.map(([column, row]) => [originX + column * cell, originY + row * cell] as const),
        ground - sink,
        roof + roofLift,
      )
    }
  }
  if (indices.length === 0) {
    return null
  }
  return {
    positions: new Float32Array(positions),
    indices: new Uint32Array(indices),
    normals: new Float32Array(normals),
  }
}

/**
 * One prism between a polygon and two heights, appended to the running buffers.
 *
 * The winding follows Babylon's left-handed default: the side a triangle shows the camera
 * is the one its right-hand normal points *away* from. So the ring is wound counter-
 * clockwise in the (x, z) plane — the orientation whose right-hand normal points down, which
 * is what draws a roof from above — and each wall's vertex order is the one whose right-hand
 * normal points inward, which shows its outward face. The normal *attribute* is independent
 * of all that and always points out of the solid, where the light comes from. Walls carry
 * their own four vertices each, flat-shaded to their edge's normal, so a corner reads as a
 * corner. The base is open: it stands sunk into the ground, where nothing can see it.
 */
function emitPrism(
  positions: number[],
  normals: number[],
  indices: number[],
  ring: ReadonlyArray<readonly [number, number]>,
  baseY: number,
  roofY: number,
): void {
  let area = 0
  for (let index = 0; index < ring.length; index += 1) {
    const [ax, az] = ring[index] ?? [0, 0]
    const [bx, bz] = ring[(index + 1) % ring.length] ?? [0, 0]
    area += ax * bz - bx * az
  }
  const wound = area < 0 ? ring.toReversed() : ring
  for (let index = 0; index < wound.length; index += 1) {
    const next = (index + 1) % wound.length
    const [ax, az] = wound[index] ?? [0, 0]
    const [bx, bz] = wound[next] ?? [0, 0]
    // The wall's outward normal: with the ring wound counter-clockwise, this quarter turn of
    // the edge direction points away from the interior on every edge.
    let nx = bz - az
    let nz = -(bx - ax)
    const length = Math.hypot(nx, nz) || 1
    nx /= length
    nz /= length
    const wall = positions.length / 3
    positions.push(ax, baseY, az, bx, baseY, bz, bx, roofY, bz, ax, roofY, az)
    for (let corner = 0; corner < 4; corner += 1) {
      normals.push(nx, 0, nz)
    }
    indices.push(wall, wall + 1, wall + 2, wall, wall + 2, wall + 3)
  }
  const roof = positions.length / 3
  for (const [x, z] of wound) {
    positions.push(x, roofY, z)
    normals.push(0, 1, 0)
  }
  for (const [a, b, c] of earClip(wound)) {
    indices.push(roof + a, roof + b, roof + c)
  }
}

/**
 * Triangulates a ring for the roof, by ear clipping.
 *
 * Building rings are small (a simplified footprint is a handful of corners), so the
 * quadratic clip is cheaper and plainer than a monotone decomposition. Returns triples of
 * indices into the ring.
 */
function earClip(ring: ReadonlyArray<readonly [number, number]>): Array<[number, number, number]> {
  const count = ring.length
  if (count < 3) {
    return []
  }
  // The ring arrives counter-clockwise in (x, z) with `y` up, which is clockwise on the
  // plane the roof is triangulated in; crossing tests below use the ring's own orientation.
  const left = [...ring.keys()]
  const triangles: Array<[number, number, number]> = []
  let guard = count * count
  while (left.length > 3 && guard > 0) {
    guard -= 1
    let clipped = false
    for (let index = 0; index < left.length; index += 1) {
      const previous = left[(index + left.length - 1) % left.length] ?? 0
      const current = left[index] ?? 0
      const next = left[(index + 1) % left.length] ?? 0
      const a = ring[previous] ?? [0, 0]
      const b = ring[current] ?? [0, 0]
      const c = ring[next] ?? [0, 0]
      if (!isEar(a, b, c, left, ring)) {
        continue
      }
      triangles.push([previous, current, next])
      left.splice(index, 1)
      clipped = true
      break
    }
    if (!clipped) {
      // Degenerate ring (collinear or self-touching after simplification): fall back to a
      // fan, which is wrong only where the ring was already broken.
      break
    }
  }
  if (left.length === 3) {
    triangles.push([left[0] ?? 0, left[1] ?? 0, left[2] ?? 0])
  } else if (left.length > 3) {
    for (let index = 1; index + 1 < left.length; index += 1) {
      triangles.push([left[0] ?? 0, left[index] ?? 0, left[index + 1] ?? 0])
    }
  }
  return triangles
}

/** True when the corner `a, b, c` is convex and holds no other ring vertex. */
function isEar(
  a: readonly [number, number],
  b: readonly [number, number],
  c: readonly [number, number],
  left: readonly number[],
  ring: ReadonlyArray<readonly [number, number]>,
): boolean {
  const cross =
    (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])
  if (cross <= 0) {
    // The winding convention makes convex corners the positive half plane; a flat or reflex
    // corner is never an ear.
    return false
  }
  for (const index of left) {
    const p = ring[index] ?? [0, 0]
    if (p === a || p === b || p === c) {
      continue
    }
    if (pointInTriangle(p, a, b, c)) {
      return false
    }
  }
  return true
}

/** Twice the signed area of `p, q` about `r`: the side of `rq` that `p` lies on. */
function sideOf(px: number, py: number, qx: number, qy: number, rx: number, ry: number): number {
  return (px - rx) * (qy - ry) - (qx - rx) * (py - ry)
}

/** True when `p` lies inside (or on the edge of) the triangle `a, b, c`. */
function pointInTriangle(
  p: readonly [number, number],
  a: readonly [number, number],
  b: readonly [number, number],
  c: readonly [number, number],
): boolean {
  const d1 = sideOf(p[0], p[1], a[0], a[1], b[0], b[1])
  const d2 = sideOf(p[0], p[1], b[0], b[1], c[0], c[1])
  const d3 = sideOf(p[0], p[1], c[0], c[1], a[0], a[1])
  const negative = d1 < 0 || d2 < 0 || d3 < 0
  const positive = d1 > 0 || d2 > 0 || d3 > 0
  return !(negative && positive)
}

/**
 * The closed boundary loops of one component's cells, as `(column, row)` corner paths.
 *
 * Every side of an inside cell whose neighbour is not in the component is a unit edge,
 * directed so the interior stays on one side; the edges of a simply-connected component
 * chain into exactly one loop, and the corners where two cells touch diagonally resolve by
 * taking any continuation, which a dilated mask has already smoothed away in practice.
 */
function componentRings(
  cells: readonly number[],
  columns: number,
  rows: number,
): Array<Array<[number, number]>> {
  const member = new Set(cells)
  const isMember = (column: number, row: number): boolean =>
    column >= 0 && row >= 0 && column < columns && row < rows && member.has(row * columns + column)
  // Directed unit edges on the corner lattice: `start → end`, interior on the walker's left
  // in a y-down reading of the grid.
  const edges = new Map<number, number[]>()
  const key = (x: number, y: number): number => y * (columns + 1) + x
  const add = (from: [number, number], to: [number, number]): void => {
    const start = key(from[0], from[1])
    const ends = edges.get(start) ?? []
    ends.push(key(to[0], to[1]))
    edges.set(start, ends)
  }
  for (const at of cells) {
    const x = at % columns
    const y = Math.floor(at / columns)
    if (!isMember(x, y - 1)) add([x, y], [x + 1, y])
    if (!isMember(x + 1, y)) add([x + 1, y], [x + 1, y + 1])
    if (!isMember(x, y + 1)) add([x + 1, y + 1], [x, y + 1])
    if (!isMember(x - 1, y)) add([x, y + 1], [x, y])
  }
  const rings: Array<Array<[number, number]>> = []
  while (edges.size > 0) {
    const startKey = edges.keys().next().value
    if (startKey === undefined) {
      break
    }
    const ring: Array<[number, number]> = []
    let current = startKey
    while (true) {
      const ends = edges.get(current)
      const next = ends?.shift()
      if (ends !== undefined && ends.length === 0) {
        edges.delete(current)
      }
      if (next === undefined) {
        break
      }
      ring.push([current % (columns + 1), Math.floor(current / (columns + 1))])
      current = next
      if (current === startKey) {
        break
      }
    }
    if (ring.length >= 3) {
      rings.push(ring)
    }
  }
  return rings
}

/**
 * Reduces a boundary loop to its corners, by Douglas–Peucker on the open path.
 *
 * The traced loop is a staircase of unit cell edges; the tolerance of about a cell keeps
 * the surveyed straight wall straight and drops the steps between its corners. The loop is
 * split at its two extremal points first, so a closed run does not lose both its ends.
 */
function simplifyRing(
  ring: ReadonlyArray<[number, number]>,
  tolerance: number,
): Array<[number, number]> {
  if (ring.length <= 4) {
    return [...ring]
  }
  // The two points farthest apart along the ring split it into two open runs, each of
  // which is simplified on its own so the closed loop cannot collapse.
  let worstFrom = 0
  let worstTo = 0
  let best = -1
  for (let from = 0; from < ring.length; from += 1) {
    for (let to = from + 1; to < ring.length; to += 1) {
      const a = ring[from] ?? [0, 0]
      const b = ring[to] ?? [0, 0]
      const distance = Math.hypot(a[0] - b[0], a[1] - b[1])
      if (distance > best) {
        best = distance
        worstFrom = from
        worstTo = to
      }
    }
  }
  const first = douglasPeucker(ring.slice(worstFrom, worstTo + 1), tolerance)
  const second = douglasPeucker(
    [...ring.slice(worstTo), ...ring.slice(0, worstFrom + 1)],
    tolerance,
  )
  const merged = [...first.slice(0, -1), ...second.slice(0, -1)]
  // Collinear runts left by the staircase disappear here rather than as sliver triangles.
  const cleaned: Array<[number, number]> = []
  for (const point of merged) {
    const previous = cleaned[cleaned.length - 1]
    if (
      previous !== undefined &&
      Math.hypot(point[0] - previous[0], point[1] - previous[1]) < tolerance / 2
    ) {
      continue
    }
    cleaned.push(point)
  }
  return cleaned
}

/** Douglas–Peucker over an open path. */
function douglasPeucker(
  path: ReadonlyArray<[number, number]>,
  tolerance: number,
): Array<[number, number]> {
  if (path.length <= 2) {
    return [...path]
  }
  const [ax, ay] = path[0] ?? [0, 0]
  const [bx, by] = path[path.length - 1] ?? [0, 0]
  const dx = bx - ax
  const dy = by - ay
  const lengthSquared = dx * dx + dy * dy
  let worstIndex = -1
  let worst = tolerance
  if (lengthSquared > 0) {
    for (let index = 1; index + 1 < path.length; index += 1) {
      const [px, py] = path[index] ?? [0, 0]
      const t = ((px - ax) * dx + (py - ay) * dy) / lengthSquared
      const clamped = Math.min(1, Math.max(0, t))
      const distance = Math.hypot(px - (ax + clamped * dx), py - (ay + clamped * dy))
      if (distance > worst) {
        worst = distance
        worstIndex = index
      }
    }
  }
  if (worstIndex === -1) {
    return [path[0] ?? [0, 0], path[path.length - 1] ?? [0, 0]]
  }
  const head = douglasPeucker(path.slice(0, worstIndex + 1), tolerance)
  const tail = douglasPeucker(path.slice(worstIndex), tolerance)
  return [...head.slice(0, -1), ...tail]
}

/** Binary dilation by one cell over the eight-neighbourhood, into a fresh field. */
function dilate(field: Uint8Array, columns: number, rows: number): Uint8Array {
  const out = new Uint8Array(field.length)
  for (let row = 0; row < rows; row += 1) {
    for (let column = 0; column < columns; column += 1) {
      let spread = 0
      for (let dy = -1; dy <= 1 && spread === 0; dy += 1) {
        for (let dx = -1; dx <= 1 && spread === 0; dx += 1) {
          const x = column + dx
          const y = row + dy
          if (x < 0 || y < 0 || x >= columns || y >= rows) {
            continue
          }
          spread = field[y * columns + x] ?? 0
        }
      }
      out[row * columns + column] = spread
    }
  }
  return out
}

/**
 * Fills the holes a dilation cannot close: a building with a courtyard reads as one block.
 *
 * A hole is a patch of empty cells no border-connected empty run reaches; flooding the
 * empties from the grid border marks every cell that is genuinely outside, and the rest
 * becomes building.
 */
function fillHoles(field: Uint8Array, columns: number, rows: number): void {
  const outside = new Uint8Array(field.length)
  const stack: number[] = []
  for (let column = 0; column < columns; column += 1) {
    stack.push(column, (rows - 1) * columns + column)
  }
  for (let row = 0; row < rows; row += 1) {
    stack.push(row * columns, row * columns + columns - 1)
  }
  while (stack.length > 0) {
    const at = stack.pop() ?? 0
    if (at < 0 || at >= field.length || outside[at] === 1 || field[at] === 1) {
      continue
    }
    outside[at] = 1
    const x = at % columns
    const y = Math.floor(at / columns)
    if (x > 0) stack.push(at - 1)
    if (x < columns - 1) stack.push(at + 1)
    if (y > 0) stack.push(at - columns)
    if (y < rows - 1) stack.push(at + columns)
  }
  for (let at = 0; at < field.length; at += 1) {
    if (field[at] === 0 && outside[at] === 0) {
      field[at] = 1
    }
  }
}
