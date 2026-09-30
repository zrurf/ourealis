/*
 * Feature layers as textures: the pixel and geometry conversion.
 *
 * A raster layer is drawn as one RGBA texture per chunk, draped over the elevation: the
 * mesh reuses the terrain's own vertex heights so the overlay follows the ground instead
 * of floating over it. Category-shaped data takes the discrete palette (with an alpha that
 * hides zero, because a sparse layer stores its absence as zero), a mask takes one
 * warning colour, and scalar data takes a ramp.
 *
 * Pure: no Babylon, so the mapping can be tested without a GPU, the texture coordinates
 * can be checked against the mesh that will carry them, and the same numbers feed a
 * legend. `render/layers.ts` uploads the result.
 */
import type { ColorMapping } from '@/types/map'
import type { Rgb } from '@/types/colormap'
import {
  SCALAR_RAMPS,
  colorForCategory,
  normalize,
  rampAt,
  rampTable,
  valueRange,
} from '@/types/colormap'
import type { CellSampler } from './cellSampler'
import { patternFactor } from './patterns'
import { materialForValue, type LayerSurfacePlan } from './surfaces'
import { buildingMask } from './terrainMesh'
import type { ChunkGrid } from './terrainMesh'

/** RGBA texture of one layer chunk, in the chunk's own `[y][x]` order. */
export interface LayerTextureData {
  /** Pixels, four bytes per cell, row-major from the chunk's first row. */
  pixels: Uint8ClampedArray
  /** Texture width in cells. */
  width: number
  /** Texture height in cells. */
  height: number
  /**
   * Whether any pixel is translucent.
   *
   * A raw texture does not carry the flag, and a material that is not told reads the alpha
   * as opaque — which is how an enabled layer covered the whole map with a black sheet:
   * every *other* cell is fully transparent, and the shader drew it as paint.
   */
  hasAlpha: boolean
}

/** Options of {@link layerTextureData}. */
export interface LayerTextureOptions {
  /** How the samples are coloured. */
  mapping: ColorMapping
  /** Value range the ramp spans; computed from the samples when absent. */
  range?: { min: number; max: number }
  /** Alpha of a cell whose value is missing or zero; `0` hides it, `1` fills the mesh. */
  emptyAlpha?: number
  /** Colour a `flag` mapping paints a set cell with. */
  flag?: Rgb
  /**
   * Pattern the cells are textured with, and the materials a category layer names.
   *
   * A pattern is sampled per *texel* at its map position, so it tiles across cells instead
   * of restarting in each one — which is what makes asphalt look like asphalt rather than
   * like a checkerboard.
   */
  surface?: LayerSurfacePlan | null
  /** Texels per cell; `1` draws one colour per cell. */
  texelsPerCell?: number
  /**
   * Reader of the building mask, so a drape can leave footprints alone.
   *
   * A building is not ground and must not be painted as ground. The drape covers the
   * surface, so a footprint left in it would show the material of the cell it stands on —
   * and worse, its lower wall vertices belong to the cells *beside* the footprint, so the
   * foot of every building would be painted with the road or grass next to it. Cutting the
   * footprint out of the drape is what leaves the white model underneath visible.
   *
   * The mask is the one the terrain itself uses, so the two agree about the boundary to
   * the cell rather than each drawing its own approximation of it.
   */
  building?: CellSampler
}

/**
 * Colour of one sample under a mapping, or `null` where the cell carries nothing.
 *
 * A zero is treated as "no feature here" for every mapping: the layers that arrive
 * sparse store their absence as zero, and painting those cells would cover the ground
 * with the value that means "no ground". A `flag` layer — a restriction mask — is the
 * case this exists for: only the cells that block a runner are painted.
 */
export function sampleColour(
  value: number | undefined,
  mapping: ColorMapping,
  range: { min: number; max: number },
  flag: Rgb = [183, 121, 31],
): Rgb | null {
  if (value === undefined || !Number.isFinite(value) || value === 0) {
    return null
  }
  if (mapping === 'flag') {
    return flag
  }
  if (mapping === 'category' || mapping === 'material') {
    // A material mapping without a material table — the panel offers it for a layer that has
    // none — falls back to the palette, which is what the values mean then.
    return colorForCategory(value)
  }
  return rampAt(SCALAR_RAMPS[mapping], normalize(value, range.min, range.max))
}

/**
 * Converts one chunk's cells into RGBA bytes, over the mesh's own grid.
 *
 * The pixels follow the *mesh* rather than the chunk: the texture of a chunk whose mesh
 * reaches one cell into its neighbour carries that neighbour's cell too, so a texel always
 * stands for the vertex sampled at its centre. Only the first channel is shown: a
 * multi-channel raster holds one channel per preference dimension, and a viewer draws one
 * at a time rather than inventing a blend.
 */
