/*
 * Map library, metadata, chunk and section endpoints.
 *
 * The viewer's loading strategy lives in `stores/maps.ts`; this module only
 * speaks HTTP. Every call takes the client so a test can point it at a fake
 * `fetch`, and every optional argument is last so a caller can stop at the part
 * it needs.
 */
import { api, ApiClient, pageQuery } from './client'
import { fileNameOf } from './omf'
import type {
  ChunkPayload,
  LayerGrid,
  MapMetadata,
  MapSummary,
  Page,
  SectionJson,
  Skeleton,
} from './types'

/** Selector of a stored chunk. */
export interface ChunkRef {
  /** Map the chunk belongs to. */
  mapId: string
  /** Layer id as it appears in the file, for example `1` for elevation. */
  layerId: number
  /** LOD level the chunk belongs to. */
  level: number
  /** Morton chunk id inside that level's chunk grid. */
  chunkId: number
}

/** Formats the chunk endpoint accepts. */
export type ChunkFormat = 'json' | 'base64'

/** Parameters of the synthetic map generator; every field is optional. */
export interface SyntheticSpec {
  /** Shape preset: `default`, `compact` or `wide`. */
  preset?: string
  /** Map width, metres. Ignored when a preset is named. */
  width_m?: number
  /** Map height, metres. */
  height_m?: number
  /** Cell resolution, metres. */
  resolution_m?: number
  /** Chunk side length in cells. */
  chunk_size?: number
  /** Seed of the generator. */
  seed?: number
  /** Include a candidate path library. */
  with_kpath_library?: boolean
}

/** Region annotations and their outlines, as map-format names them. */
export interface RegionSection {
  /** Region features. */
  features: unknown[]
  /** Outlines, each an `{index, points}` pair referencing a feature index. */
  outlines: Array<{ index: number; points: Array<[number, number]> }>
}

/** Z-axis connector table. */
export interface ConnectorSection {
  /** Connector entries. */
  connectors: unknown[]
  /** Unit cost of the cheapest connector, metres. */
  unit_cost_min: number
}

/** One PRM roadmap batch. */
export interface PrmGraphSection {
  /** Batch index. */
  batch: number
  /** Sampling seed. */
  seed: number
  /** Waypoints. */
  nodes: unknown[]
  /** Edges as a flat list with the source node spelled out. */
  edges: unknown[]
  /** Links between roadmap nodes and grid cells. */
  interfaces: unknown[]
}

/** A page of maps. */
export function listMaps(
  offset = 0,
  limit = 1_000,
  client: ApiClient = api,
  signal?: AbortSignal,
): Promise<Page<MapSummary>> {
  return client.get<Page<MapSummary>>('maps', { query: pageQuery(offset, limit), signal })
}

/** Everything the inspector knows about one map. */
export function getMap(
  mapId: string,
  client: ApiClient = api,
  signal?: AbortSignal,
): Promise<MapMetadata> {
  return client.get<MapMetadata>(`maps/${encodeURIComponent(mapId)}`, { signal })
}

/**
 * Imports an OMF image.
 *
 * The bytes are the body and the name travels in the query, which is how the
 * service accepts an upload; the reply is the new library entry.
 */
export function importMap(
  bytes: Uint8Array,
  name?: string,
  client: ApiClient = api,
  signal?: AbortSignal,
): Promise<MapSummary> {
  return client.post<MapSummary>('maps', {
    query: { name },
    body: bytes as BodyInit,
    contentType: 'application/octet-stream',
    signal,
  })
}

/** Builds a synthetic map and adds it to the library. */
export function createSyntheticMap(
  spec: SyntheticSpec,
  name?: string,
  client: ApiClient = api,
  signal?: AbortSignal,
): Promise<MapSummary> {
  return client.post<MapSummary>('maps/synthetic', {
    query: { name },
    body: JSON.stringify(spec),
    signal,
  })
}

/**
 * Downloads a map's OMF image.
 *
 * The service returns the bytes it stored at import, so this is the file the caller
 * uploaded rather than a rebuild. The file name the service suggests comes back with
 * the bytes for the caller to use in a download.
 */
export async function downloadMapImage(
  id: string,
  signal?: AbortSignal,
): Promise<{ bytes: Uint8Array; fileName: string }> {
  const response = await api.raw(`maps/${encodeURIComponent(id)}/image`, { signal })
  return {
    bytes: new Uint8Array(await response.arrayBuffer()),
    fileName: fileNameOf(response.headers.get('content-disposition')) ?? `${id}.omf`,
  }
}

/** Removes a map from the library. */
export function deleteMap(
  mapId: string,
  client: ApiClient = api,
  signal?: AbortSignal,
): Promise<null> {
  return client.delete<null>(`maps/${encodeURIComponent(mapId)}`, { signal })
}

/** Cell and chunk geometry of one layer, plus the chunk ids the file stores. */
export function getLayerGrid(
  mapId: string,
  layerId: number,
  client: ApiClient = api,
  signal?: AbortSignal,
): Promise<LayerGrid> {
  return client.get<LayerGrid>(`maps/${encodeURIComponent(mapId)}/layers/${layerId}/grid`, {
    signal,
  })
}

/** One chunk's samples in `[y][x][channel]` order. */
export function getChunk(
  ref: ChunkRef,
  format: ChunkFormat = 'base64',
  client: ApiClient = api,
  signal?: AbortSignal,
): Promise<ChunkPayload> {
  const path = `maps/${encodeURIComponent(ref.mapId)}/layers/${ref.layerId}/chunks/${ref.level}/${ref.chunkId}`
  return client.get<ChunkPayload>(path, { query: { format }, signal })
}

/** Region annotations and their outlines. */
export function getRegions(
  mapId: string,
  client: ApiClient = api,
  signal?: AbortSignal,
): Promise<SectionJson<RegionSection>> {
  return client.get<SectionJson<RegionSection>>(`maps/${encodeURIComponent(mapId)}/regions`, {
    signal,
  })
}

/** Z-axis connector table. */
export function getConnectors(
  mapId: string,
  client: ApiClient = api,
  signal?: AbortSignal,
): Promise<SectionJson<ConnectorSection>> {
  return client.get<SectionJson<ConnectorSection>>(`maps/${encodeURIComponent(mapId)}/connectors`, {
    signal,
  })
}

/** Vector polylines and polygons. */
export function getVectors(
  mapId: string,
  client: ApiClient = api,
  signal?: AbortSignal,
): Promise<SectionJson<unknown>> {
  return client.get<SectionJson<unknown>>(`maps/${encodeURIComponent(mapId)}/vectors`, { signal })
}

/** One PRM roadmap batch. */
export function getPrmGraph(
  mapId: string,
  batch = 0,
  client: ApiClient = api,
  signal?: AbortSignal,
): Promise<SectionJson<PrmGraphSection>> {
  return client.get<SectionJson<PrmGraphSection>>(`maps/${encodeURIComponent(mapId)}/graph/prm`, {
    query: { batch },
    signal,
  })
}

/** Quadtree skeleton nodes. */
export function getSkeleton(
  mapId: string,
  client: ApiClient = api,
  signal?: AbortSignal,
): Promise<Skeleton> {
  return client.get<Skeleton>(`maps/${encodeURIComponent(mapId)}/skeleton`, { signal })
}
