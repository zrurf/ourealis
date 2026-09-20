/*
 * Service information and capabilities.
 *
 * Small on purpose: the shell reads it once to say which service it is talking
 * to, and the map views use it to decide whether a service that answers no maps is
 * a healthy empty library or a facade that is switched off. A failure to load it
 * is not fatal, so it is kept as a value rather than thrown.
 */
import { defineStore } from 'pinia'
import { computed, ref } from 'vue'
import { fetchHealth, fetchSystemInfo, type Health, type SystemInfo } from '@/api/system'
import { isApiError } from '@/api/errors'

/** How far the information request has come. */
export type SystemStatus = 'idle' | 'loading' | 'ready' | 'failed'

/** Health, build facts and capabilities of the connected service. */
export const useSystemStore = defineStore('system', () => {
  const info = ref<SystemInfo | null>(null)
  const health = ref<Health | null>(null)
  const status = ref<SystemStatus>('idle')
  const error = ref<string | null>(null)

  /** Version line the shell shows, or an empty string before the first reply. */
  const version = computed(() => info.value?.version ?? '')

  /** True once a reply has arrived and the service reported itself alive. */
  const isAlive = computed(() => health.value?.status === 'ok')

  /** Reads health and build facts once; a second call while loading is ignored. */
  async function load(): Promise<void> {
    if (status.value === 'loading') {
      return
    }
    status.value = 'loading'
    error.value = null
    try {
      health.value = await fetchHealth()
      info.value = await fetchSystemInfo()
      status.value = 'ready'
    } catch (failure) {
      error.value = isApiError(failure) ? failure.message : String(failure)
      status.value = 'failed'
    }
  }

  return { info, health, status, error, version, isAlive, load }
})
