/*
 * Scene, camera and the neutral ground furniture.
 *
 * The map is a local metre plane with y up, so a map position `(x, y)` becomes the
 * world position `(x, height, y)`: one scene unit is one metre and the height
 * exaggeration is a scale on the terrain meshes rather than a change to the
 * vertex data.
 *
 * `MapScene` owns the Babylon engine, the scene, the orbit camera and the render
 * loop. It is also the single place that publishes `window.__ourealis_debug`, the
 * object the e2e lane reads to assert that frames advanced and which backend was
 * chosen; nothing in the rendering path consults it, so the hook cannot affect
 * what is drawn.
 */
import { ArcRotateCamera } from '@babylonjs/core/Cameras/arcRotateCamera'
import { Engine } from '@babylonjs/core/Engines/engine'
import { WebGPUEngine } from '@babylonjs/core/Engines/webgpuEngine'
import { DirectionalLight } from '@babylonjs/core/Lights/directionalLight'
import { HemisphericLight } from '@babylonjs/core/Lights/hemisphericLight'
import { StandardMaterial } from '@babylonjs/core/Materials/standardMaterial'
import { Color3, Color4 } from '@babylonjs/core/Maths/math.color'
import { Vector3 } from '@babylonjs/core/Maths/math.vector'
import { CreateLineSystem } from '@babylonjs/core/Meshes/Builders/linesBuilder'
import type { LinesMesh } from '@babylonjs/core/Meshes/linesMesh'
import { Scene } from '@babylonjs/core/scene'
import type { Aabb } from '@/api/types'
import { parseCssColor } from '@/types/colormap'
import { gridSpacing } from '@/types/map'
import type { EngineBackend } from './engine'

/** The debug hook the e2e lane reads; kept out of every production code path. */
export interface OurealisDebug {
  /** Backend in use, or `null` while the viewer could not start. */
  engine: string | null
  /** Frames rendered since the scene started. */
  frames: number
  /** True once the first frame has been drawn. */
  loaded: boolean
  /** Map the viewer is showing, or `null` before one is opened. */
  mapId: string | null
  /** Failure message, or `null` while the viewer is healthy. */
  error: string | null
}

/** Extends the global scope with the debug hook the e2e lane expects. */
declare global {
  /** Debug surface written by {@link updateDebug}. */
  interface Window {
    /** Renderer state, published after the scene starts and on teardown. */
    __ourealis_debug?: OurealisDebug
  }
}

/** Live state behind the debug hook; the published object is this one, not a copy. */
const debugState: OurealisDebug = {
  engine: null,
  frames: 0,
  loaded: false,
  mapId: null,
  error: null,
}

/**
 * Updates the debug state and hands the object to the page.
 *
 * The reference is published, not a copy, so the frame counter the render loop
 * keeps incrementing is the number a test reads without the loop touching the
 * window on every frame.
 */
export function updateDebug(patch: Partial<OurealisDebug>): void {
  Object.assign(debugState, patch)
  if (typeof window !== 'undefined') {
    // The property name is part of the e2e contract, so it is written through a
    // string key rather than renamed.
    window['__ourealis_debug'] = debugState
  }
}

/** Reads the debug state; the viewer uses it only to report a failure reason. */
export function debugSnapshot(): OurealisDebug {
  return { ...debugState }
}

/** Options of {@link MapScene.create}. */
export interface MapSceneOptions {
  /** Canvas the engine draws into. */
  canvas: HTMLCanvasElement
  /** Backend the probe selected. */
  backend: EngineBackend
}

/** Grid spacing that keeps a large map's line count sane, metres. */
const TARGET_GRID_LINES = 40

/** Length of the scale bar, metres; the view labels it with its own length. */
export const SCALE_BAR_METRES = 100

/** The scene, camera and render loop of the map viewer. */
export class MapScene {
  /** Babylon scene holding every mesh. */
  readonly scene: Scene
  /** Orbit camera; drag rotates, wheel zooms, right-drag pans. */
  readonly camera: ArcRotateCamera

