/*
 * The render host: one engine for the session, lent to one view at a time.
 *
 * An engine costs a graphics device — a WebGPU adapter request or a WebGL context —
 * and creating one takes hundreds of milliseconds to seconds. Every map-bearing view
 * used to have its own, so moving between the preview, the workspace and a trajectory
 * paid that cost again each time, and two views alive at once held two devices.
 *
 * A Babylon engine is bound to the canvas it was built on for its whole life, so the
 * host owns the canvas and *moves* it: the canvas is appended into the container that
 * asks for it, and when the lease is released it is parked in a hidden dock. Moving a
 * canvas in the DOM does not touch its context, which is what makes the device
 * survive the trip.
 *
 * The render loop runs only while a view holds a lease. A docked canvas draws nothing
 * a reader can see, and a loop on one still burns the GPU and keeps the frame counter
 * in the debug hook climbing, which would make that hook useless to the e2e lane.
 */
import { MapScene, updateDebug } from './scene'
import type { EngineBackend } from './engine'

/** A view's hold on the shared engine. */
export interface SceneLease {
  /** The scene to draw with. */
  readonly scene: MapScene
  /** The canvas it draws into, already inside the caller's container. */
  readonly canvas: HTMLCanvasElement
  /** Gives the engine back. Safe to call twice. */
  release(): void
}

/** The host. One instance for the page. */
class RenderHost {
  private canvas: HTMLCanvasElement | null = null
  private dock: HTMLElement | null = null
  private scene: MapScene | null = null
  private backend: EngineBackend | null = null
  private starting: Promise<MapScene> | null = null
  private held = false

  /**
   * Borrows the engine, creating it on first use.
   *
   * A backend that differs from the running engine's — `?engine=webgl2` after a
   * WebGPU session, or a probe that fell back — replaces it: the canvas of a live
   * engine cannot be re-created on another backend, and the debug hook's contract is
   * that the reported backend is the one drawing.
   */
  async acquire(container: HTMLElement, backend: EngineBackend): Promise<SceneLease> {
    if (this.scene !== null && this.backend !== null && this.backend !== backend) {
      this.destroy()
    }
    if (this.scene === null) {
      this.starting ??= this.create(backend)
      try {
        this.scene = await this.starting
      } finally {
        this.starting = null
      }
      this.backend = backend
    }
    const canvas = this.canvas
    const scene = this.scene
    if (canvas === null || scene === null) {
      throw new Error('the render host could not build a scene')
    }
    container.appendChild(canvas)
    // The engine's size follows the canvas, which has just changed parent and size.
    scene.resize()
    scene.start()
    this.held = true
    return {
      scene,
      canvas,
      release: () => {
        if (!this.held) {
          return
        }
        this.held = false
        // The callbacks belong to the view that set them; a stale one would act on a
        // component that no longer exists.
        scene.scene.onPointerDown = undefined
        scene.scene.onPointerMove = undefined
        scene.scene.onPointerUp = undefined
        scene.stop()
        updateDebug({ loaded: false, mapId: null, error: null })
        this.dockCanvas()
      },
    }
  }

  /** Disposes the engine outright; the next `acquire` builds a new one. */
  destroy(): void {
    this.scene?.dispose()
    this.scene = null
    this.backend = null
    updateDebug({ canvasId: null })
    this.canvas?.remove()
    this.canvas = null
    this.dock?.remove()
    this.dock = null
    this.held = false
  }

  /** Builds the canvas and its scene. */
  private async create(backend: EngineBackend): Promise<MapScene> {
    const canvas = document.createElement('canvas')
    canvas.className = 'block h-full w-full'
    canvas.setAttribute('data-testid', 'shared-canvas')
    const scene = await MapScene.create({ canvas, backend })
    this.canvas = canvas
    updateDebug({ canvasId: canvasIdOf(canvas) })
    return scene
  }

  /** Parks the canvas out of the layout, keeping its context alive. */
  private dockCanvas(): void {
    const canvas = this.canvas
    if (canvas === null) {
      return
    }
    this.dock ??= this.createDock()
    this.dock.appendChild(canvas)
  }

  /** The hidden element a released canvas waits in. */
  private createDock(): HTMLElement {
    const dock = document.createElement('div')
    dock.setAttribute('aria-hidden', 'true')
    // Out of the flow and invisible, but *not* `display: none`: a canvas with no box
    // has zero size, and a resized-to-zero drawing buffer is not always restored.
    dock.style.position = 'fixed'
    dock.style.left = '-10000px'
    dock.style.top = '0'
    dock.style.width = '1px'
    dock.style.height = '1px'
    dock.style.overflow = 'hidden'
    document.body.appendChild(dock)
    return dock
  }
}

/** Identity of a canvas, stamped once and kept on the element. */
const CANVAS_ID = 'ourealisCanvasId'

/** Reads a canvas's identity, stamping a new one the first time it is asked. */
function canvasIdOf(canvas: HTMLCanvasElement): string {
  const existing = canvas.dataset[CANVAS_ID]
  if (existing !== undefined) {
    return existing
  }
  const created = `canvas-${Math.random().toString(36).slice(2, 10)}`
  canvas.dataset[CANVAS_ID] = created
  return created
}

/** The page's host. */
let host: RenderHost | null = null

/** The render host of this page. */
export function renderHost(): RenderHost {
  host ??= new RenderHost()
  return host
}