export function layerTextureData(
  spec: ChunkGrid,
  sample: CellSampler,
  options: LayerTextureOptions,
): LayerTextureData {
  const perCell = Math.max(1, Math.round(options.texelsPerCell ?? 1))
  const width = Math.max(1, spec.columns * perCell)
  const height = Math.max(1, spec.rows * perCell)
  const pixels = new Uint8ClampedArray(width * height * 4)
  const emptyAlpha = options.emptyAlpha ?? 0
  const range = options.range ?? sampledRange(spec, sample)
  const surface = options.surface ?? null
  const pattern = surface !== null && surface.materials === null ? surface.pattern : 'flat'
  // The scalar ramp is sampled once for the whole chunk rather than interpolated per
  // cell, and the colour is written straight into the buffer. A 128-cell chunk at four
  // texels a cell is 262 144 writes; a fresh colour array for each of them was the
  // allocation churn that made a draped layer cost seconds.
  const ramp =
    options.mapping in SCALAR_RAMPS
      ? rampTable(SCALAR_RAMPS[options.mapping as keyof typeof SCALAR_RAMPS])
      : null
  // Where the footprint is, as a weight. Multiplied into the alpha rather than tested, so
  // the drape's edge and the white model's edge fade together instead of one stair-stepping
  // over the other.
  const built = options.building === undefined ? null : buildingMask(spec, options.building).weight
  const hasPattern = pattern !== 'flat' || surface?.materials != null
  // A drape with a mask can always have a cut edge, so it is treated as blended from the
  // start rather than only if a cell happens to come out translucent.
  let hasAlpha = built !== null
  for (let cellRow = 0; cellRow < spec.rows; cellRow += 1) {
    for (let cellColumn = 0; cellColumn < spec.columns; cellColumn += 1) {
      const value = cellValue(spec, sample, cellColumn, cellRow)
      // A named material wins over the mapping: a road is drawn as a road.
      const material = surface === null ? null : materialForValue(surface, value)
      const named = material?.colour ?? null
      const base =
        named ??
        (ramp === null
          ? sampleColour(value, options.mapping, range, options.flag)
          : rampColour(value, range, ramp))
      if (base === null) {
        const alpha = Math.round(emptyAlpha * 255)
        hasAlpha ||= alpha < 255
        for (let ty = 0; ty < perCell; ty += 1) {
          for (let tx = 0; tx < perCell; tx += 1) {
            const pixel = ((cellRow * perCell + ty) * width + cellColumn * perCell + tx) * 4
            pixels[pixel + 3] = alpha
          }
        }
        continue
      }
      // How much of this cell the drape paints. A footprint is left to the white model
      // under it, and a cell half on the edge of one is painted half as much, which is the
      // sub-cell edge the mask carries and the format's own data cannot express.
      const cellAlpha =
        built === null
          ? 255
          : Math.round(255 * (1 - (built[cellRow * spec.columns + cellColumn] ?? 0)))
      if (cellAlpha < 255) {
        hasAlpha = true
      }
      const cellPattern = material?.pattern ?? pattern
      // A flat cell is one write per texel; only a patterned one needs the per-texel
      // position, which is what makes a hatch tile across cells.
      if (!hasPattern) {
        for (let ty = 0; ty < perCell; ty += 1) {
          let pixel = ((cellRow * perCell + ty) * width + cellColumn * perCell) * 4
          for (let tx = 0; tx < perCell; tx += 1) {
            pixels[pixel] = base[0]
            pixels[pixel + 1] = base[1]
            pixels[pixel + 2] = base[2]
            pixels[pixel + 3] = cellAlpha
            pixel += 4
          }
        }
        continue
      }
      for (let ty = 0; ty < perCell; ty += 1) {
        for (let tx = 0; tx < perCell; tx += 1) {
          const worldX = spec.originX + (cellColumn + (tx + 0.5) / perCell) * spec.cell
          const worldY = spec.originY + (cellRow + (ty + 0.5) / perCell) * spec.cell
          const factor = patternFactor(cellPattern, worldX, worldY, value ?? 0)
          const pixel = ((cellRow * perCell + ty) * width + cellColumn * perCell + tx) * 4
          pixels[pixel] = base[0] * factor
          pixels[pixel + 1] = base[1] * factor
          pixels[pixel + 2] = base[2] * factor
          pixels[pixel + 3] = cellAlpha
        }
      }
    }
  }
  return { pixels, width, height, hasAlpha }
}

/**
 * Colour of one scalar sample, read out of a pre-sampled ramp table.
 *
 * `Uint8ClampedArray` rounds and clamps on store, so the scale into the ramp and the
 * scale out of it are both the caller's business; only the zero-means-absent rule is
 * repeated here, because that is a property of the layers rather than of the ramp.
 */
function rampColour(
  value: number | undefined,
  range: { min: number; max: number },
  ramp: Uint8Array,
): Rgb | null {
  if (value === undefined || !Number.isFinite(value) || value === 0) {
    return null
  }
  const step = Math.round(normalize(value, range.min, range.max) * 255) * 3
  return [ramp[step] ?? 0, ramp[step + 1] ?? 0, ramp[step + 2] ?? 0]
}

