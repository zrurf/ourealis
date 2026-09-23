/*
 * Section overlays and the direction field, shared by every map view.
 *
 * A family is one service request plus some geometry, and the geometry depends on what the *view*
 * has loaded: the surface an outline is draped on, the level it is drawn at, the chunks its arrows
 * read. Both the preview and the workspace need exactly that, and a copy in each would drift — the
 * viewer had the logic first and the workspace simply had no overlay controls for a while.
 *
 * The controller owns the toggles' *effect*: it loads a family the first time it is switched on,
 * re-applies visibility afterwards, and rebuilds the arrow field. Beyond which families it has
 * read it holds no state, so a view can hand it the same dependencies on every pass.
 */
import type { Aabb, LayerGrid, MapMetadata } from '@/api/types'
import type { ChunkRef } from '@/api/maps'
import { getConnectors, getPrmGraph, getRegions, getSkeleton } from '@/api/maps'
import {
  LAYER_DIRECTION,
  drapeSourceLevel,
  levelCellSize,
  type DecodedChunk,
  type PlaneOrigin,
} from '@/types/map'
import { connectors as readConnectors, prmEdges, prmNodes, regionOutlines } from '@/types/sections'
import type { OverlayToggles } from '@/stores/viewer'
import { createCellSampler } from './cellSampler'
import { directionGrid } from './direction'
import type { DirectionArrows } from './directionArrows'
import type { OverlayKind, OverlaySet, SurfaceHeight } from './overlays'

/** Everything the controller reads from the view it is serving. */
export interface OverlaySyncDeps {
  /** Geometry manager for the section families. */
  manager: OverlaySet
  /** The direction field's arrows. */
  arrows: DirectionArrows | null
  /** Map the view is showing. */
  mapId: string
  /** The map's metadata, or `null` before it is read. */
  metadata: () => MapMetadata | null
  /** Which families are switched on. */
  toggles: () => OverlayToggles
  /** Level the surface is drawn at. */
  level: () => number
  /** Ground height under a map position, already draped and terraced. */
  surface: () => SurfaceHeight
  /** Loaded chunks, for the arrow field's samples. */
  chunks: () => ReadonlyMap<string, DecodedChunk>
  /** Cached grid of a layer, or `null` when it has not been read. */
  gridOf: (layerId: number) => LayerGrid | null
  /** Reads a layer's grid when it is not cached yet. */
  ensureGrid: (layerId: number) => Promise<void>
  /** Chunk references of an area, already addressing the map's own origin. */
  refsInArea: (layerId: number, level: number, area: Aabb) => ChunkRef[]
  /** Loads a batch of chunks. */
  loadChunks: (refs: ChunkRef[]) => Promise<void>
  /** Area the camera can see, in the map's metre plane. */
  footprint: () => Aabb
  /** Origin of the map's local plane. */
  origin: () => PlaneOrigin
  /** Reports a family that could not be read, already classified by the caller. */
  onError: (kind: OverlayKind, error: unknown) => void
}

/** The families that come from a map section; the direction field has its own reader. */
const SECTION_KINDS: readonly OverlayKind[] = ['regions', 'connectors', 'skeleton', 'prm']

/** What a view keeps between passes: the families it has already read. */
export type LoadedOverlays = Set<OverlayKind>

/** Builds the controller state a view holds for the lifetime of one open map. */
export function createOverlaySync(): LoadedOverlays {
  return new Set<OverlayKind>()
}

/**
 * Brings the overlays in line with the toggles.
 *
 * Idempotent and cheap when nothing changed: a family that is already loaded only has its
 * visibility re-applied, which is what makes "switch off, switch on" work — the early return it
 * replaced left the family hidden for the rest of the session.
 */
