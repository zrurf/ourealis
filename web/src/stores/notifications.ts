/*
 * Notification toasts.
 *
 * One queue for everything the user must see: a failed request, a fallback the
 * viewer took, a completed import. A message carries the service's own text
 * beside the translated one where the failure came from the service, which is
 * what the error banner labels (doc §6.6).
 */
import { defineStore } from 'pinia'
import { computed, ref } from 'vue'
import { isApiError, type ApiError } from '@/api/errors'

/** Severity of a notice, mapped onto the toast appearance. */
export type NoticeKind = 'info' | 'success' | 'warning' | 'error'

/** One notice in the queue. */
export interface Notice {
  /** Monotonic identifier, used as the list key. */
  id: number
  /** Severity. */
  kind: NoticeKind
  /** Already-translated headline. */
  message: string
  /** The service's own message, shown as-is and marked as coming from the service. */
  fromService?: string
  /** Translated detail: what the client did about it. */
  detail?: string
  /** Whether the notice disappears on its own. */
  dismissible: boolean
}

/** What a caller passes to {@link useNotificationsStore.push}. */
export interface NoticeInput {
  /** Severity. */
  kind: NoticeKind
  /** Already-translated headline. */
  message: string
  /** The service's message, when the failure came from it. */
  fromService?: string
  /** Translated detail. */
  detail?: string
}

/** Queue of notices shown to the user. */
export const useNotificationsStore = defineStore('notifications', () => {
  const notices = ref<Notice[]>([])
  let nextId = 1

  /** Notices of the highest severity, for a global banner. */
  const errors = computed(() => notices.value.filter((notice) => notice.kind === 'error'))

  /** Adds a notice and returns its identifier. */
  function push(input: NoticeInput): number {
    const id = nextId
    nextId += 1
    notices.value = [...notices.value, { id, dismissible: true, ...input }]
    return id
  }

  /** Adds an error notice from a failure, carrying the service's message when there is one. */
  function pushError(message: string, error: unknown, detail?: string): number {
    const service = isApiError(error) ? error.message : undefined
    return push({
      kind: 'error',
      message,
      ...(service === undefined ? {} : { fromService: service }),
      ...(detail === undefined ? {} : { detail }),
    })
  }

  /** Adds an error notice from an {@link ApiError} the caller already narrowed. */
  function pushApiError(message: string, error: ApiError, detail?: string): number {
    return push({
      kind: 'error',
      message,
      fromService: error.message,
      ...(detail === undefined ? {} : { detail }),
    })
  }

  /** Removes one notice. */
  function dismiss(id: number): void {
    notices.value = notices.value.filter((notice) => notice.id !== id)
  }

  /** Removes every notice. */
  function clear(): void {
    notices.value = []
  }

  return { notices, errors, push, pushError, pushApiError, dismiss, clear }
})
