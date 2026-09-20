/*
 * The map library and the chunks the viewer has loaded.
 *
 * The loading strategy lives here rather than in the viewer: a chunk is fetched
 * once and kept under `layer/level/chunk`, a second request for a chunk already in
 * flight joins the first promise instead of issuing another request, and a
 * screenful is fetched with a bounded number of parallel requests. The cache is
 * what makes panning cheap — the same chunk is usually visible from two camera
 * angles before it is off screen — and the in-flight map is what keeps a fast
 * camera from queueing the same chunk several times.
 */
import { defineStore } from 'pinia'
import { computed, ref, shallowRef } from 'vue'
import { api, mapWithConcurrency } from '@/api/client'
import { isApiError, type ApiError } from '@/api/errors'
import {
  createSyntheticMap,
  deleteMap,
  getChunk,
  getLayerGrid,
  getMap,
  importMap,
  listMaps,
  type ChunkRef,
  type SyntheticSpec,
} from '@/api/maps'
import type { LayerGrid, MapMetadata, MapSummary } from '@/api/types'
import {
  chunkKey,
  chunksInBounds,
  decodeChunk,
  defaultMapping,
  type DecodedChunk,
} from '@/types/map'
import type { Aabb } from '@/api/types'

/** How many chunk requests the viewer keeps in flight. */
export const CHUNK_CONCURRENCY = 6

/** Options of {@link useMapsStore.loadChunks}. */
export interface LoadChunksOptions {
  /** Requests in flight at once. */
  concurrency?: number
  /** Messages a request that fails reports; a missing chunk is not fatal on its own. */
  onError?: (ref: ChunkRef, error: ApiError) => void
}

/** State of a one-shot request the store keeps for the views. */
export type LoadStatus = 'idle' | 'loading' | 'ready' | 'failed'

