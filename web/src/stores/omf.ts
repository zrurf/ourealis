/*
 * OMF inspector and editor state.
 *
 * The service parses the format (doc §10.4), so this store holds what came back
 * from `/omf/inspect` and the image bytes an edit applies to — never a second
 * parser. The structure tree's shape is the service's own JSON, typed here as the
 * subset the inspector renders; {@link readStructure} narrows an untrusted reply
 * rather than trusting a cast.
 *
 * Edits are stateful on purpose: the studio and the inspector both accumulate an
 * edit script, preview the rows they will send, and apply it to the image in one
 * request.
 */
import { defineStore } from 'pinia'
import { computed, ref, shallowRef } from 'vue'
import { isApiError } from '@/api/errors'
import {
  editOmf,
  inspectOmf,
  patchOmf,
  type ConnectorEdit,
  type EditScript,
  type ImageReply,
  type MapInfoEdit,
  type RegionFeatureEdit,
  type RegionsEdit,
} from '@/api/omf'
import type { DerivedLayer, LayerInfo, SectionCounts } from '@/api/types'
import { useNotificationsStore } from './notifications'
import { i18n } from '@/locales'

/** Header fields the inspector lists, as `api::maps::header_json` writes them. */
export interface OmfHeader {
  /** Semantic major version. */
  version_major: number
  /** Semantic minor version. */
  version_minor: number
  /** Flag bits as a hex string. */
  flags: string
  /** Reference longitude, degrees. */
  ref_lon_deg: number
  /** Reference latitude, degrees. */
  ref_lat_deg: number
  /** EPSG code, 0 for a pure local plane. */
  epsg: number
  /** Map extent. */
  bounds: { min_x: number; min_y: number; max_x: number; max_y: number }
  /** Finest resolution, centimetres per cell. */
  base_res_cm: number
  /** Chunk side length in cells. */
  chunk_size: number
  /** Declared LOD level count. */
  lod_count: number
  /** Feature dimension count. */
  feature_dim: number
  /** Registered layer count. */
  layer_count: number
  /** Offset of the metadata block, bytes. */
  meta_offset: number
  /** Length of the metadata block, bytes. */
  meta_len: number
  /** Offset of the directory, bytes. */
  dir_offset: number
  /** Length of the directory, bytes. */
  dir_len: number
  /** Offset of the extended metadata block, bytes. */
  ext_meta_offset: number
  /** Length of the extended metadata block, bytes. */
  ext_meta_len: number
}

/** One metadata record as the inspector lists it. */
export interface OmfTlvRecord {
  /** Numeric tag. */
  tag: number
  /** Wire name of the tag, for example `map_info`. */
  name: string
  /** Payload length, bytes. */
  len: number
}

/** One directory record. */
export interface OmfDirectoryRecord {
  /** Layer id. */
  layer_id: number
  /** LOD level. */
  level: number
  /** Morton chunk id. */
  chunk_id: number
  /** Codec id. */
  codec: number
  /** Codec name. */
  codec_name: string
  /** Offset in the file, bytes. */
  offset: number
  /** Stored length, bytes. */
  comp_len: number
  /** Decoded length, bytes. */
  raw_len: number
  /** Raw flag bits. */
  flags: number
  /** Whether the record is a tombstone. */
  tombstone: boolean
}

/** Directory counts of one layer and level. */
export interface OmfDirectoryCount {
  /** Layer id. */
  layer_id: number
  /** LOD level. */
  level: number
  /** Records stored. */
  records: number
  /** Records that are not tombstones. */
  chunks: number
}

/** Whole-file statistics. */
export interface OmfStats {
  /** File length, bytes. */
  file_len: number
  /** Chunk count. */
  chunk_count: number
  /** Registered layer count. */
  layer_count: number
  /** Stored bytes. */
  stored_bytes: number
  /** Decoded bytes. */
  raw_bytes: number
}

/** Patch availability of an image. */
export interface OmfPatchInfo {
  /** Whether any layer may be patched. */
  available: boolean
  /** Base hash the patch must name, 64 bit. */
  base_hash64: number
  /** Layer ids a patch may replace. */
  patchable_layers: number[]
  /** Derived layer ids the format protects from patching. */
  protected_layers: number[]
}

/** Skeleton summary of an image. */
export interface OmfSkeletonInfo {
  /** Node count. */
  nodes: number
  /** Leaf count. */
  leaves: number
  /** Deepest node depth. */
  max_depth: number
  /** Aggregation rules in effect. */
  aggregation_rules: {
    /** Layer the aggregate proxies. */
    proxy_layer: number
    /** Channel the aggregate proxies. */
    proxy_channel: number
    /** Quantisation scale of the aggregate. */
    aggr_scale: number
    /** Quantisation bias of the aggregate. */
    aggr_bias: number
  }
}

