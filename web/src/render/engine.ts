/*
 * Renderer backend selection.
 *
 * The chain is WebGPU, then WebGL2, then WebGL, and the first one that answers
 * with a usable context wins; when none does, the viewer reports the failure
 * instead of showing an empty canvas. Probing is separated from engine creation
 * so it can run — and be tested — without a browser: everything it touches is
 * passed in, and the browser's own `navigator.gpu` and canvas factory are the
 * defaults.
 *
 * `?engine=` forces a backend for testing and for a side-by-side comparison of
 * the fallback path. Forcing a backend the machine cannot provide is reported as
 * a failure rather than silently falling back: a forced run is a test, and a test
 * that quietly used another backend would prove nothing.
 */

/** A backend the viewer can render with. */
export type EngineBackend = 'webgpu' | 'webgl2' | 'webgl'

/** Backends in probe order, each a child of the one before it. */
export const BACKEND_ORDER: readonly EngineBackend[] = ['webgpu', 'webgl2', 'webgl']

/**
 * Why the probe settled where it did; a UI translates the code, while `detail`
 * carries the diagnostic in English.
 *
 * Only reachable outcomes are named: a fallback to a working backend is
 * `available`, and the reason a device cannot render at all is that no context
 * could be created, whether or not a WebGPU API was present.
 */
export type EngineReasonCode = 'available' | 'forced' | 'forced-unavailable' | 'no-webgl'

/** Outcome of a probe. */
export interface EngineProbe {
  /** Backend to build the scene with, or `null` when none is usable. */
  backend: EngineBackend | null
  /** Machine-readable reason for the outcome. */
  reason: EngineReasonCode
  /** Diagnostic detail, English, for the tooltip and the console; never translated. */
  detail: string
  /** True when `?engine=` chose the backend. */
  forced: boolean
}

/** The slice of `navigator.gpu` the probe uses. */
export interface GpuLike {
  /** Resolves with an adapter, or with `null` when the browser has no usable device. */
  requestAdapter(options?: Record<string, unknown>): Promise<unknown>
}

/** The slice of a canvas the probe uses. */
export interface CanvasLike {
  /** Returns a drawing context for a context id, or `null` when the id is unsupported. */
  getContext(contextId: string): unknown
}

/** Everything the probe reads from its environment. */
export interface ProbeEnvironment {
  /** `navigator.gpu`, when the browser has WebGPU. */
  gpu?: GpuLike | null
  /** Builds a throwaway canvas for the WebGL probes. */
  createCanvas?: () => CanvasLike
  /** Backend forced by `?engine=`, when the URL named one. */
  forced?: EngineBackend | null
}

/** Parses the `engine` query value; anything unrecognised is not a forced backend. */
export function forcedBackend(value: unknown): EngineBackend | null {
  return typeof value === 'string' && (BACKEND_ORDER as readonly string[]).includes(value)
    ? (value as EngineBackend)
    : null
}

/** Reads `?engine=` from a query string, defaulting to the current location. */
export function engineFromQuery(search?: string): EngineBackend | null {
  const query = search ?? (typeof window === 'undefined' ? '' : window.location.search)
  return forcedBackend(new URLSearchParams(query).get('engine'))
}

/**
 * Probes the backends in order.
 *
 * A WebGPU probe counts as successful only when an adapter actually resolves:
 * `navigator.gpu` exists on some browsers with no usable device, and treating
 * that as available would put the viewer on an engine that cannot draw.
 */
export async function probeEngine(environment: ProbeEnvironment = {}): Promise<EngineProbe> {
  if (environment.forced !== undefined && environment.forced !== null) {
    const forced = environment.forced
    const usable = await isBackendUsable(forced, environment)
    return usable
      ? {
          backend: forced,
          reason: 'forced',
          detail: `backend forced by ?engine=${forced}`,
          forced: true,
        }
      : {
          backend: null,
          reason: 'forced-unavailable',
          detail: `?engine=${forced} was requested but this device has no usable ${forced} context`,
          forced: true,
        }
  }
  for (const backend of BACKEND_ORDER) {
    // The chain is ordered: the next backend is only probed when this one failed.
    // oxlint-disable-next-line no-await-in-loop
    if (await isBackendUsable(backend, environment)) {
      return {
        backend,
        reason: 'available',
        detail: backend === 'webgpu' ? 'WebGPU adapter obtained' : `selected ${backend}`,
        forced: false,
      }
    }
  }
  return { backend: null, reason: 'no-webgl', detail: detailForFailure(environment), forced: false }
}

/** Diagnostic text for a chain that exhausted: which pieces were missing, in order. */
function detailForFailure(environment: ProbeEnvironment): string {
  const gpu = environment.gpu ?? browserGpu()
  const gpuNote = gpu === null || gpu === undefined ? 'no WebGPU API' : 'no WebGPU adapter'
  return `${gpuNote}; no WebGL2 or WebGL context either`
}

/** True when one backend can be created in this environment. */
async function isBackendUsable(
  backend: EngineBackend,
  environment: ProbeEnvironment,
): Promise<boolean> {
  if (backend === 'webgpu') {
    const gpu = environment.gpu ?? browserGpu()
    if (gpu === null || gpu === undefined) {
      return false
    }
    try {
      return (await gpu.requestAdapter()) !== null
    } catch {
      // A browser can reject the request outright; that is not a usable adapter.
      return false
    }
  }
  const create = environment.createCanvas ?? browserCanvas
  if (create === undefined) {
    return false
  }
  try {
    const canvas = create()
    return canvas.getContext(backend === 'webgl2' ? 'webgl2' : 'webgl') !== null
  } catch {
    return false
  }
}

/** `navigator.gpu` of the current page, or `null` when the browser has none. */
function browserGpu(): GpuLike | null {
  if (typeof navigator === 'undefined') {
    return null
  }
  const gpu: unknown = (navigator as unknown as { gpu?: unknown }).gpu
  return typeof gpu === 'object' && gpu !== null ? (gpu as GpuLike) : null
}

/** Builds a throwaway canvas in a browser, or `undefined` outside one. */
function browserCanvas(): CanvasLike {
  if (typeof document === 'undefined') {
    throw new Error('no canvas factory is available in this environment')
  }
  return document.createElement('canvas')
}

/** Human-readable name of a backend, as the status chip shows it. */
export function backendLabel(backend: EngineBackend): string {
  switch (backend) {
    case 'webgpu':
      return 'WebGPU'
    case 'webgl2':
      return 'WebGL2'
    case 'webgl':
      return 'WebGL'
  }
}
