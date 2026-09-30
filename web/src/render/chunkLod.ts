/*
 * Chunk geometry at fewer triangles.
 *
 * A level of a large map is dozens of chunks, and each chunk is a grid of cells, so the
 * whole surface framed at once is millions of triangles — every one of them submitted on
 * every frame to draw a hillside that occupies a few dozen pixels. The map already answers
 * "how much detail" with its own pyramid of levels; what it does not answer is "this chunk
 * is four hundred metres away and a hundred and twenty-eight cells across", which no level
 * can express because the reader is looking at the whole map rather than at one corner.
 *
 * This is the missing half: keep the same grid, drop the triangles that cannot be seen.
 * The simplifier is meshoptimizer's edge-collapse pass, which is the reason a decimation
 * looks like the surface rather than like a coarser version of it — it removes a vertex by
 * moving its neighbours onto it, where dropping every fourth triangle leaves a visible
 * diagonal seam.
 *
 * The rules are the same as the culler's, for the same reason. A wrong decimation shows up
 * as a hole in the ground, and a hole in the ground looks exactly like a bug in the
 * renderer. So a chunk that cannot be simplified safely is returned untouched: the
 * original geometry is always a correct answer, only a slower one.
 */
import { MeshoptSimplifier } from 'meshoptimizer/simplifier'

/** Geometry of a chunk, in the layout `MeshData` carries. */
export interface ChunkGeometry {
  /** Vertex positions, `x, y, z` per vertex. */
  positions: Float32Array
  /** Triangle indices, three per triangle. */
  indices: Uint32Array
}

/** What a decimation achieved, for the test surface and for a diagnostic line. */
export interface SimplifyReport {
  /** Triangles the chunk was drawn with before. */
  from: number
  /** Triangles it is drawn with after. */
  to: number
  /** Largest error the simplifier reported, as a fraction of the chunk's own size. */
  error: number
  /** False when the chunk was returned untouched. */
  simplified: boolean
}

/** Below this a chunk is left alone: a grid this small is not worth the build cost. */
const MIN_TRIANGLES = 64

/**
 * The fewest triangles any chunk is reduced to.
 *
 * A floor rather than a ratio alone, because a ratio on a small chunk lands on a handful
 * of triangles and the surface visibly facets long before the pixels could tell.
 */
const MIN_KEPT_TRIANGLES = 24

/**
 * Decimates a chunk, or returns it untouched.
 *
 * `ratio` is the share of the triangles to keep, in `(0, 1]`. Awaiting this is cheap after
 * the first call: the simplifier is an asm.js module that has to be instantiated once, and
 * that happens behind a promise.
 */
export async function simplifyChunk(
  geometry: ChunkGeometry,
  ratio: number,
): Promise<{ geometry: ChunkGeometry; report: SimplifyReport }> {
  const triangles = Math.floor(geometry.indices.length / 3)
  const unchanged: SimplifyReport = {
    from: triangles,
    to: triangles,
    error: 0,
    simplified: false,
  }
  if (
    !(ratio > 0) ||
    ratio >= 1 ||
    triangles < MIN_TRIANGLES ||
    geometry.positions.length % 3 !== 0 ||
    geometry.indices.length % 3 !== 0
  ) {
    return { geometry, report: unchanged }
  }
  // The border is locked because a chunk is drawn next to others: collapsing a vertex on
  // the shared edge would tear the seam open, and a torn seam is the failure a reader
  // notices first and attributes to the map rather than to the renderer.
  if (Math.round(triangles * ratio) >= triangles) {
    return { geometry, report: unchanged }
  }
  try {
    await MeshoptSimplifier.ready
    if (MeshoptSimplifier.supported === false) {
      return { geometry, report: unchanged }
    }
    const target = Math.max(MIN_KEPT_TRIANGLES * 3, Math.round(triangles * ratio) * 3)
    const [indices, error] = MeshoptSimplifier.simplify(
      geometry.indices,
      geometry.positions,
      3,
      target,
      // A bound rather than a request: past this the surface starts to deviate from the
      // ground a reader would walk on, and the honesty is worth more than the triangles.
      0.01,
      ['LockBorder'],
    )
    if (indices.length < 3 || indices.length >= geometry.indices.length) {
      return { geometry, report: unchanged }
    }
    return {
      geometry: compact(geometry, indices),
      report: {
        from: triangles,
        to: Math.floor(indices.length / 3),
        error: Number.isFinite(error) ? error : 0,
        simplified: true,
      },
    }
  } catch {
    // The simplifier asserts rather than returning when a target is unreachable — a
    // chunk asked to drop five per cent of its triangles is one of those — and a build
    // without the module throws on load. Either way the chunk is returned as it was,
    // which is always a correct answer and only a slower one.
    return { geometry, report: unchanged }
  }
}

/** What the remap holds for a vertex no surviving triangle points at. */
const UNUSED = 0xffffffff

