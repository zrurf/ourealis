<script setup lang="ts">
/*
 * OMF inspector: the structure tree the service reports, the edits it accepts, and
 * a patch applied to the same image.
 *
 * Every parse happens in the service (doc §10.4), so this page never reads the
 * format itself: it uploads bytes, renders the tree that comes back, and writes
 * edits as the JSON the edit endpoint takes. The one thing the page owes the user
 * is honesty about what a patch can express — the hint under the edit form says so,
 * because a refused edit comes back as `unsupported` with the reason rather than
 * being approximated.
 */
import { computed, onBeforeUnmount, ref } from 'vue'
import { useI18n } from 'vue-i18n'
import {
  Alert as TAlert,
  Button as TButton,
  Card as TCard,
  Divider as TDivider,
  Input as TInput,
  InputNumber as TInputNumber,
  Table as TTable,
  Tag as TTag,
  Textarea as TTextarea,
} from 'tdesign-vue-next'
import { fromBase64 } from '@/types/bytes'
import type { ConnectorEdit, EditScript, MapInfoEdit, RegionsEdit, TlvEdit } from '@/api/omf'
import { useNotificationsStore } from '@/stores/notifications'
import { formatBytes, footerRows, headerRows, useOmfStore, type StructureRow } from '@/stores/omf'
import { downloadBytes, formatMetric } from '@/stores/simulations'

/** Directory rows drawn at once; a big image has thousands. */
const DIRECTORY_ROWS = 200

const { t, locale } = useI18n({ useScope: 'global' })
const notifications = useNotificationsStore()
const omf = useOmfStore()

const fileInput = ref<HTMLInputElement | null>(null)
const patchInput = ref<HTMLInputElement | null>(null)
const pasted = ref('')
const pastedError = ref<string | null>(null)

const mapInfo = ref<MapInfoEdit>({ name: '' })
const regionsJson = ref('')
const connectorsJson = ref('')
const connectorsJsonError = ref<string | null>(null)
const tlvEdits = ref<Array<{ tag: string; payload: string }>>([])
const tlvError = ref<string | null>(null)

/** Header rows of the loaded image. */
const headerTable = computed<StructureRow[]>(() =>
  omf.structure === null ? [] : headerRows(omf.structure),
)

/** Footer rows of the loaded image. */
const footerTable = computed<StructureRow[]>(() =>
  omf.structure === null ? [] : footerRows(omf.structure),
)

/** Capability flags of the loaded image. */
const flagRows = computed(() => {
  const structure = omf.structure
  if (structure === null) {
    return []
  }
  return [
    { key: 'prm', labelKey: 'omf.inspector.flagPrm', value: structure.flags.has_prm },
    { key: 'zstd', labelKey: 'omf.inspector.flagZstdDict', value: structure.flags.has_zstd_dict },
    { key: 'kpath', labelKey: 'omf.inspector.flagKpath', value: structure.flags.has_kpath_library },
    { key: 'regions', labelKey: 'omf.inspector.flagRegions', value: structure.flags.has_regions },
    { key: 'vectors', labelKey: 'omf.inspector.flagVectors', value: structure.flags.has_vectors },
    { key: 'debug', labelKey: 'omf.inspector.flagDebug', value: structure.flags.debug_data },
  ]
})

/** Statistics rows of the loaded image. */
const statRows = computed(() => {
  const stats = omf.structure?.stats
  if (stats === undefined) {
    return []
  }
  return [
    { key: 'fileLen', labelKey: 'omf.inspector.statFileLen', value: formatBytes(stats.file_len) },
    { key: 'chunks', labelKey: 'omf.inspector.statChunkCount', value: String(stats.chunk_count) },
    { key: 'layers', labelKey: 'omf.inspector.statLayerCount', value: String(stats.layer_count) },
    {
      key: 'stored',
      labelKey: 'omf.inspector.statStoredBytes',
      value: formatBytes(stats.stored_bytes),
    },
    { key: 'raw', labelKey: 'omf.inspector.statRawBytes', value: formatBytes(stats.raw_bytes) },
  ]
})

