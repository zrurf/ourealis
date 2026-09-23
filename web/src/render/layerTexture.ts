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
import { SCALAR_RAMPS, colorForCategory, normalize, rampAt, valueRange } from '@/types/colormap'
import type { CellSampler } from './cellSampler'
import { applyPattern } from './patterns'
import { materialForValue, type LayerSurfacePlan } from './surfaces'
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
  let hasAlpha = false
  for (let cellRow = 0; cellRow < spec.rows; cellRow += 1) {
    for (let cellColumn = 0; cellColumn < spec.columns; cellColumn += 1) {
      const value = cellValue(spec, sample, cellColumn, cellRow)
      // A named material wins over the mapping: a road is drawn as a road.
      const material = surface === null ? null : materialForValue(surface, value)
      const base = material?.colour ?? sampleColour(value, options.mapping, range, options.flag)
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
      for (let ty = 0; ty < perCell; ty += 1) {
        for (let tx = 0; tx < perCell; tx += 1) {
          const worldX = spec.originX + (cellColumn + (tx + 0.5) / perCell) * spec.cell
          const worldY = spec.originY + (cellRow + (ty + 0.5) / perCell) * spec.cell
          const colour = applyPattern(base, material?.pattern ?? pattern, worldX, worldY, value)
          const pixel = ((cellRow * perCell + ty) * width + cellColumn * perCell + tx) * 4
          pixels[pixel] = colour[0]
          pixels[pixel + 1] = colour[1]
          pixels[pixel + 2] = colour[2]
          pixels[pixel + 3] = 255
        }
      }
    }
  }
  return { pixels, width, height, hasAlpha }
}

/** Texels per cell a plan asks for: patterns need room, a flat colour does not. */
export function texelsPerCell(plan: LayerSurfacePlan | null, requested = 4): number {
  if (plan === null) {
    return 1
  }
  const textured = plan.materials !== null || plan.pattern !== 'flat'
  return textured ? Math.max(1, requested) : 1
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
