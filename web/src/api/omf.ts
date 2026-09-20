/*
 * OMF inspection and editing.
 *
 * The service parses the format, so the client never carries a second
 * implementation of it (doc §10.4). Inspection uploads the image as the request
 * body; editing and patching carry images inside a JSON body as base64, because
 * the edit script travels beside them.
 */
import { api, ApiClient } from './client'
import { toBase64 } from '@/types/bytes'

/** Map information record as the editor writes it. */
export interface MapInfoEdit {
  /** Map name. */
  name: string
  /** Author of the build. */
  author?: string
  /** Build time, Unix seconds. */
  built_unix?: number
  /** Upstream data snapshot hash, lowercase or uppercase hex. */
  upstream_hash?: string
  /** Free-form description. */
  description?: string
}

/** One region feature as the editor writes it. */
export interface RegionFeatureEdit {
  /** Semantic tag id. */
  tag_id: number
  /** Outline index inside the outline list. */
  geom_ref?: number
  /** Multipath event probability per entry. */
  p_mp?: number
  /** Multipath bias magnitude, metres. */
  mp_bias_m?: number
  /** GNSS dropout probability inside the region. */
  p_loss?: number
  /** `probabilistic` or `spatial_deterministic`. */
  trigger_mode?: string
}

/** Region features and outlines to write. */
export interface RegionsEdit {
  /** Features to write. */
  features?: RegionFeatureEdit[]
  /** Outlines, each a list of `[x, y]` metre pairs. */
  outlines?: Array<Array<[number, number]>>
  /** Whether the content is merged into the existing set instead of replacing it. */
  merge?: boolean
}

/** One connector as the editor writes it. */
export interface ConnectorEdit {
  /** Connector kind id. */
  type_id: number
  /** Endpoint A, metres. */
  a: [number, number, number]
  /** Endpoint B, metres. */
  b: [number, number, number]
  /** `both`, `a_to_b` or `b_to_a`. */
  direction?: string
  /** Speed upwards, m/s. */
  v_up?: number
  /** Speed downwards, m/s. */
  v_down?: number
  /** Waiting time at an endpoint, seconds. */
  wait_time?: number
  /** Vector shape the connector references. */
  attr_ref?: number
  /** Unit cost in equivalent metres. */
  unit_cost?: number
}

/** Connectors to write. */
export interface ConnectorsEdit {
  /** Connectors to write. */
  connectors?: ConnectorEdit[]
  /** Whether the connectors are merged into the existing table instead of replacing it. */
  merge?: boolean
}

/** A metadata record to rewrite. */
export interface TlvEdit {
  /** Tag, as a wire name (`map_info`, `slope_model`, …) or its numeric value. */
  tag: string | number
  /** Record payload, in the shape the inspector reports. */
  json: unknown
}

/**
 * The edits of one request.
 *
 * The service applies them in the order map info, regions, connectors, raw
 * records; the order inside this object does not matter.
 */
export interface EditScript {
  /** Replacement map information record. */
  set_map_info?: MapInfoEdit
  /** Region set to write. */
  regions?: RegionsEdit
  /** Connector table to write. */
  connectors?: ConnectorsEdit
  /** Metadata records to rewrite. */
  tlv?: TlvEdit[]
}

/** An image the service returned, with the download name it suggested. */
export interface ImageReply {
  /** New image bytes. */
  bytes: Uint8Array
  /** File name from `Content-Disposition`, or `null` when the header named none. */
  fileName: string | null
}

/**
 * Inspects an uploaded image.
 *
 * The reply is the structure tree the service built — header fields, TLV list,
 * directory, layers, skeleton, fingerprints, patch availability. Its shape is
 * the inspector's own business, so it stays `unknown` here rather than being
 * restated as a type that would have to track the Rust struct.
 */
export function inspectOmf(
  bytes: Uint8Array,
  client: ApiClient = api,
  signal?: AbortSignal,
): Promise<unknown> {
  return client.post<unknown>('omf/inspect', {
    body: bytes as BodyInit,
    contentType: 'application/octet-stream',
    signal,
  })
}

/** Applies an edit script to an image and returns the new image. */
export function editOmf(
  bytes: Uint8Array,
  edits: EditScript,
  client: ApiClient = api,
  signal?: AbortSignal,
): Promise<ImageReply> {
  return postForImage('omf/edit', { image_base64: toBase64(bytes), edits }, client, signal)
}

/** Applies a patch file to a base image and returns the new image. */
export function patchOmf(
  base: Uint8Array,
  patch: Uint8Array,
  client: ApiClient = api,
  signal?: AbortSignal,
): Promise<ImageReply> {
  return postForImage(
    'omf/patch',
    { base_base64: toBase64(base), patch_base64: toBase64(patch) },
    client,
    signal,
  )
}

/** Posts a JSON body that answers with an image and reads its download name. */
async function postForImage(
  path: string,
  body: unknown,
  client: ApiClient,
  signal?: AbortSignal,
): Promise<ImageReply> {
  const response = await client.raw(path, {
    method: 'POST',
    body: JSON.stringify(body),
    signal,
  })
  return {
    bytes: new Uint8Array(await response.arrayBuffer()),
    fileName: fileNameOf(response.headers.get('content-disposition')),
  }
}

/** Extracts the file name of a `Content-Disposition` header, or `null` when it names none. */
export function fileNameOf(header: string | null): string | null {
  if (header === null) {
    return null
  }
  const match = /filename="?([^";]+)"?/i.exec(header)
  return match?.[1] ?? null
}
