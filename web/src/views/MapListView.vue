<script setup lang="ts">
// Map library: import, synthetic generation, deletion and the way into the viewer.
import { computed, onMounted, ref } from 'vue'
import { useI18n } from 'vue-i18n'
import { useRouter } from 'vue-router'
import {
  Alert as TAlert,
  Button as TButton,
  DialogPlugin,
  Input as TInput,
  InputNumber as TInputNumber,
  Select as TSelect,
  Table as TTable,
  Tag as TTag,
} from 'tdesign-vue-next'
import type { MapSummary } from '@/api/types'
import { useMapsStore } from '@/stores/maps'
import { useNotificationsStore } from '@/stores/notifications'

/** One row of the table: the fields as they are shown, already formatted. */
type MapRow = {
  /** Map id, used by the row actions. */
  id: string
  /** Display name. */
  name: string
  /** Extent as `width × depth` in metres. */
  extent: string
  /** Cell size in metres. */
  resolution: string
  /** Registered layer count. */
  layers: string
  /** Translated source label. */
  source: string
  /** Image size, human readable. */
  size: string
  /** Creation timestamp in the interface locale. */
  created: string
}

const { t, locale } = useI18n({ useScope: 'global' })
const router = useRouter()
const maps = useMapsStore()
const notifications = useNotificationsStore()

const fileInput = ref<HTMLInputElement | null>(null)
const importName = ref('')
const importing = ref(false)
const syntheticName = ref('')
const syntheticPreset = ref('default')
const syntheticSeed = ref(0x0ddb1a5e)
const generating = ref(false)
const loadError = ref<string | null>(null)

const presetOptions = computed(() => [
  { value: 'default', label: t('map.list.syntheticPresetDefault') },
  { value: 'compact', label: t('map.list.syntheticPresetCompact') },
  { value: 'wide', label: t('map.list.syntheticPresetWide') },
])

const columns = computed(() => [
  { colKey: 'name', title: t('map.list.columnName'), width: 220 },
  { colKey: 'extent', title: t('map.list.columnExtent'), width: 170 },
  { colKey: 'resolution', title: t('map.list.columnResolution'), width: 110 },
  { colKey: 'layers', title: t('map.list.columnLayers'), width: 90 },
  { colKey: 'source', title: t('map.list.columnSource'), width: 110 },
  { colKey: 'size', title: t('map.list.columnSize'), width: 110 },
  { colKey: 'created', title: t('map.list.columnCreated'), width: 190 },
  { colKey: 'actions', title: ' ', width: 230 },
])

const rows = computed<MapRow[]>(() => maps.summaries.map(toRow))

/** Formats one summary for the table. */
function toRow(summary: MapSummary): MapRow {
  const width = summary.bounds.max_x - summary.bounds.min_x
  const depth = summary.bounds.max_y - summary.bounds.min_y
  return {
    id: summary.id,
    name: summary.name,
    extent: `${formatNumber(width)} × ${formatNumber(depth)} m`,
    resolution: `${formatNumber(summary.base_res_m)} m`,
    layers: formatNumber(summary.layer_count, 0),
    source: sourceLabel(summary.source),
    size: formatBytes(summary.size_bytes),
    created: formatTimestamp(summary.created_at),
  }
}

/** Formats a number in the interface locale; the core's units are the ones shown. */
function formatNumber(value: number, fractionDigits = 2): string {
  return new Intl.NumberFormat(locale.value, { maximumFractionDigits: fractionDigits }).format(
    value,
  )
}

/** Formats a byte count with a binary unit. */
function formatBytes(value: number): string {
  const units = ['B', 'KiB', 'MiB', 'GiB']
  let scaled = value
  let unit = 0
  while (scaled >= 1024 && unit < units.length - 1) {
    scaled /= 1024
    unit += 1
  }
  return `${formatNumber(scaled, 1)} ${units[unit] ?? 'B'}`
}