export async function syncOverlays(loaded: LoadedOverlays, deps: OverlaySyncDeps): Promise<void> {
  const toggles = deps.toggles()
  for (const kind of SECTION_KINDS) {
    if (!toggles[kind]) {
      deps.manager.setVisible(kind, false)
      continue
    }
    if (loaded.has(kind)) {
      deps.manager.setVisible(kind, true)
      continue
    }
    loaded.add(kind)
    try {
      // One family at a time, deliberately: each request builds geometry from the surface the view
      // has loaded, and a burst of them on one connection would starve the terrain chunks.
      // oxlint-disable-next-line no-await-in-loop
      await applyFamily(kind, deps)
      deps.manager.setVisible(kind, true)
    } catch (error) {
      // A family that failed is not remembered as loaded, so switching it on again retries.
      loaded.delete(kind)
      deps.onError(kind, error)
    }
  }
  await syncDirectionField(deps)
}

/** Reads one section family and draws it. */
async function applyFamily(kind: OverlayKind, deps: OverlaySyncDeps): Promise<void> {
  const surface = deps.surface()
  switch (kind) {
    case 'regions': {
      const section = await getRegions(deps.mapId)
      deps.manager.setRegions(regionOutlines(section.json), surface)
      return
    }
    case 'connectors': {
      const section = await getConnectors(deps.mapId)
      deps.manager.setConnectors(readConnectors(section.json))
      return
    }
    case 'skeleton': {
      const skeleton = await getSkeleton(deps.mapId)
      deps.manager.setSkeleton(skeleton.nodes, 6, surface)
      return
    }
    case 'prm': {
      const section = await getPrmGraph(deps.mapId)
      deps.manager.setPrm(prmNodes(section.json), prmEdges(section.json))
      return
    }
    default:
      return
  }
}

/**
 * Rebuilds the direction arrows.
 *
 * The field is a *cell layer* rather than a section, so its chunks are read like a drape's: without
 * that the arrows had nothing to sample and the switch did nothing at all.
 */
async function syncDirectionField(deps: OverlaySyncDeps): Promise<void> {
  const field = deps.arrows
  const bounds = deps.metadata()?.summary.bounds
  if (field === null || bounds === undefined) {
    field?.clear()
    return
  }
  if (!deps.toggles().direction) {
    field.clear()
    return
  }
  await deps.ensureGrid(LAYER_DIRECTION)
  const grid = deps.gridOf(LAYER_DIRECTION)
  if (grid === null) {
    field.clear()
    return
  }
  const level = deps.level()
  const sourceLevel = drapeSourceLevel(grid, level) ?? level
  const chunkSize = deps.metadata()?.summary.chunk_size ?? 0
  await deps.loadChunks(deps.refsInArea(LAYER_DIRECTION, sourceLevel, deps.footprint()))
  const sample = createCellSampler({
    chunks: deps.chunks(),
    grid,
    chunkSize,
    level,
    layerId: LAYER_DIRECTION,
    sourceLevel,
  })
  const cellSize = levelCellSize(grid, sourceLevel)
  const step = Math.max(24, cellSize * 8)
  const origin = deps.origin()
  const samples = directionGrid(
    { x: bounds.min_x, y: bounds.min_y },
    { width: bounds.max_x - bounds.min_x, height: bounds.max_y - bounds.min_y },
    step,
    (x, y) => {
      const value = sample(
        Math.floor((x - origin.x) / cellSize),
        Math.floor((y - origin.y) / cellSize),
      )
      if (value === null || value <= 0) {
        return null
      }
      const packed = Math.trunc(value)
      const strength = packed & 0xff
      return strength === 0
        ? null
        : { at: { x, y }, angleDeg: ((packed >> 8) / 256) * 360, strength }
    },
  )
  field.set(samples, deps.surface())
  field.setVisible(true)
}

/** Whether a map carries a family at all, so a control can say so before it is switched on. */
export function overlayAvailability(metadata: MapMetadata | null): Record<OverlayKind, boolean> {
  return {
    regions: (metadata?.sections.regions ?? 0) > 0,
    connectors: (metadata?.sections.connectors ?? 0) > 0,
    skeleton: (metadata?.skeleton_nodes ?? 0) > 0,
    prm: (metadata?.sections.roadmap_batches ?? 0) > 0,
    direction: (metadata?.layers ?? []).some((layer) => layer.layer_id === LAYER_DIRECTION),
  }
}