/** Rows of the layer table. */
const layerRows = computed(() =>
  (omf.structure?.layers ?? []).map((layer) => ({
    key: String(layer.layer_id),
    layer: `0x${layer.layer_id.toString(16).padStart(4, '0')}`,
    kind: layer.kind,
    channels: layer.channels,
    dtype: layer.dtype,
    codec: layer.codec,
    scale: layer.scale,
    bias: layer.bias,
    sparse: layer.sparse ? t('omf.inspector.yes') : t('omf.inspector.no'),
    levels: layer.levels.join(', '),
  })),
)

/** Rows of the directory table, capped so a large image stays responsive. */
const directoryRows = computed(() =>
  (omf.structure?.directory ?? []).slice(0, DIRECTORY_ROWS).map((record, index) => ({
    key: `${record.layer_id}:${record.level}:${record.chunk_id}:${index}`,
    record: `0x${record.layer_id.toString(16).padStart(4, '0')} / ${record.level} / ${record.chunk_id}`,
    codec: record.codec_name,
    offset: record.offset,
    stored: formatBytes(record.comp_len),
    raw: formatBytes(record.raw_len),
    tombstone: record.tombstone ? t('omf.inspector.yes') : t('omf.inspector.no'),
  })),
)

/** Section counts of the loaded image. */
const sectionRows = computed(() => {
  const sections = omf.structure?.sections
  if (sections === undefined) {
    return []
  }
  return [
    {
      key: 'connectors',
      labelKey: 'omf.inspector.sectionsConnectors',
      value: sections.connectors ?? 0,
    },
    { key: 'regions', labelKey: 'omf.inspector.sectionsRegions', value: sections.regions ?? 0 },
    { key: 'vectors', labelKey: 'omf.inspector.sectionsVectors', value: sections.vectors ?? 0 },
    {
      key: 'roadmap',
      labelKey: 'omf.inspector.sectionsRoadmap',
      value: sections.roadmap_batches ?? 0,
    },
    {
      key: 'library',
      labelKey: 'omf.inspector.sectionsLibrary',
      value: sections.library_paths ?? 0,
    },
  ]
})

/** Whether the edit form has anything to send. */
const hasEdit = computed(
  () =>
    mapInfo.value.name.trim() !== '' ||
    regionsJson.value.trim() !== '' ||
    connectorsJson.value.trim() !== '' ||
    tlvEdits.value.length > 0,
)

/** Opens the file picker. */
function chooseFile(): void {
  fileInput.value?.click()
}

/** Inspects a picked file. */
async function onFileChosen(event: Event): Promise<void> {
  const input = event.target as HTMLInputElement
  const file = input.files?.[0]
  input.value = ''
  if (file === undefined) {
    return
  }
  const bytes = new Uint8Array(await file.arrayBuffer())
  await omf.inspect(bytes, file.name)
}

/** Inspects the pasted base64. */
async function inspectPasted(): Promise<void> {
  pastedError.value = null
  const text = pasted.value.trim()
  if (text === '') {
    pastedError.value = t('omf.source.bytesRequired')
    return
  }
  try {
    const bytes = fromBase64(text)
    await omf.inspect(bytes, t('omf.source.unnamed'))
  } catch (error) {
    pastedError.value = t('omf.source.encodeFailed')
    notifications.pushError(t('omf.source.inspectFailed'), error)
  }
}

/** Stages a metadata record edit. */
function addTlv(): void {
  tlvEdits.value = [...tlvEdits.value, { tag: '', payload: '{}' }]
}

/** Removes one staged metadata record. */
function removeTlv(index: number): void {
  tlvEdits.value = tlvEdits.value.filter((_, position) => position !== index)
}

