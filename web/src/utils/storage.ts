/*
 * Persistence helpers for the settings stores.
 *
 * Storage is a browser API that can be absent twice over: private-mode browsers
 * deny it and the node-side test lanes have no `window`. Both calls therefore
 * degrade to a no-op instead of throwing, and a caller reads `null` for "no
 * stored value or no storage".
 */

/** The stored string for `key`, or null when the key is unset or storage is unavailable. */
export function readStored(key: string): string | null {
  if (typeof window === 'undefined') {
    return null
  }
  try {
    return globalThis.localStorage.getItem(key)
  } catch {
    return null
  }
}

/** Stores `value` under `key`; a denied write leaves the caller's in-memory state intact. */
export function writeStored(key: string, value: string): void {
  if (typeof window === 'undefined') {
    return
  }
  try {
    globalThis.localStorage.setItem(key, value)
  } catch {
    // Quota or policy refusal: the setting applies for this session only.
  }
}