/**
 * Texels per cell a plan asks for, given how close the camera is.
 *
 * A pattern needs room: a hatch one texel wide is a different drawing, and one that
 * thins with distance stops reading as the material it stands for. So the resolution
 * follows the zoom — a whole map seen at once is a flat wash and needs a texel a cell,
 * while a chunk filling the screen is where a hatch has to hold up. Asking for the close
 * range everywhere instead cost sixteen times the work and the memory for detail no one
 * could see: one draped layer over a 399-chunk map went from about half a gigabyte of
 * texture to about a twelfth of that.
 *
 * `metresPerPixel` is the ground resolution the camera is reading at, and `cellMetres`
 * the size of a cell on the ground: a texel per cell is right once a cell is wider than
 * a pixel, which is the whole-map case.
 */
export function texelsPerCell(
  plan: LayerSurfacePlan | null,
  metresPerPixel: number | null = null,
  cellMetres: number | null = null,
  max = 4,
): number {
  if (plan === null) {
    return 1
  }
  const textured = plan.materials !== null || plan.pattern !== 'flat'
  if (!textured) {
    return 1
  }
  if (metresPerPixel === null || cellMetres === null || !(cellMetres > 0)) {
    return Math.max(1, max)
  }
  // How many texels a cell could show without the pattern aliasing: a cell no wider
  // than a pixel on screen has nothing to resolve.
  const affordable = Math.floor(cellMetres / Math.max(1e-6, metresPerPixel))
  return Math.max(1, Math.min(max, affordable))
}

/**
 * Value of the cell a texel stands for.
 *
 * The last column and row repeat their own cell on the map's rim, exactly as the mesh's
 * rim vertices do, so the texture and the geometry agree about which cell is under them.
 */
function cellValue(spec: ChunkGrid, sample: CellSampler, column: number, row: number): number {
  const atRimEast = spec.edges.east && column === spec.columns - 1
  const atRimSouth = spec.edges.south && row === spec.rows - 1
  const i = spec.firstI + (atRimEast ? column - 1 : column)
  const j = spec.firstJ + (atRimSouth ? row - 1 : row)
  return sample(i, j) ?? 0
}

/** Range of the cells a chunk's texture covers, for a caller that has no statistics. */
function sampledRange(spec: ChunkGrid, sample: CellSampler): { min: number; max: number } {
  const values: number[] = []
  for (let row = 0; row < spec.rows; row += 1) {
    for (let column = 0; column < spec.columns; column += 1) {
      values.push(cellValue(spec, sample, column, row))
    }
  }
  return valueRange(Float32Array.from(values))
}

/** Geometry of one draped chunk: the terrain's surface with the layer's texture coordinates. */
export interface DrapedMeshData {
  /** Vertex positions, `x, y, z`, taking the terrain's heights. */
  positions: Float32Array
  /** Triangle indices. */
  indices: Uint32Array
  /** Texture coordinates, `u, v` per vertex. */
  uvs: Float32Array
  /** Vertex normals, copied from the terrain so the drape shades like it. */
  normals: Float32Array
}

/** Builds the drape of one chunk from the terrain mesh it covers. */
export function drapedMesh(
  terrain: {
    positions: Float32Array
    indices: Uint32Array
    normals: Float32Array
    grid: { columns: number; rows: number }
  },
  /**
   * Height above the surface, metres.
   *
   * Drapes share the terrain's own vertices, so two of them enabled at once would
   * occupy the same depth and flicker between each other. Each layer is raised by a
   * small multiple of this instead, which is invisible at map scale.
   */
  liftM: number = 0,
): DrapedMeshData {
  const { columns, rows } = terrain.grid
  const uvs = new Float32Array(columns * rows * 2)
  for (let row = 0; row < rows; row += 1) {
    for (let column = 0; column < columns; column += 1) {
      const vertex = row * columns + column
      // Texel *centres*, on both axes: a vertex stands at a cell centre, so its texture
      // coordinate has to be the centre of that cell's texel. Stretching the texture across the
      // whole quad run (`column / (columns - 1)`) shifted every cell by up to half a cell, and
      // mirroring v (`1 - (row + 0.5) / rows`) drew the whole drape upside down relative to the
      // ground — a layer whose colours were flipped about the map's north edge while the mesh
      // under it was not, which is what "the layer does not line up with the terrain" looked like.
      //
      // Row 0 of the texture data is the row the mesh's row 0 stands on: the pixels are written
      // in the same order the cells are laid out, and the texture is uploaded without a flip.
      uvs[vertex * 2] = (column + 0.5) / columns
      uvs[vertex * 2 + 1] = (row + 0.5) / rows
    }
  }
  let positions = terrain.positions
  if (liftM !== 0) {
    positions = new Float32Array(terrain.positions)
    // The terrain is built with y up, so the lift is the second component.
    for (let vertex = 1; vertex < positions.length; vertex += 3) {
      positions[vertex] = (positions[vertex] ?? 0) + liftM
    }
  }
  return {
    positions,
    indices: terrain.indices,
    uvs,
    normals: terrain.normals,
  }
}

/** CSS colour of one value under a mapping, for a legend swatch. */
export function legendColor(
  mapping: ColorMapping,
  value: number,
  range: { min: number; max: number },
  flag?: Rgb,
): string {
  const color = sampleColour(value, mapping, range, flag) ?? [0, 0, 0]
  return `rgb(${color[0]} ${color[1]} ${color[2]})`
}