/** Parses the staged edit into the script the service takes. */
function buildScript(): EditScript | null {
  const script: EditScript = {}
  if (mapInfo.value.name.trim() !== '') {
    script.set_map_info = { ...mapInfo.value, name: mapInfo.value.name.trim() }
  }
  if (regionsJson.value.trim() !== '') {
    try {
      script.regions = JSON.parse(regionsJson.value) as RegionsEdit
    } catch {
      notifications.push({ kind: 'error', message: t('omf.edit.tlvInvalid') })
      return null
    }
  }
  if (connectorsJson.value.trim() !== '') {
    try {
      script.connectors = JSON.parse(connectorsJson.value) as { connectors?: ConnectorEdit[] }
    } catch {
      connectorsJsonError.value = t('omf.edit.tlvInvalid')
      return null
    }
  }
  if (tlvEdits.value.length > 0) {
    const tlv: TlvEdit[] = []
    for (const edit of tlvEdits.value) {
      const tag = edit.tag.trim()
      if (tag === '') {
        tlvError.value = t('omf.edit.tlvTag')
        return null
      }
      try {
        tlv.push({ tag: numericTag(tag), json: JSON.parse(edit.payload) as unknown })
      } catch {
        tlvError.value = t('omf.edit.tlvInvalid')
        return null
      }
    }
    script.tlv = tlv
  }
  tlvError.value = null
  connectorsJsonError.value = null
  return script
}

/** A tag given as `0x…` or a decimal becomes a number; a wire name stays a name. */
function numericTag(tag: string): string | number {
  if (/^0x[0-9a-f]+$/i.test(tag)) {
    return Number.parseInt(tag, 16)
  }
  if (/^\d+$/.test(tag)) {
    return Number.parseInt(tag, 10)
  }
  return tag
}

/** Applies the staged edit to the loaded image. */
async function applyEdit(): Promise<void> {
  const script = buildScript()
  if (script === null) {
    return
  }
  const bytes = await omf.applyEdits(script)
  if (bytes !== null) {
    notifications.push({
      kind: 'success',
      message: t('omf.edit.done', { size: formatBytes(bytes.byteLength) }),
    })
  }
}

/** Applies a picked patch file to the loaded image. */
async function onPatchChosen(event: Event): Promise<void> {
  const input = event.target as HTMLInputElement
  const file = input.files?.[0]
  input.value = ''
  if (file === undefined) {
    return
  }
  const bytes = new Uint8Array(await file.arrayBuffer())
  const result = await omf.applyPatch(bytes)
  if (result !== null) {
    notifications.push({
      kind: 'success',
      message: t('omf.patch.done', { size: formatBytes(result.byteLength) }),
    })
  }
}

/** Downloads the edited image, or the original when nothing was applied. */
function download(): void {
  const bytes = omf.editedBytes ?? omf.sourceBytes
  if (bytes === null) {
    return
  }
  const name =
    omf.editedBytes === null ? (omf.fileName ?? 'map.omf') : (omf.editedName ?? 'map-edited.omf')
  downloadBytes(bytes, name)
  notifications.push({ kind: 'success', message: t('omf.download.done') })
}

/** Clears the loaded image and every staged edit. */
function clear(): void {
  omf.reset()
  mapInfo.value = { name: '' }
  regionsJson.value = ''
  connectorsJson.value = ''
  tlvEdits.value = []
  pasted.value = ''
  notifications.push({ kind: 'info', message: t('omf.source.cleared') })
}

onBeforeUnmount(() => {
  // The image stays in the store: the studio reads it from there.
})
</script>