  private readonly engine: Engine | WebGPUEngine
  private readonly gridMaterial: StandardMaterial
  private gridMesh: LinesMesh | null = null
  private scaleBar: LinesMesh | null = null
  private scaleBarMetres = SCALE_BAR_METRES
  private bounds: Aabb | null = null
  private exaggeration = 1
  private disposed = false

  /**
   * Meshes the height exaggeration applies to.
   *
   * The terrain registers itself here, so the viewer sets the factor once and
   * every height-carrying mesh follows without the caller knowing their names.
   */
  private readonly renderTargets = new Set<{ scaling: { y: number } }>()

  private constructor(engine: Engine | WebGPUEngine, options: MapSceneOptions) {
    this.engine = engine
    const scene = new Scene(engine)
    this.scene = scene
    scene.clearColor = surfaceColor()

    const camera = new ArcRotateCamera(
      'camera',
      -Math.PI / 2,
      Math.PI / 3,
      600,
      Vector3.Zero(),
      scene,
    )
    camera.attachControl(options.canvas, true)
    camera.wheelPrecision = 3
    camera.panningSensibility = 30
    camera.minZ = 0.5
    camera.maxZ = 100_000
    camera.lowerRadiusLimit = 5
    camera.upperRadiusLimit = 20_000
    camera.useAutoRotationBehavior = false
    this.camera = camera

    const sky = new HemisphericLight('sky', new Vector3(0.2, 1, 0.1), scene)
    sky.intensity = 0.75
    sky.groundColor = new Color3(0.35, 0.35, 0.36)
    const sun = new DirectionalLight('sun', new Vector3(-0.5, -1, -0.3), scene)
    sun.intensity = 0.5

    this.gridMaterial = new StandardMaterial('gridMaterial', scene)
    this.gridMaterial.emissiveColor = gridColor()
    this.gridMaterial.diffuseColor = new Color3(0, 0, 0)
    this.gridMaterial.specularColor = new Color3(0, 0, 0)
    this.gridMaterial.disableLighting = true

    engine.runRenderLoop(() => {
      scene.render()
      debugState.frames += 1
      debugState.loaded = true
    })
  }

  /**
   * Builds the engine of the probed backend and starts the loop.
   *
   * The WebGPU engine is asynchronous by construction: its device is requested
   * during `initAsync`, and rendering before that resolves draws nothing.
   */
  static async create(options: MapSceneOptions): Promise<MapScene> {
    const engine =
      options.backend === 'webgpu'
        ? await createWebGpuEngine(options.canvas)
        : new Engine(options.canvas, true, { preserveDrawingBuffer: true, stencil: true }, true)
    const scene = new MapScene(engine, options)
    updateDebug({ engine: options.backend, frames: 0, loaded: false, error: null })
    return scene
  }

  /** Height exaggeration applied to the terrain, 1 being the map's own metres. */
  get exaggerationFactor(): number {
    return this.exaggeration
  }

  /** Sets the height exaggeration; terrain meshes scale, nothing is rebuilt. */
  setExaggeration(factor: number): void {
    const clamped = Math.min(5, Math.max(0, factor))
    this.exaggeration = clamped
    this.renderTargets.forEach((mesh) => {
      mesh.scaling.y = clamped
    })
    this.placeScaleBar()
  }

  /** Registers a height-carrying mesh with the exaggeration control. */
  trackHeightMesh(mesh: { scaling: { y: number } }): void {
    mesh.scaling.y = this.exaggeration
    this.renderTargets.add(mesh)
  }

  /** Stops tracking a disposed mesh. */
  untrackHeightMesh(mesh: { scaling: { y: number } }): void {
    this.renderTargets.delete(mesh)
  }

  /** Frames a map extent and redraws the grid under it. */
  frameBounds(bounds: Aabb, maxElevation = 0): void {
    this.bounds = bounds
    const width = bounds.max_x - bounds.min_x
    const depth = bounds.max_y - bounds.min_y
    const centerX = (bounds.min_x + bounds.max_x) / 2
    const centerZ = (bounds.min_y + bounds.max_y) / 2
    this.camera.setTarget(new Vector3(centerX, (maxElevation * this.exaggeration) / 2, centerZ))
    this.camera.radius = Math.max(60, Math.hypot(width, depth) * 1.1)
    this.drawGrid()
    this.placeScaleBar()
  }