/** The map library and its loaded chunks. */
export const useMapsStore = defineStore('maps', () => {
  const summaries = shallowRef<MapSummary[]>([])
  const listStatus = ref<LoadStatus>('idle')
  const listError = ref<string | null>(null)

  const metadata = shallowRef<Map<string, MapMetadata>>(new Map())
  const grids = shallowRef<Map<string, LayerGrid>>(new Map())
  const chunks = shallowRef<Map<string, DecodedChunk>>(new Map())
  const failures = shallowRef<Map<string, string>>(new Map())
  /**
   * Map the cache above belongs to.
   *
   * A chunk key is `layer/level/chunk`, and two maps drawn from the same preset have
   * identical bounds, grids and therefore identical chunk ids — so a cache without
   * the map in it hands the *previous* map's terrain to the next one, which the user
   * sees as the right frame drawn with the wrong ground. The cache is dropped when
   * the map changes instead of widening every key, which keeps the key format (and
   * every caller that builds one) unchanged.
   */
  let cacheMapId: string | null = null

  /** In-flight chunk requests, keyed the same way as the cache. */
  const inFlight = new Map<string, Promise<DecodedChunk>>()

  const loadedChunkCount = computed(() => chunks.value.size)

  /** Number of maps in the library. */
  const mapCount = computed(() => summaries.value.length)

  /** One library entry by id. */
  function summaryOf(id: string): MapSummary | null {
    return summaries.value.find((entry) => entry.id === id) ?? null
  }

  /** Cached metadata of a map, if it has been read. */
  function metadataOf(id: string): MapMetadata | null {
    return metadata.value.get(id) ?? null
  }

  /** Cached grid of one layer, if it has been read. */
  function gridOf(mapId: string, layerId: number): LayerGrid | null {
    return grids.value.get(`${mapId}/${layerId}`) ?? null
  }

  /** Cached chunk by its key, if it has been fetched. */
  function chunkOf(key: string): DecodedChunk | null {
    return chunks.value.get(key) ?? null
  }

  /** Reads the library; a later call replaces the list. */
  async function loadMaps(): Promise<void> {
    listStatus.value = 'loading'
    listError.value = null
    try {
      const page = await listMaps(0, 1_000)
      summaries.value = page.items
      listStatus.value = 'ready'
    } catch (error) {
      listError.value = isApiError(error) ? error.message : String(error)
      listStatus.value = 'failed'
    }
  }

  /** Imports an OMF image and returns the new entry, refreshing the list. */
  async function importOmf(bytes: Uint8Array, name?: string): Promise<MapSummary> {
    const summary = await importMap(bytes, name, api)
    summaries.value = [...summaries.value, summary]
    return summary
  }

  /** Builds a synthetic map and returns the new entry, refreshing the list. */
  async function createSynthetic(spec: SyntheticSpec, name?: string): Promise<MapSummary> {
    const summary = await createSyntheticMap(spec, name, api)
    summaries.value = [...summaries.value, summary]
    return summary
  }

  /** Removes a map, its cache and its metadata. */
  async function remove(id: string): Promise<void> {
    await deleteMap(id, api)
    summaries.value = summaries.value.filter((entry) => entry.id !== id)
    dropMap(id)
  }

  /** Reads a map's metadata, returning the cached copy when there is one. */
  async function loadMetadata(id: string, force = false): Promise<MapMetadata> {
    const cached = metadataOf(id)
    if (cached !== null && !force) {
      return cached
    }
    const reply = await getMap(id, api)
    metadata.value = new Map(metadata.value).set(id, reply)
    return reply
  }

  /** Reads one layer's chunk geometry, returning the cached copy when there is one. */
  async function loadGrid(mapId: string, layerId: number, force = false): Promise<LayerGrid> {
    const key = `${mapId}/${layerId}`
    const cached = grids.value.get(key)
    if (cached !== undefined && !force) {
      return cached
    }
    const reply = await getLayerGrid(mapId, layerId, api)
    grids.value = new Map(grids.value).set(key, reply)
    return reply
  }

  /**
   * Loads one chunk, or joins the request already in flight for it.
   *
   * The cache and the in-flight map are consulted in that order, so a chunk is
   * fetched exactly once per session unless the caller clears it.
   */
  async function loadChunk(target: ChunkRef): Promise<DecodedChunk> {
    ensureCacheFor(target.mapId)
    const key = chunkKey(target.layerId, target.level, target.chunkId)
    const cached = chunks.value.get(key)
    if (cached !== undefined) {
      return cached
    }
    const pending = inFlight.get(key)
    if (pending !== undefined) {
      return pending
    }
    const request = getChunk(target, 'base64', api)
      .then((payload) => {
        const decoded = decodeChunk(payload)
        chunks.value = new Map(chunks.value).set(key, decoded)
        if (failures.value.has(key)) {
          // A plain `delete` would not be seen: `failures` is shallow, so the panel
          // would keep reporting a failure that has since been resolved.
          const remaining = new Map(failures.value)
          remaining.delete(key)
          failures.value = remaining
        }
        return decoded
      })
      .finally(() => {
        inFlight.delete(key)
      })
    inFlight.set(key, request)
    return request
  }

  /**
   * Drops the chunk cache when a different map is being loaded.
   *
   * Called before every read, so a view that switches maps (or a second view opening
   * another map) cannot be served the previous map's chunks.
   */
  function ensureCacheFor(mapId: string): void {
    if (cacheMapId === mapId) {
      return
    }
    if (cacheMapId !== null) {
      clearChunks()
    }
    cacheMapId = mapId
  }

  /**
   * Loads several chunks with a bounded number of requests in flight.
   *
   * A failed chunk is reported and skipped: one missing chunk of a screenful
   * should leave the rest of the surface drawn.
   */
  async function loadChunks(
    refs: readonly ChunkRef[],
    options: LoadChunksOptions = {},
  ): Promise<void> {
    // One map per call: the references a caller builds always describe one layer of
    // one map, and the cache is dropped as soon as that map changes.
    if (refs[0] !== undefined) {
      ensureCacheFor(refs[0].mapId)
    }
    const pending = refs.filter(
      (target) => !chunks.value.has(chunkKey(target.layerId, target.level, target.chunkId)),
    )
    if (pending.length === 0) {
      return
    }
    await mapWithConcurrency(pending, options.concurrency ?? CHUNK_CONCURRENCY, async (target) => {
      try {
        await loadChunk(target)
      } catch (error) {
        const failure = isApiError(error) ? error : null
        if (failure !== null) {
          failures.value = new Map(failures.value).set(
            chunkKey(target.layerId, target.level, target.chunkId),
            failure.message,
          )
          options.onError?.(target, failure)
        }
      }
    })
  }

  /**
   * Chunk references that cover an area at a level.
   *
   * This is the "visible area only" rule: the caller passes the camera's footprint,
   * and only chunks the layer actually stores come back.
   */
  function chunkRefsInArea(
    mapId: string,
    layerId: number,
    chunkSize: number,
    level: number,
    area: Aabb,
  ): ChunkRef[] {
    const grid = gridOf(mapId, layerId)
    if (grid === null) {
      return []
    }
    return chunksInBounds(grid, chunkSize, level, area).map((chunkId) => ({
      mapId,
      layerId,
      level,
      chunkId,
    }))
  }

  /**
   * Forgets everything cached about one map.
   *
   * The chunk cache is keyed by `layer/level/chunk` — the three parts of a chunk's
   * address inside one map — so it carries no map id and cannot be filtered by
   * one. Dropping a map therefore clears the whole chunk cache; the viewer does
   * the same when it opens another map, which is the same trade made for the same
   * reason.
   */
  function dropMap(id: string): void {
    clearChunks()
    metadata.value = new Map([...metadata.value].filter(([key]) => key !== id))
    grids.value = new Map([...grids.value].filter(([key]) => !key.startsWith(`${id}/`)))
  }

  /** Drops every cached chunk and its failure notes. */
  function clearChunks(): void {
    chunks.value = new Map()
    failures.value = new Map()
    cacheMapId = null
    inFlight.clear()
  }

  /** The default colour mapping of one layer of a map, or `null` before its metadata is read. */
  function mappingFor(mapId: string, layerId: number): ReturnType<typeof defaultMapping> | null {
    const layer = metadataOf(mapId)?.layers.find((candidate) => candidate.layer_id === layerId)
    return layer === undefined ? null : defaultMapping(layer)
  }

  return {
    summaries,
    listStatus,
    listError,
    metadata,
    grids,
    chunks,
    failures,
    loadedChunkCount,
    mapCount,
    summaryOf,
    metadataOf,
    gridOf,
    chunkOf,
    loadMaps,
    importOmf,
    createSynthetic,
    remove,
    loadMetadata,
    loadGrid,
    loadChunk,
    loadChunks,
    chunkRefsInArea,
    clearChunks,
    mappingFor,
  }
})