/** The structure tree `/omf/inspect` answers with. */
export interface OmfStructure {
  /** Header fields. */
  header: OmfHeader
  /** Footer summary. */
  footer: {
    /** Semantic major version. */
    version_major: number
    /** Semantic minor version. */
    version_minor: number
    /** Directory record count. */
    dir_record_count: number
    /** Chunk count. */
    chunk_count: number
    /** Skeleton node count. */
    node_count: number
    /** Registered layer count. */
    layer_count: number
    /** Whole-file length, bytes. */
    file_len: number
    /** Whole-file hash, lowercase hex. */
    file_hash: string
  }
  /** Capability flags declared by the header. */
  flags: {
    /** Whether the image carries a roadmap. */
    has_prm: boolean
    /** Whether the image carries a zstd dictionary. */
    has_zstd_dict: boolean
    /** Whether the image carries a candidate path library. */
    has_kpath_library: boolean
    /** Whether the image carries regions. */
    has_regions: boolean
    /** Whether the image carries vectors. */
    has_vectors: boolean
    /** Whether the image carries debug data. */
    debug_data: boolean
  }
  /** Whole-file statistics. */
  stats: OmfStats
  /** Metadata records. */
  metadata: OmfTlvRecord[]
  /** Registered layers. */
  layers: LayerInfo[]
  /** Optional section counts. */
  sections: SectionCounts
  /** Derived layers and whether their fingerprints check out. */
  derived: DerivedLayer[]
  /** Skeleton summary. */
  skeleton: OmfSkeletonInfo
  /** Directory records. */
  directory: OmfDirectoryRecord[]
  /** Directory records per layer and level. */
  directory_counts: OmfDirectoryCount[]
  /** Patch availability. */
  patch: OmfPatchInfo
  /** Whether the image carries a geographic reference. */
  geo_referenced: boolean
}

/** One row of the header table. */
export interface StructureRow {
  /** Field name as the service writes it. */
  key: string
  /** Translation key of the label, when the inspector has one. */
  labelKey: string | null
  /** Value as text, already formatted. */
  value: string
}

/** True when the untrusted value looks like an inspection reply. */
export function readStructure(value: unknown): OmfStructure | null {
  if (typeof value !== 'object' || value === null) {
    return null
  }
  const record = value as Record<string, unknown>
  if (
    typeof record.header !== 'object' ||
    record.header === null ||
    typeof record.footer !== 'object' ||
    record.footer === null ||
    !Array.isArray(record.layers) ||
    !Array.isArray(record.directory)
  ) {
    return null
  }
  return value as OmfStructure
}

/** Formats a byte count for a table cell. */
export function formatBytes(value: number, digits = 1): string {
  const units = ['B', 'KiB', 'MiB', 'GiB']
  let scaled = Math.max(0, value)
  let unit = 0
  while (scaled >= 1024 && unit < units.length - 1) {
    scaled /= 1024
    unit += 1
  }
  return `${scaled.toFixed(digits)} ${units[unit] ?? 'B'}`
}

/** Formats a number with a fixed number of decimals, for a structure table. */
function fixed(value: number, digits = 2): string {
  return value.toFixed(digits)
}

/** The header rows the inspector shows, in reading order. */
export function headerRows(structure: OmfStructure): StructureRow[] {
  const header = structure.header
  const number = fixed
  return [
    {
      key: 'version',
      labelKey: 'omf.inspector.version',
      value: `${header.version_major}.${header.version_minor}`,
    },
    { key: 'flags', labelKey: 'omf.inspector.fieldFlags', value: header.flags },
    { key: 'epsg', labelKey: 'omf.inspector.fieldEpsg', value: String(header.epsg) },
    {
      key: 'ref_lon_deg',
      labelKey: 'omf.inspector.fieldRefLon',
      value: number(header.ref_lon_deg, 7),
    },
    {
      key: 'ref_lat_deg',
      labelKey: 'omf.inspector.fieldRefLat',
      value: number(header.ref_lat_deg, 7),
    },
    { key: 'bounds', labelKey: 'omf.inspector.fieldBounds', value: boundsText(header.bounds) },
    {
      key: 'base_res_cm',
      labelKey: 'omf.inspector.fieldBaseRes',
      value: `${number(header.base_res_cm)} cm`,
    },
    {
      key: 'chunk_size',
      labelKey: 'omf.inspector.fieldChunkSize',
      value: String(header.chunk_size),
    },
    { key: 'lod_count', labelKey: 'omf.inspector.fieldLodCount', value: String(header.lod_count) },
    {
      key: 'feature_dim',
      labelKey: 'omf.inspector.fieldFeatureDim',
      value: String(header.feature_dim),
    },
    {
      key: 'layer_count',
      labelKey: 'omf.inspector.fieldLayerCount',
      value: String(header.layer_count),
    },
    {
      key: 'meta',
      labelKey: 'omf.inspector.fieldMetaBlock',
      value: offsetText(header.meta_offset, header.meta_len),
    },
    {
      key: 'dir',
      labelKey: 'omf.inspector.fieldDirBlock',
      value: offsetText(header.dir_offset, header.dir_len),
    },
    {
      key: 'ext_meta',
      labelKey: 'omf.inspector.fieldExtMetaBlock',
      value: offsetText(header.ext_meta_offset, header.ext_meta_len),
    },
  ]
}

