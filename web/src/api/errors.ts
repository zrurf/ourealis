/*
 * Failure classification of the API.
 *
 * The service answers every rejected request with `{"error":{"kind","message",
 * "status"}}` (doc §4.3) and keeps the message English, so the message travels
 * unchanged to the UI while `kind` decides what the client does about it. A
 * failure that never reached the service — a dropped connection, a CORS refusal
 * — has no body and is classified as `network`.
 */

/** Classifications the service reports, mirroring its `ErrorKind`. */
export const API_ERROR_KINDS = [
  'invalid',
  'unprocessable',
  'not_found',
  'conflict',
  'too_large',
  'busy',
  'unsupported',
  'core',
  'internal',
] as const

/** A classification the service reports. */
export type ApiErrorKind = (typeof API_ERROR_KINDS)[number]

/** Classification of a failure that never reached the service. */
export const NETWORK_KIND = 'network'

/** Every classification a client can see. */
export type ApiFailureKind = ApiErrorKind | typeof NETWORK_KIND

/** Narrows an untrusted `kind` field to a classification the service documents. */
export function isApiErrorKind(value: unknown): value is ApiErrorKind {
  return typeof value === 'string' && (API_ERROR_KINDS as readonly string[]).includes(value)
}

/**
 * A failed API call.
 *
 * `fromService` distinguishes a message the service wrote from one this client
 * wrote, which is what the error banner labels (doc §6.6).
 */
export class ApiError extends Error {
  /** Classification of the failure. */
  readonly kind: ApiFailureKind
  /** HTTP status the service answered with, or `null` when no reply arrived. */
  readonly status: number | null

  constructor(kind: ApiFailureKind, message: string, status: number | null = null) {
    super(message)
    this.name = 'ApiError'
    this.kind = kind
    this.status = status
  }

  /** True when the message is the service's own text rather than a client fallback. */
  get fromService(): boolean {
    return this.kind !== NETWORK_KIND
  }
}

/** Narrows an unknown value to an {@link ApiError}. */
export function isApiError(value: unknown): value is ApiError {
  return value instanceof ApiError
}

/**
 * Maps a response body onto an {@link ApiError}.
 *
 * A body that does not match the documented shape still carries a status, and that
 * status is classified rather than discarded; a body whose `kind` this client does
 * not know is classified by its status too, because that is the part both sides
 * agree on.
 */
export function apiErrorFromBody(status: number, body: unknown): ApiError {
  const detail = errorDetail(body)
  if (detail === null) {
    return new ApiError(kindFromStatus(status), `HTTP ${status}`, status)
  }
  return new ApiError(
    isApiErrorKind(detail.kind) ? detail.kind : kindFromStatus(status),
    detail.message,
    status,
  )
}

/** Maps a thrown value onto an {@link ApiError}, leaving an existing one untouched. */
export function apiErrorFromUnknown(error: unknown): ApiError {
  if (isApiError(error)) {
    return error
  }
  const message = error instanceof Error ? error.message : String(error)
  return new ApiError(NETWORK_KIND, message, null)
}

/** The `{kind, message}` pair of a documented error body, or `null` for any other shape. */
function errorDetail(body: unknown): { kind: string; message: string } | null {
  if (typeof body !== 'object' || body === null || !('error' in body)) {
    return null
  }
  const error = body.error
  if (typeof error !== 'object' || error === null) {
    return null
  }
  const kind = 'kind' in error ? error.kind : undefined
  const message = 'message' in error ? error.message : undefined
  if (typeof kind !== 'string' || typeof message !== 'string') {
    return null
  }
  return { kind, message }
}

/**
 * Classification a bare status suggests, used when the body carries no `kind`.
 * The mapping is the inverse of the service's own table.
 */
function kindFromStatus(status: number): ApiErrorKind {
  switch (status) {
    case 400:
      return 'invalid'
    case 404:
      return 'not_found'
    case 409:
      return 'conflict'
    case 413:
      return 'too_large'
    case 415:
      return 'unsupported'
    case 422:
      return 'unprocessable'
    case 503:
      return 'busy'
    default:
      return 'internal'
  }
}