<template>
  <section class="mx-auto max-w-7xl px-8 py-8" data-testid="omf-inspector">
    <div class="flex items-center justify-between gap-4">
      <div class="flex items-center gap-3">
        <h1 class="font-semibold text-ink">{{ t('views.omf.title') }}</h1>
        <TTag v-if="omf.structure !== null" size="small" variant="light" theme="success">
          {{
            t('omf.source.loaded', {
              name: omf.fileName ?? t('omf.source.unnamed'),
              size: formatBytes(omf.sourceSize),
            })
          }}
        </TTag>
      </div>
      <div class="flex items-center gap-2">
        <TButton variant="outline" :disabled="omf.sourceBytes === null" @click="download()">
          {{
            omf.editedBytes === null ? t('omf.download.original') : t('omf.download.editedFallback')
          }}
        </TButton>
        <TButton variant="outline" :disabled="omf.sourceBytes === null" @click="clear()">
          {{ t('omf.source.clear') }}
        </TButton>
      </div>
    </div>

    <TCard class="mt-6" :title="t('omf.source.title')" size="small">
      <div class="grid grid-cols-3 gap-6">
        <div>
          <TButton
            theme="primary"
            :loading="omf.status === 'loading'"
            data-testid="omf-upload"
            @click="chooseFile()"
          >
            {{ t('omf.source.upload') }}
          </TButton>
          <input
            ref="fileInput"
            type="file"
            accept=".omf,application/octet-stream"
            class="hidden"
            data-testid="omf-file"
            @change="onFileChosen"
          />
        </div>
        <label class="col-span-2">
          <span class="text-sm text-muted">{{ t('omf.source.paste') }}</span>
          <TTextarea
            v-model="pasted"
            :placeholder="t('omf.source.pastePlaceholder')"
            :autosize="{ minRows: 2, maxRows: 4 }"
            data-testid="omf-paste"
          />
          <div class="mt-2 flex items-center gap-3">
            <TButton variant="outline" data-testid="omf-inspect-paste" @click="inspectPasted()">
              {{ t('omf.source.inspect') }}
            </TButton>
            <span v-if="pastedError !== null" class="text-sm text-danger">{{ pastedError }}</span>
          </div>
        </label>
      </div>

      <TAlert
        v-if="omf.status === 'failed'"
        class="mt-3"
        theme="error"
        :message="t('omf.source.parseFailed')"
        data-testid="omf-error"
      >
        <p class="text-sm text-muted">
          <TTag size="small" variant="light">{{ t('omf.source.inspectFailed') }}</TTag>
          <span class="ml-2">{{ omf.error }}</span>
        </p>
      </TAlert>
      <p v-else-if="omf.structure === null" class="mt-3 text-sm text-muted">
        {{ t('omf.inspector.empty') }}
      </p>
    </TCard>

    <template v-if="omf.structure !== null">
      <div class="mt-6 grid grid-cols-3 gap-6">
        <TCard :title="t('omf.inspector.header')" size="small">
          <dl class="flex flex-col gap-1 text-sm">
            <div v-for="row in headerTable" :key="row.key" class="flex justify-between gap-3">
              <dt class="text-muted">{{ row.labelKey === null ? row.key : t(row.labelKey) }}</dt>
              <dd class="font-mono text-xs text-ink">{{ row.value }}</dd>
            </div>
          </dl>
        </TCard>
        <TCard :title="t('omf.inspector.footer')" size="small">
          <dl class="flex flex-col gap-1 text-sm">
            <div v-for="row in footerTable" :key="row.key" class="flex justify-between gap-3">
              <dt class="text-muted">{{ row.labelKey === null ? row.key : t(row.labelKey) }}</dt>
              <dd class="font-mono text-xs text-ink">{{ row.value }}</dd>
            </div>
          </dl>
        </TCard>
        <TCard :title="t('omf.inspector.flags')" size="small">
          <ul class="flex flex-col gap-1 text-sm">
            <li v-for="row in flagRows" :key="row.key" class="flex justify-between gap-3">
              <span class="text-muted">{{ t(row.labelKey ?? row.key) }}</span>
              <TTag size="small" variant="light" :theme="row.value ? 'success' : 'default'">
                {{ row.value ? t('omf.inspector.yes') : t('omf.inspector.no') }}
              </TTag>
            </li>
          </ul>
          <TDivider class="my-3" />
          <dl class="flex flex-col gap-1 text-sm">
            <div v-for="row in statRows" :key="row.key" class="flex justify-between gap-3">
              <dt class="text-muted">{{ t(row.labelKey ?? row.key) }}</dt>
              <dd class="font-mono text-xs text-ink">{{ row.value }}</dd>
            </div>
          </dl>
          <TDivider class="my-3" />
          <dl class="flex flex-col gap-1 text-sm">
            <div v-for="row in sectionRows" :key="row.key" class="flex justify-between gap-3">
              <dt class="text-muted">{{ t(row.labelKey ?? row.key) }}</dt>
              <dd class="font-mono text-xs text-ink">{{ row.value }}</dd>
            </div>
          </dl>
        </TCard>
      </div>

      <div class="mt-6 grid grid-cols-2 gap-6">
        <TCard :title="t('omf.inspector.metadata')" size="small">
          <TTable
            :data="omf.structure.metadata"
            :columns="[
              { colKey: 'tag', title: t('omf.inspector.tag'), width: 100 },
              { colKey: 'name', title: t('omf.inspector.tagName') },
              { colKey: 'len', title: t('omf.inspector.payloadLength'), width: 110 },
            ]"
            row-key="tag"
            size="small"
            data-testid="omf-metadata"
          />
        </TCard>
        <TCard :title="t('omf.inspector.patch')" size="small">
          <p class="text-sm text-muted">
            {{
              omf.structure.patch.available
                ? t('omf.inspector.patchAvailable', {
                    layers: omf.structure.patch.patchable_layers
                      .map((id) => `0x${id.toString(16).padStart(4, '0')}`)
                      .join(', '),
                  })
                : t('omf.inspector.patchUnavailable')
            }}
          </p>
          <dl class="mt-2 flex flex-col gap-1 text-sm">
            <div class="flex justify-between gap-3">
              <dt class="text-muted">{{ t('omf.inspector.patchBaseHash') }}</dt>
              <dd class="font-mono text-xs text-ink">{{ omf.structure.patch.base_hash64 }}</dd>
            </div>
            <div class="flex justify-between gap-3">
              <dt class="text-muted">{{ t('omf.inspector.patchProtected') }}</dt>
              <dd class="font-mono text-xs text-ink">
                {{
                  omf.structure.patch.protected_layers
                    .map((id) => `0x${id.toString(16).padStart(4, '0')}`)
                    .join(', ') || '—'
                }}
              </dd>
            </div>
            <div class="flex justify-between gap-3">
              <dt class="text-muted">{{ t('omf.inspector.geo') }}</dt>
              <dd class="font-mono text-xs text-ink">
                {{
                  omf.structure.geo_referenced
                    ? t('omf.inspector.geoIn')
                    : t('omf.inspector.geoOut')
                }}
              </dd>
            </div>
          </dl>
          <TButton
            class="mt-3"
            variant="outline"
            :disabled="omf.sourceBytes === null"
            data-testid="omf-patch"
            @click="patchInput?.click()"
          >
            {{ t('omf.patch.apply') }}
          </TButton>
          <input
            ref="patchInput"
            type="file"
            accept=".patch,.bin,application/octet-stream"
            class="hidden"
            data-testid="omf-patch-file"
            @change="onPatchChosen"
          />
        </TCard>
      </div>

      <TCard class="mt-6" :title="t('omf.inspector.skeleton')" size="small">
        <div class="flex items-center gap-6 text-sm">
          <span class="text-muted">
            {{ t('omf.inspector.skeletonNodes') }}:
            <span class="font-mono text-ink">{{ omf.structure.skeleton.nodes }}</span>
          </span>
          <span class="text-muted">
            {{ t('omf.inspector.skeletonLeaves') }}:
            <span class="font-mono text-ink">{{ omf.structure.skeleton.leaves }}</span>
          </span>
          <span class="text-muted">
            {{ t('omf.inspector.skeletonMaxDepth') }}:
            <span class="font-mono text-ink">{{ omf.structure.skeleton.max_depth }}</span>
          </span>
          <span class="text-muted">
            {{
              t('omf.inspector.aggregationProxy', {
                layer: omf.structure.skeleton.aggregation_rules.proxy_layer,
                channel: omf.structure.skeleton.aggregation_rules.proxy_channel,
              })
            }}
            <span class="font-mono text-ink">
              {{
                t('omf.inspector.aggregationScale', {
                  scale: omf.structure.skeleton.aggregation_rules.aggr_scale,
                  bias: omf.structure.skeleton.aggregation_rules.aggr_bias,
                })
              }}
            </span>
          </span>
        </div>
      </TCard>

      <TCard class="mt-6" :title="t('omf.inspector.layers')" size="small">
        <TTable
          :data="layerRows"
          :columns="[
            { colKey: 'layer', title: t('omf.inspector.columnLayer'), width: 100 },
            { colKey: 'kind', title: t('omf.inspector.columnKind'), width: 110 },
            { colKey: 'channels', title: t('omf.inspector.columnChannels'), width: 100 },
            { colKey: 'dtype', title: t('omf.inspector.columnDtype'), width: 100 },
            { colKey: 'codec', title: t('omf.inspector.columnCodec'), width: 90 },
            { colKey: 'scale', title: t('omf.inspector.columnScale'), width: 100 },
            { colKey: 'bias', title: t('omf.inspector.columnBias'), width: 100 },
            { colKey: 'sparse', title: t('omf.inspector.columnSparse'), width: 90 },
            { colKey: 'levels', title: t('omf.inspector.columnLevels'), width: 110 },
          ]"
          row-key="key"
          size="small"
          data-testid="omf-layers"
        />
        <p v-if="omf.structure.derived.length > 0" class="mt-3 text-sm text-muted">
          {{ t('omf.inspector.derived') }}:
          <span v-for="layer in omf.structure.derived" :key="layer.layer_id" class="mr-3">
            {{ layer.name }} ({{ layer.status }})
          </span>
        </p>
      </TCard>

      <TCard class="mt-6" :title="t('omf.inspector.directory')" size="small">
        <p class="text-sm text-muted">
          {{ t('omf.inspector.directoryTotal', { count: omf.structure.directory.length }) }}
        </p>
        <TTable
          class="mt-2"
          :data="directoryRows"
          :columns="[
            { colKey: 'record', title: t('omf.inspector.columnRecord') },
            { colKey: 'codec', title: t('omf.inspector.columnCodec'), width: 110 },
            { colKey: 'offset', title: t('omf.inspector.columnOffset'), width: 120 },
            { colKey: 'stored', title: t('omf.inspector.columnStored'), width: 110 },
            { colKey: 'raw', title: t('omf.inspector.columnRaw'), width: 110 },
            { colKey: 'tombstone', title: t('omf.inspector.columnTombstone'), width: 110 },
          ]"
          row-key="key"
          size="small"
          data-testid="omf-directory"
        />
      </TCard>

      <TCard class="mt-6" :title="t('omf.edit.title')" size="small">
        <p class="text-sm text-muted">{{ t('omf.edit.stored') }}</p>
        <p class="mt-1 text-sm text-muted">{{ t('omf.edit.unsupported') }}</p>

        <div class="mt-3 grid grid-cols-3 gap-4">
          <label class="col-span-3">
            <span class="text-sm text-muted">{{ t('omf.edit.mapInfo') }}</span>
          </label>
          <label>
            <span class="text-xs text-muted">{{ t('omf.edit.name') }}</span>
            <TInput
              :value="mapInfo.name"
              data-testid="edit-name"
              @change="(value) => (mapInfo = { ...mapInfo, name: String(value) })"
            />
          </label>
          <label>
            <span class="text-xs text-muted">{{ t('omf.edit.author') }}</span>
            <TInput
              :value="mapInfo.author ?? ''"
              @change="(value) => (mapInfo = { ...mapInfo, author: String(value) })"
            />
          </label>
          <label>
            <span class="text-xs text-muted">{{ t('omf.edit.builtUnix') }}</span>
            <TInputNumber
              :value="mapInfo.built_unix ?? undefined"
              :min="0"
              @change="(value) => (mapInfo = { ...mapInfo, built_unix: Number(value) })"
            />
          </label>
          <label class="col-span-3">
            <span class="text-xs text-muted">{{ t('omf.edit.description') }}</span>
            <TInput
              :value="mapInfo.description ?? ''"
              @change="(value) => (mapInfo = { ...mapInfo, description: String(value) })"
            />
          </label>
        </div>

        <div class="mt-4 grid grid-cols-2 gap-4">
          <label>
            <span class="text-sm text-muted">{{ t('omf.edit.regions') }}</span>
            <TTextarea
              v-model="regionsJson"
              :placeholder="'{ &quot;features&quot;: [], &quot;outlines&quot;: [], &quot;merge&quot;: true }'"
              :autosize="{ minRows: 3, maxRows: 6 }"
              data-testid="edit-regions-json"
            />
            <span class="text-xs text-muted">{{ t('omf.edit.regionHint') }}</span>
          </label>
          <label>
            <span class="text-sm text-muted">{{ t('omf.edit.connectors') }}</span>
            <TTextarea
              v-model="connectorsJson"
              :placeholder="'{ &quot;connectors&quot;: [], &quot;merge&quot;: true }'"
              :autosize="{ minRows: 3, maxRows: 6 }"
              data-testid="edit-connectors-json"
            />
            <span v-if="connectorsJsonError !== null" class="text-xs text-danger">
              {{ connectorsJsonError }}
            </span>
          </label>
        </div>

        <div class="mt-4">
          <div class="flex items-center justify-between">
            <span class="text-sm text-muted">
              {{ t('omf.edit.entries', { count: tlvEdits.length }) }}
            </span>
            <TButton variant="outline" size="small" data-testid="edit-add-tlv" @click="addTlv()">
              {{ t('omf.edit.addTlv') }}
            </TButton>
          </div>
          <p class="mt-1 text-xs text-muted">{{ t('omf.edit.tlvHint') }}</p>
          <div
            v-for="(entry, index) in tlvEdits"
            :key="index"
            class="mt-2 flex items-end gap-2"
            :data-testid="`tlv-${index}`"
          >
            <label class="w-56">
              <span class="text-xs text-muted">{{ t('omf.edit.tlvTag') }}</span>
              <TInput v-model="entry.tag" :placeholder="t('omf.edit.tlvTagPlaceholder')" />
            </label>
            <label class="flex-1">
              <span class="text-xs text-muted">{{ t('omf.edit.tlvPayload') }}</span>
              <TTextarea v-model="entry.payload" :autosize="{ minRows: 1, maxRows: 4 }" />
            </label>
            <TButton variant="text" size="small" @click="removeTlv(index)">
              {{ t('omf.studio.remove') }}
            </TButton>
          </div>
          <p v-if="tlvError !== null" class="mt-1 text-sm text-danger">{{ tlvError }}</p>
        </div>

        <div class="mt-4 flex items-center gap-3">
          <TButton
            theme="primary"
            :loading="omf.editStatus === 'loading'"
            :disabled="!hasEdit"
            data-testid="edit-apply"
            @click="applyEdit()"
          >
            {{ omf.editStatus === 'loading' ? t('omf.edit.applying') : t('omf.edit.apply') }}
          </TButton>
          <TButton
            variant="outline"
            data-testid="edit-download"
            :disabled="omf.editedBytes === null"
            @click="download()"
          >
            {{ t('omf.download.editedFallback') }}
          </TButton>
          <span v-if="omf.editError !== null" class="text-sm text-danger">
            {{ omf.editError }}
          </span>
        </div>

        <TAlert
          v-if="omf.editedBytes !== null"
          class="mt-3"
          theme="success"
          :message="t('omf.edit.done', { size: formatBytes(omf.editedBytes.byteLength) })"
        >
          <p class="text-sm text-muted">
            {{
              t('omf.edit.outlinePoints', {
                count: formatMetric(omf.editedBytes.byteLength, 'B', locale, 0),
              })
            }}
          </p>
        </TAlert>
      </TCard>
    </template>
  </section>
</template>