/** The footer rows the inspector shows. */
export function footerRows(structure: OmfStructure): StructureRow[] {
  const footer = structure.footer
  return [
    {
      key: 'file_len',
      labelKey: 'omf.inspector.fieldFileLen',
      value: formatBytes(footer.file_len),
    },
    { key: 'file_hash', labelKey: 'omf.inspector.fieldFileHash', value: footer.file_hash },
    {
      key: 'dir_record_count',
      labelKey: 'omf.inspector.fieldDirRecords',
      value: String(footer.dir_record_count),
    },
    {
      key: 'chunk_count',
      labelKey: 'omf.inspector.fieldChunkCount',
      value: String(footer.chunk_count),
    },
    {
      key: 'node_count',
      labelKey: 'omf.inspector.fieldNodeCount',
      value: String(footer.node_count),
    },
    {
      key: 'layer_count',
      labelKey: 'omf.inspector.fieldLayerCount',
      value: String(footer.layer_count),
    },
    {
      key: 'version',
      labelKey: 'omf.inspector.version',
      value: `${footer.version_major}.${footer.version_minor}`,
    },
  ]
}

/** `offset + length` of one block, in bytes. */
function offsetText(offset: number, length: number): string {
  return `${offset} + ${formatBytes(length)}`
}

/** Extent of an image, in metres. */
function boundsText(bounds: {
  min_x: number
  min_y: number
  max_x: number
  max_y: number
}): string {
  const width = bounds.max_x - bounds.min_x
  const depth = bounds.max_y - bounds.min_y
  return `${width.toFixed(1)} × ${depth.toFixed(1)} m @ (${bounds.min_x.toFixed(1)}, ${bounds.min_y.toFixed(1)})`
}

/**
 * A region edit the studio accumulates.
 *
 * The outline travels with the feature, because the service addresses an outline
 * through the feature's `geom_ref` index.
 */
export interface RegionDraft {
  /** Outline vertices, `[east, north]` metre pairs. */
  points: Array<[number, number]>
  /** Semantic tag id the feature carries. */
  tag_id: number
  /** Multipath event probability per entry. */
  p_mp?: number
  /** Multipath bias magnitude, metres. */
  mp_bias_m?: number
  /** GNSS dropout probability inside the region. */
  p_loss?: number
  /** `probabilistic` or `spatial_deterministic`. */
  trigger_mode?: string
}

/** Turns region drafts into the edit the service expects. */
export function regionsEdit(drafts: readonly RegionDraft[], merge = false): RegionsEdit {
  const features: RegionFeatureEdit[] = []
  const outlines: Array<Array<[number, number]>> = []
  drafts.forEach((draft, index) => {
    features.push({
      tag_id: draft.tag_id,
      geom_ref: index,
      ...(draft.p_mp === undefined ? {} : { p_mp: draft.p_mp }),
      ...(draft.mp_bias_m === undefined ? {} : { mp_bias_m: draft.mp_bias_m }),
      ...(draft.p_loss === undefined ? {} : { p_loss: draft.p_loss }),
      ...(draft.trigger_mode === undefined ? {} : { trigger_mode: draft.trigger_mode }),
    })
    outlines.push(draft.points.map((point) => [point[0], point[1]]))
  })
  return { features, outlines, merge }
}

/** Area of a closed polygon, square metres; the shoelace formula. */
export function polygonArea(points: readonly [number, number][]): number {
  if (points.length < 3) {
    return 0
  }
  let sum = 0
  for (let index = 0; index < points.length; index += 1) {
    const current = points[index]
    const next = points[(index + 1) % points.length]
    if (current === undefined || next === undefined) {
      continue
    }
    sum += current[0] * next[1] - next[0] * current[1]
  }
  return Math.abs(sum) / 2
}

