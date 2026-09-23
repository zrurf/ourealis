/*
 * The task endpoints.
 *
 * One submission call for every kind, because the service takes them all on one
 * route: the body is tagged by `kind`, so a plan, a build and a preview differ only
 * in that tag. The result is tagged the same way (`{route: …}` or `{map: …}`), which
 * is what lets the manager hand a caller the payload it asked for without knowing the
 * kind itself.
 */
import { api, collectPages, pageQuery, ApiClient } from './client'
import type { Page } from './types'
import type {
  TaskKind,
  TaskReply,
  TaskResult,
  TaskResultRef,
  TaskStateDto,
  TaskSubmit,
} from '@/types/tasks'

/** Submits a task and returns its ticket. */
export function submitTask(
  body: TaskSubmit,
  client: ApiClient = api,
  signal?: AbortSignal,
): Promise<TaskReply> {
  return client.post<TaskReply>('tasks', {
    body: JSON.stringify(body),
    signal,
  })
}

/** One task's state. */
export function getTask(
  id: string,
  client: ApiClient = api,
  signal?: AbortSignal,
): Promise<TaskStateDto> {
  return client.get<TaskStateDto>(`tasks/${encodeURIComponent(id)}`, { signal })
}

/** Tasks, newest first, optionally filtered by kind. */
export function listTasks(
  options: { kind?: TaskKind; offset?: number; limit?: number } = {},
  client: ApiClient = api,
  signal?: AbortSignal,
): Promise<Page<TaskStateDto>> {
  const { offset, limit } = pageQuery(options.offset ?? 0, options.limit ?? 100)
  return client.get<Page<TaskStateDto>>('tasks', {
    query: { ...(options.kind === undefined ? {} : { kind: options.kind }), offset, limit },
    signal,
  })
}

/** Every task the service still holds, newest first. */
export function listAllTasks(
  client: ApiClient = api,
  signal?: AbortSignal,
): Promise<TaskStateDto[]> {
  return collectPages<TaskStateDto>(
    (offset, limit) => listTasks({ offset, limit }, client, signal),
    { limit: 200 },
  )
}

/**
 * The result of a task that succeeded.
 *
 * A task that has not finished is a `409` rather than a `404`: the ticket is valid,
 * and telling the caller it does not exist would make it start over.
 */
export function getTaskResult(
  id: string,
  client: ApiClient = api,
  signal?: AbortSignal,
): Promise<TaskResult> {
  return client.get<TaskResult>(`tasks/${encodeURIComponent(id)}/result`, { signal })
}

/** Where a task's result lives, without fetching it. */
export function getTaskResultRef(
  id: string,
  client: ApiClient = api,
  signal?: AbortSignal,
): Promise<TaskResultRef> {
  return client.get<TaskResultRef>(`tasks/${encodeURIComponent(id)}/result/ref`, { signal })
}

/** Cancels a task. */
export function cancelTask(
  id: string,
  client: ApiClient = api,
  signal?: AbortSignal,
): Promise<void> {
  return client.delete<void>(`tasks/${encodeURIComponent(id)}`, { signal })
}