/** Formats an RFC 3339 timestamp, falling back to the raw value when it does not parse. */
function formatTimestamp(value: string): string {
  const parsed = new Date(value)
  return Number.isNaN(parsed.getTime()) ? value : parsed.toLocaleString(locale.value)
}

/** Translated label of a library source. */
function sourceLabel(source: string): string {
  switch (source) {
    case 'import':
      return t('map.list.sourceImport')
    case 'synthetic':
      return t('map.list.sourceSynthetic')
    case 'inline':
      return t('map.list.sourceInline')
    default:
      return source
  }
}

/** Reads the library again. */
async function refresh(): Promise<void> {
  await maps.loadMaps()
  loadError.value = maps.listStatus === 'failed' ? maps.listError : null
}

/** Opens the file picker. */
function chooseFile(): void {
  fileInput.value?.click()
}

/** Imports the chosen image; the reply is the new library entry. */
async function onFileChosen(event: Event): Promise<void> {
  const input = event.target as HTMLInputElement
  const file = input.files?.[0]
  input.value = ''
  if (file === undefined) {
    return
  }
  importing.value = true
  try {
    const bytes = new Uint8Array(await file.arrayBuffer())
    const summary = await maps.importOmf(bytes, importName.value.trim() || file.name)
    importName.value = ''
    notifications.push({
      kind: 'success',
      message: t('map.list.importDone', { name: summary.name }),
    })
  } catch (error) {
    notifications.pushError(t('map.list.importFailed'), error)
  } finally {
    importing.value = false
  }
}

/** Generates a synthetic map through the API and adds it to the library. */
async function generate(): Promise<void> {
  generating.value = true
  try {
    const summary = await maps.createSynthetic(
      { preset: syntheticPreset.value, seed: syntheticSeed.value },
      syntheticName.value.trim() || undefined,
    )
    syntheticName.value = ''
    notifications.push({
      kind: 'success',
      message: t('map.list.syntheticDone', { name: summary.name }),
    })
  } catch (error) {
    notifications.pushError(t('map.list.syntheticFailed'), error)
  } finally {
    generating.value = false
  }
}

/** Asks for confirmation, then removes the map. */
function confirmDelete(row: MapRow): void {
  const dialog = DialogPlugin.confirm({
    header: t('map.list.delete'),
    body: t('map.list.deleteConfirm', { name: row.name }),
    confirmBtn: t('common.confirm'),
    cancelBtn: t('common.cancel'),
    onConfirm: async () => {
      dialog.destroy()
      try {
        await maps.remove(row.id)
        notifications.push({
          kind: 'success',
          message: t('map.list.deleteDone', { name: row.name }),
        })
      } catch (error) {
        notifications.pushError(t('map.list.deleteFailed'), error)
      }
    },
  })
}

/** Opens the viewer of one map. */
function open(row: MapRow): void {
  void router.push({ name: 'map-viewer', params: { id: row.id } })
}

onMounted(() => {
  void refresh()
})
</script>