  /** Redraws the ground grid: one line every `spacing` metres, in the neutral grid colour. */
  private drawGrid(): void {
    const bounds = this.bounds
    if (bounds === null) {
      return
    }
    this.gridMesh?.dispose()
    const width = bounds.max_x - bounds.min_x
    const depth = bounds.max_y - bounds.min_y
    const spacing = gridSpacing(width, depth, TARGET_GRID_LINES)
    const lines: Vector3[][] = []
    for (let x = bounds.min_x; x <= bounds.max_x + 1e-6; x += spacing) {
      lines.push([new Vector3(x, 0, bounds.min_y), new Vector3(x, 0, bounds.max_y)])
    }
    for (let z = bounds.min_y; z <= bounds.max_y + 1e-6; z += spacing) {
      lines.push([new Vector3(bounds.min_x, 0, z), new Vector3(bounds.max_x, 0, z)])
    }
    const grid = CreateLineSystem('grid', { lines, updatable: false }, this.scene)
    grid.material = this.gridMaterial
    grid.isPickable = false
    this.gridMesh = grid
  }

  /**
   * Places the scale bar at the map's south-west corner.
   *
   * It sits above the terrain so a raised surface cannot hide it, and it keeps its
   * length through an exaggeration change: the bar measures ground distance, which
   * the vertical scale does not affect.
   */
  private placeScaleBar(): void {
    const bounds = this.bounds
    if (bounds === null) {
      return
    }
    this.scaleBar?.dispose()
    const length = Math.min(SCALE_BAR_METRES, Math.max(1, bounds.max_x - bounds.min_x))
    const inset = length * 0.05
    const x0 = bounds.min_x + inset
    const z = bounds.min_y + inset
    const bar = CreateLineSystem(
      'scaleBar',
      {
        lines: [
          [new Vector3(x0, 0, z), new Vector3(x0 + length, 0, z)],
          [new Vector3(x0, 0, z), new Vector3(x0, 1, z)],
          [new Vector3(x0 + length, 0, z), new Vector3(x0 + length, 1, z)],
        ],
      },
      this.scene,
    )
    bar.material = this.gridMaterial
    bar.isPickable = false
    bar.position.y = 1
    this.scaleBar = bar
    this.scaleBarMetres = length
  }

  /** Length the drawn scale bar represents, metres. */
  get scaleBarLength(): number {
    return this.scaleBarMetres
  }

  /** Frames the whole scene again after the canvas was resized. */
  resize(): void {
    this.engine.resize()
  }

  /** Stops the loop, disposes the scene and closes the debug hook. */
  dispose(): void {
    if (this.disposed) {
      return
    }
    this.disposed = true
    this.engine.stopRenderLoop()
    this.scene.dispose()
    this.engine.dispose()
    updateDebug({ loaded: false, error: null, engine: null })
  }
}

/** Creates and initialises a WebGPU engine. */
async function createWebGpuEngine(canvas: HTMLCanvasElement): Promise<WebGPUEngine> {
  const engine = new WebGPUEngine(canvas, { antialias: true })
  await engine.initAsync()
  return engine
}

/** Reads a CSS custom property off the document, or the fallback when it is unset. */
function tokenColor(name: string, fallback: string): string {
  if (typeof document === 'undefined') {
    return fallback
  }
  const value = getComputedStyle(document.documentElement).getPropertyValue(name).trim()
  return value === '' ? fallback : value
}

/** Scene background: the page's own surface colour, so the canvas does not look like a hole. */
function surfaceColor(): Color4 {
  const [r, g, b] = parseCssColor(tokenColor('--ourealis-page', '#ffffff'))
  return new Color4(r / 255, g / 255, b / 255, 1)
}

/** Grid line colour: a neutral that reads on both appearances. */
function gridColor(): Color3 {
  const [r, g, b] = parseCssColor(tokenColor('--ourealis-muted', '#6e6e73'))
  return new Color3((r / 255) * 0.6, (g / 255) * 0.6, (b / 255) * 0.6)
}