/** Message of a failure, preferring the service's own text. */
function messageOf(error: unknown): string {
  return isApiError(error) ? error.message : String(error)
}

/** Inspection, editing and patching of one image. */
export const useOmfStore = defineStore('omf', () => {
  const structure = shallowRef<OmfStructure | null>(null)
  const status = ref<'idle' | 'loading' | 'ready' | 'failed'>('idle')
  const error = ref<string | null>(null)
  const fileName = ref<string | null>(null)

  const sourceBytes = shallowRef<Uint8Array | null>(null)
  const editedBytes = shallowRef<Uint8Array | null>(null)
  const editedName = ref<string | null>(null)
  const editStatus = ref<'idle' | 'loading' | 'ready' | 'failed'>('idle')
  const editError = ref<string | null>(null)

  /** Size of the loaded image, bytes. */
  const sourceSize = computed(() => sourceBytes.value?.byteLength ?? 0)

  /** Whether an image is loaded and could be edited. */
  const hasImage = computed(() => sourceBytes.value !== null)

  /** Whether the loaded image can be patched at all. */
  const patchAvailable = computed(() => structure.value?.patch.available ?? false)

  /** Reads an image's structure tree and keeps the bytes it will edit. */
  async function inspect(bytes: Uint8Array, name?: string): Promise<OmfStructure | null> {
    status.value = 'loading'
    error.value = null
    try {
      const reply = readStructure(await inspectOmf(bytes))
      if (reply === null) {
        status.value = 'failed'
        error.value = 'the inspection reply was not a structure tree'
        return null
      }
      structure.value = reply
      sourceBytes.value = bytes
      fileName.value = name ?? null
      editedBytes.value = null
      editedName.value = null
      status.value = 'ready'
      return reply
    } catch (failure) {
      error.value = messageOf(failure)
      status.value = 'failed'
      return null
    }
  }

  /** Applies an edit script to the loaded image and keeps the new bytes. */
  async function applyEdits(edits: EditScript, base?: Uint8Array): Promise<Uint8Array | null> {
    const image = base ?? sourceBytes.value
    if (image === null) {
      editStatus.value = 'failed'
      editError.value = 'no image is loaded'
      return null
    }
    editStatus.value = 'loading'
    editError.value = null
    try {
      const reply = await editOmf(image, edits)
      return keepEdited(reply)
    } catch (failure) {
      const notifications = useNotificationsStore()
      editError.value = messageOf(failure)
      editStatus.value = 'failed'
      notifications.pushError(i18n.global.t('omf.edit.failed'), failure)
      return null
    }
  }

  /** Applies a patch file to the loaded image and keeps the new bytes. */
  async function applyPatch(patch: Uint8Array, base?: Uint8Array): Promise<Uint8Array | null> {
    const image = base ?? sourceBytes.value
    if (image === null) {
      editStatus.value = 'failed'
      editError.value = 'no image is loaded'
      return null
    }
    editStatus.value = 'loading'
    editError.value = null
    try {
      const reply = await patchOmf(image, patch)
      return keepEdited(reply)
    } catch (failure) {
      const notifications = useNotificationsStore()
      editError.value = messageOf(failure)
      editStatus.value = 'failed'
      notifications.pushError(i18n.global.t('omf.patch.failed'), failure)
      return null
    }
  }

  /** Stores an edited image and returns its bytes. */
  function keepEdited(reply: ImageReply): Uint8Array {
    editedBytes.value = reply.bytes
    editedName.value = reply.fileName
    editStatus.value = 'ready'
    return reply.bytes
  }

  /** Forgets the loaded image, its structure and its edits. */
  function reset(): void {
    structure.value = null
    sourceBytes.value = null
    editedBytes.value = null
    editedName.value = null
    fileName.value = null
    status.value = 'idle'
    error.value = null
    editStatus.value = 'idle'
    editError.value = null
  }

  /** Replaces the source bytes without a fresh inspection; the studio uses it for a library map. */
  function setSource(bytes: Uint8Array, name?: string): void {
    sourceBytes.value = bytes
    fileName.value = name ?? null
  }

  return {
    structure,
    status,
    error,
    fileName,
    sourceBytes,
    editedBytes,
    editedName,
    editStatus,
    editError,
    sourceSize,
    hasImage,
    patchAvailable,
    inspect,
    applyEdits,
    applyPatch,
    reset,
    setSource,
  }
})

/** A map information record as the edit form holds it. */
export type { MapInfoEdit, ConnectorEdit, EditScript, RegionsEdit }