<template>
  <section class="mx-auto max-w-7xl px-8 py-8">
    <div class="flex items-center justify-between gap-4">
      <h1 class="font-semibold text-ink">{{ t('views.mapList.title') }}</h1>
      <div class="flex items-center gap-3">
        <span class="text-sm text-muted">{{ t('map.list.count', { total: maps.mapCount }) }}</span>
        <TButton variant="outline" :loading="maps.listStatus === 'loading'" @click="refresh()">
          {{ t('common.refresh') }}
        </TButton>
      </div>
    </div>

    <TAlert
      v-if="loadError !== null"
      class="mt-4"
      theme="error"
      :message="t('map.list.loadFailed')"
      data-testid="map-list-error"
    >
      <template #operation>
        <TButton variant="text" @click="refresh()">{{ t('common.retry') }}</TButton>
      </template>
      <p class="text-sm text-muted">
        <TTag size="small" variant="light">{{ t('map.viewer.fromService') }}</TTag>
        <span class="ml-2">{{ loadError }}</span>
      </p>
    </TAlert>

    <div
      v-for="notice in notifications.notices"
      :key="notice.id"
      class="mt-3 rounded-card border border-line bg-surface px-4 py-3"
      :data-testid="`notice-${notice.kind}`"
    >
      <div class="flex items-start justify-between gap-4">
        <div>
          <p class="text-sm font-medium text-ink">{{ notice.message }}</p>
          <p v-if="notice.fromService !== undefined" class="mt-1 text-sm text-muted">
            <TTag size="small" variant="light">{{ t('map.viewer.fromService') }}</TTag>
            <span class="ml-2">{{ notice.fromService }}</span>
          </p>
        </div>
        <TButton variant="text" @click="notifications.dismiss(notice.id)">
          {{ t('common.close') }}
        </TButton>
      </div>
    </div>

    <div class="mt-6 grid grid-cols-2 gap-6">
      <div class="rounded-card border border-line bg-surface p-4">
        <h2 class="text-base font-medium text-ink">{{ t('map.list.import') }}</h2>
        <p class="mt-1 text-sm text-muted">{{ t('map.list.importHint') }}</p>
        <div class="mt-3 flex items-end gap-3">
          <label class="flex-1">
            <span class="text-sm text-muted">{{ t('map.list.importName') }}</span>
            <TInput v-model="importName" :placeholder="t('map.list.importNamePlaceholder')" />
          </label>
          <TButton
            theme="primary"
            :loading="importing"
            data-testid="map-import"
            @click="chooseFile()"
          >
            {{ importing ? t('map.list.importing') : t('map.list.importAction') }}
          </TButton>
        </div>
        <input
          ref="fileInput"
          type="file"
          accept=".omf,application/octet-stream"
          class="hidden"
          data-testid="map-import-file"
          @change="onFileChosen"
        />
      </div>

      <div class="rounded-card border border-line bg-surface p-4">
        <h2 class="text-base font-medium text-ink">{{ t('map.list.synthetic') }}</h2>
        <p class="mt-1 text-sm text-muted">{{ t('map.list.syntheticHint') }}</p>
        <div class="mt-3 flex items-end gap-3">
          <label>
            <span class="text-sm text-muted">{{ t('map.list.syntheticPreset') }}</span>
            <TSelect v-model="syntheticPreset" :options="presetOptions" class="w-32" />
          </label>
          <label>
            <span class="text-sm text-muted">{{ t('map.list.syntheticSeed') }}</span>
            <TInputNumber v-model="syntheticSeed" :min="0" class="w-32" />
          </label>
          <label class="flex-1">
            <span class="text-sm text-muted">{{ t('map.list.importName') }}</span>
            <TInput
              v-model="syntheticName"
              :placeholder="t('map.list.importNamePlaceholder')"
              data-testid="map-synthetic-name"
            />
          </label>
          <TButton
            theme="primary"
            :loading="generating"
            data-testid="map-synthetic"
            @click="generate()"
          >
            {{ t('map.list.syntheticAction') }}
          </TButton>
        </div>
      </div>
    </div>

    <div class="mt-6">
      <TTable
        v-if="rows.length > 0"
        :data="rows"
        :columns="columns"
        row-key="id"
        hover
        data-testid="map-table"
      >
        <template #name="{ row }">
          <span class="font-medium text-ink">{{ row.name }}</span>
          <span class="ml-2 text-xs text-muted">{{ row.id }}</span>
        </template>
        <template #actions="{ row }">
          <div class="flex items-center gap-2">
            <TButton variant="text" @click="open(row)">{{ t('map.list.open') }}</TButton>
            <TButton variant="text" theme="danger" @click="confirmDelete(row)">
              {{ t('map.list.delete') }}
            </TButton>
          </div>
        </template>
      </TTable>
      <p v-else-if="maps.listStatus === 'loading'" class="text-sm text-muted">
        {{ t('common.loading') }}
      </p>
      <TAlert v-else-if="maps.listStatus === 'ready'" theme="info" :message="t('map.list.empty')" />
    </div>
  </section>
</template>