/**
 * Rewrites a decimated chunk into a vertex space with nothing orphaned in it.
 *
 * Edge collapse leaves the vertices nothing references still sitting in the buffer, so
 * without this a "decimated" chunk would carry a quarter of the triangles over all of the
 * vertices — the saving the whole exercise is for would not be made.
 *
 * The remap is built here rather than taken from the library: `generatePositionRemap` is
 * handed positions alone and so has no way to know which of them a surviving triangle still
 * points at, and returns the identity. Reading the surviving indices is the only way to
 * find out, and it is a single pass.
 */
function compact(source: ChunkGeometry, simplified: Uint32Array): ChunkGeometry {
  const vertices = Math.floor(source.positions.length / 3)
  const remap = new Uint32Array(vertices).fill(UNUSED)
  let kept = 0
  for (const original of simplified) {
    if (original < vertices && remap[original] === UNUSED) {
      remap[original] = kept
      kept += 1
    }
  }
  if (kept === 0 || kept >= vertices) {
    // Nothing was orphaned, so the original buffer is already the compacted one.
    return { positions: source.positions.slice(), indices: simplified.slice() }
  }
  const positions = new Float32Array(kept * 3)
  for (let vertex = 0; vertex < vertices; vertex += 1) {
    const target = remap[vertex] ?? UNUSED
    if (target === UNUSED) {
      continue
    }
    const from = vertex * 3
    const to = target * 3
    positions[to] = source.positions[from] ?? 0
    positions[to + 1] = source.positions[from + 1] ?? 0
    positions[to + 2] = source.positions[from + 2] ?? 0
  }
  const indices = new Uint32Array(simplified.length)
  for (let i = 0; i < simplified.length; i += 1) {
    const original = simplified[i] ?? 0
    const target = remap[original] ?? UNUSED
    // A triangle pointing at a vertex the collapse orphaned cannot happen — the simplifier
    // only emits indices it kept — but an index outside the array would, and that is a
    // renderer crash rather than a wrong picture.
    indices[i] = target === UNUSED ? 0 : target
  }
  return { positions, indices }
}

/**
 * The ratio a chunk at a distance should keep.
 *
 * Falls away with distance and stops at {@link MIN_KEPT_TRIANGLES} worth: close to the
 * camera a chunk is the subject and keeps everything; far away it is a few dozen pixels of
 * hillside and keeps enough to read its shape. The curve is the square of the normalised
 * distance, so the reduction is gentle where detail is legible and steep where it is not.
 */
export function lodRatioFor(distanceM: number, nearM: number, farM: number): number {
  if (!(farM > nearM) || !(distanceM > nearM)) {
    return 1
  }
  if (distanceM >= farM) {
    return 0
  }
  const t = (distanceM - nearM) / (farM - nearM)
  return 1 - t * t * 0.9
}

/**
 * The same decimation, applied to a chunk that is already built.
 *
 * This is the form the renderer uses, and it differs from {@link simplifyChunk} in one
 * deliberate way: it rewrites the *index buffer* and leaves every vertex in place. A built
 * chunk carries a normal, a colour and a texture coordinate per vertex, all derived from
 * the cell grid, and dropping the vertices a collapse orphaned would mean re-deriving them
 * — which is where a decimation starts tearing seams and inverting normals. Keeping the
 * vertices costs some memory and saves none of the risk, and the memory is not what the
 * per-frame cost is made of: it is the triangles submitted.
 *
 * The caller gets back exactly the geometry it passed in when the decimation does not
 * apply, so a chunk that cannot be reduced is a chunk that was never at risk.
 */
export async function decimateIndices(
  positions: Float32Array,
  indices: Uint32Array,
  ratio: number,
): Promise<{ indices: Uint32Array; report: SimplifyReport }> {
  const triangles = Math.floor(indices.length / 3)
  const unchanged: SimplifyReport = { from: triangles, to: triangles, error: 0, simplified: false }
  if (
    !(ratio > 0) ||
    ratio >= 1 ||
    triangles < MIN_TRIANGLES ||
    positions.length % 3 !== 0 ||
    indices.length % 3 !== 0
  ) {
    return { indices, report: unchanged }
  }
  try {
    await MeshoptSimplifier.ready
    if (MeshoptSimplifier.supported === false) {
      return { indices, report: unchanged }
    }
    const target = Math.max(MIN_KEPT_TRIANGLES * 3, Math.round(triangles * ratio) * 3)
    const [result, error] = MeshoptSimplifier.simplify(indices, positions, 3, target, 0.01, [
      'LockBorder',
    ])
    if (result.length < 3 || result.length >= indices.length) {
      return { indices, report: unchanged }
    }
    return {
      indices: result.slice(),
      report: {
        from: triangles,
        to: Math.floor(result.length / 3),
        error: Number.isFinite(error) ? error : 0,
        simplified: true,
      },
    }
  } catch {
    return { indices, report: unchanged }
  }
}
