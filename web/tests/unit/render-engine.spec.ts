/*
 * Renderer backend selection.
 *
 * The probe takes its environment as an argument, so every branch of the chain —
 * WebGPU, a browser with `navigator.gpu` but no adapter, WebGL2, WebGL, nothing at
 * all — is exercised here without a browser or a GPU.
 */
import { expect, test } from '@playwright/test'
import {
  BACKEND_ORDER,
  backendLabel,
  engineFromQuery,
  forcedBackend,
  probeEngine,
  type CanvasLike,
  type GpuLike,
  type ProbeEnvironment,
} from '../../src/render/engine'
import { gridSpacing } from '../../src/types/map'

/** A GPU stub that resolves with an adapter, or with null when it has none. */
function gpuWith(adapter: unknown): GpuLike {
  return { requestAdapter: async () => adapter }
}

/** A GPU stub whose request rejects, which some browsers do instead of answering null. */
function gpuRejecting(): GpuLike {
  return {
    requestAdapter: async () => {
      throw new Error('requestAdapter is not allowed here')
    },
  }
}

/** A canvas factory that answers the given context ids and nothing else. */
function canvasSupporting(...contexts: string[]): () => CanvasLike {
  return () => ({
    getContext: (contextId: string) => (contexts.includes(contextId) ? {} : null),
  })
}

/** A canvas factory that throws, as a detached or resized canvas does. */
function canvasThrowing(): CanvasLike {
  return {
    getContext: () => {
      throw new Error('context creation failed')
    },
  }
}

test.describe('backend probe', () => {
  test('the chain is WebGPU, then WebGL2, then WebGL', () => {
    expect(BACKEND_ORDER).toEqual(['webgpu', 'webgl2', 'webgl'])
    expect(backendLabel('webgpu')).toBe('WebGPU')
    expect(backendLabel('webgl2')).toBe('WebGL2')
    expect(backendLabel('webgl')).toBe('WebGL')
  })

  test('an adapter selects WebGPU', async () => {
    const probe = await probeEngine({
      gpu: gpuWith({ name: 'fake adapter' }),
      createCanvas: canvasSupporting('webgl2', 'webgl'),
    })
    expect(probe.backend).toBe('webgpu')
    expect(probe.reason).toBe('available')
    expect(probe.forced).toBe(false)
  })

  test('no adapter falls through to WebGL2', async () => {
    const probe = await probeEngine({
      gpu: gpuWith(null),
      createCanvas: canvasSupporting('webgl2', 'webgl'),
    })
    expect(probe.backend).toBe('webgl2')
    expect(probe.reason).toBe('available')
  })

  test('a rejected adapter request falls through as well', async () => {
    const probe = await probeEngine({
      gpu: gpuRejecting(),
      createCanvas: canvasSupporting('webgl2'),
    })
    expect(probe.backend).toBe('webgl2')
  })

  test('a browser without WebGL2 lands on WebGL', async () => {
    const probe = await probeEngine({
      gpu: undefined,
      createCanvas: canvasSupporting('webgl'),
    })
    expect(probe.backend).toBe('webgl')
    expect(probe.reason).toBe('available')
  })

  test('no backend at all is reported rather than guessed', async () => {
    const probe = await probeEngine({
      gpu: gpuWith(null),
      createCanvas: canvasSupporting(),
    })
    expect(probe.backend).toBeNull()
    expect(probe.reason).toBe('no-webgl')
    expect(probe.detail).toContain('no WebGPU adapter')
  })

  test('a browser without the WebGPU API is reported once WebGL fails too', async () => {
    const environment: ProbeEnvironment = { gpu: undefined, createCanvas: canvasSupporting() }
    const probe = await probeEngine(environment)
    expect(probe.backend).toBeNull()
    expect(probe.reason).toBe('no-webgl')
    expect(probe.detail).toContain('no WebGPU API')
  })

  test('a canvas that cannot be asked for a context is not fatal', async () => {
    const probe = await probeEngine({ gpu: undefined, createCanvas: () => canvasThrowing() })
    expect(probe.backend).toBeNull()
    expect(probe.reason).toBe('no-webgl')
  })
})

test.describe('ground grid spacing', () => {
  test('the step is a round number that keeps the line count near the target', () => {
    // 600 m over 40 lines is 15 m, rounded down to the nearest round step of 10 m.
    expect(gridSpacing(600, 400)).toBe(10)
    expect(gridSpacing(3000, 3000)).toBe(50)
    expect(gridSpacing(40, 40)).toBe(1)
    // A degenerate extent still gets a step rather than a division by zero.
    expect(gridSpacing(0, 0)).toBe(1)
  })

  test('the target line count is respected when it is given', () => {
    expect(gridSpacing(600, 400, 4)).toBe(100)
  })
})

test.describe('forced backend', () => {
  test('the query value names one of the three backends', () => {
    expect(forcedBackend('webgl2')).toBe('webgl2')
    expect(forcedBackend('webgpu')).toBe('webgpu')
    expect(forcedBackend('webgl')).toBe('webgl')
    expect(forcedBackend('nonsense')).toBeNull()
    expect(forcedBackend(null)).toBeNull()
    expect(forcedBackend(42)).toBeNull()
  })

  test('a query string is parsed, an absent parameter forces nothing', () => {
    expect(engineFromQuery('?engine=webgl2')).toBe('webgl2')
    expect(engineFromQuery('?engine=webgl2&other=1')).toBe('webgl2')
    expect(engineFromQuery('?other=1')).toBeNull()
    expect(engineFromQuery('')).toBeNull()
  })

  test('a forced backend that is usable is selected and marked as forced', async () => {
    const probe = await probeEngine({
      gpu: gpuWith({ name: 'adapter' }),
      createCanvas: canvasSupporting('webgl2'),
      forced: 'webgl2',
    })
    expect(probe.backend).toBe('webgl2')
    expect(probe.forced).toBe(true)
    expect(probe.reason).toBe('forced')
  })

  test('a forced backend that is unavailable fails instead of silently falling back', async () => {
    const probe = await probeEngine({
      gpu: gpuWith(null),
      createCanvas: canvasSupporting('webgl2', 'webgl'),
      forced: 'webgpu',
    })
    expect(probe.backend).toBeNull()
    expect(probe.reason).toBe('forced-unavailable')
    expect(probe.forced).toBe(true)
    expect(probe.detail).toContain('webgpu')
  })

  test('forcing WebGL keeps the viewer off WebGPU even when an adapter exists', async () => {
    const probe = await probeEngine({
      gpu: gpuWith({ name: 'adapter' }),
      createCanvas: canvasSupporting('webgl2', 'webgl'),
      forced: 'webgl',
    })
    expect(probe.backend).toBe('webgl')
    expect(probe.forced).toBe(true)
  })
})
