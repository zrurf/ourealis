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
 *
 * The camera is a *viewport*: zoom step, pitch, field of view, projection and the sun
 * are settable while a map is open, because how a surface is read depends on the
 * question being asked of it. Everything that changes a vertex colour (the sun) or the
 * geometry (the camera itself) is applied here, and the views rebuild the terrain when
 * a baked parameter changes.
 */
import { ArcRotateCamera } from '@babylonjs/core/Cameras/arcRotateCamera'
import { Camera } from '@babylonjs/core/Cameras/camera'
import { Engine } from '@babylonjs/core/Engines/engine'
import { WebGPUEngine } from '@babylonjs/core/Engines/webgpuEngine'
import { DirectionalLight } from '@babylonjs/core/Lights/directionalLight'
import { HemisphericLight } from '@babylonjs/core/Lights/hemisphericLight'
import { StandardMaterial } from '@babylonjs/core/Materials/standardMaterial'
import { Color3, Color4 } from '@babylonjs/core/Maths/math.color'
import { Vector3 } from '@babylonjs/core/Maths/math.vector'
import type { LinesMesh } from '@babylonjs/core/Meshes/linesMesh'
import { Scene } from '@babylonjs/core/scene'
import type { Aabb } from '@/api/types'
import { parseCssColor } from '@/types/colormap'
import type { EngineBackend } from './engine'
import { DEFAULT_SUN_AZIMUTH_DEG, DEFAULT_SUN_ELEVATION_DEG, sunDirection } from './shading'

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
  /**
   * Identity of the canvas the engine draws into.
   *
   * A new engine gets a new canvas, so two views reporting the same value are drawing
   * through one device — which is the whole point of lending an engine rather than
   * building one per view.
   */
  canvasId: string | null
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
  canvasId: null,
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

/** Camera limits the viewport parameters are clamped to. */
export const CAMERA_LIMITS = {
  /** Smallest zoom step a wheel notch applies, as a fraction of the radius. */
  minZoomStep: 0.02,
  /** Largest zoom step, which is the point where one notch overshoots the map. */
  maxZoomStep: 0.3,
  /** Lowest pitch, which keeps the camera above the ground plane. */
  minPitchDeg: 5,
  /** Highest pitch: straight down, where the horizon flips. */
  maxPitchDeg: 88,
  /** Narrowest field of view. */
  minFovDeg: 20,
  /** Widest field of view. */
  maxFovDeg: 90,
} as const

/** The scene, camera and render loop of the map viewer. */
export class MapScene {
  /** Babylon scene holding every mesh. */
  readonly scene: Scene
  /** Orbit camera; drag rotates, wheel zooms, right-drag pans. */
  readonly camera: ArcRotateCamera

  private readonly engine: Engine | WebGPUEngine
  private readonly sky: HemisphericLight
  private readonly sunLight: DirectionalLight
  private readonly gridMaterial: StandardMaterial
  private cellGrid: LinesMesh | null = null
  private bounds: Aabb | null = null
  private exaggeration = 1
  private azimuthDeg = DEFAULT_SUN_AZIMUTH_DEG
  private elevationDeg = DEFAULT_SUN_ELEVATION_DEG
  private orthographic = false
  private disposed = false
  private loopRunning = false
  private readonly renderLoop: () => void
  private readonly resizeObserver: ResizeObserver | null = null

  /**
   * Meshes the height exaggeration applies to.
   *
   * The terrain, its slab, the overlays and the handles register here, so the viewer
   * sets the factor once and every height-carrying mesh follows without the caller
   * knowing their names. A mesh that stayed at the map's own metres would sink into a
   * stretched terrain and float over a flattened one.
   */
  private readonly renderTargets = new Set<{ scaling: { y: number } }>()

  private constructor(engine: Engine | WebGPUEngine, options: MapSceneOptions) {
    this.engine = engine
    // A canvas whose CSS size changes without the engine being told renders with the
    // old viewport and — worse — resolves picking rays with it, so a click lands
    // somewhere other than where the reader pointed. A panel that opens, a scrollbar
    // that appears and a window that is resized all do this.
    if (typeof ResizeObserver !== 'undefined') {
      this.resizeObserver = new ResizeObserver(() => engine.resize())
      this.resizeObserver.observe(options.canvas)
    }
    const scene = new Scene(engine)
    this.scene = scene
    scene.clearColor = surfaceColor()

    const camera = new ArcRotateCamera(
      'camera',
      -Math.PI / 2,
      Math.PI / 4,
      600,
      Vector3.Zero(),
      scene,
    )
    camera.attachControl(options.canvas, true)
    // Proportional zoom: a notch scales the radius instead of subtracting a fixed
    // number of metres, so the same gesture reads the same on a 40 m plan and a 4 km
    // map, and zooming out is the exact inverse of zooming in. A fixed divisor is what
    // made the wheel feel like it did nothing.
    camera.wheelDeltaPercentage = 0.1
    camera.wheelPrecision = 1
    camera.pinchDeltaPercentage = 0.001
    camera.panningSensibility = 30
    camera.inertia = 0.72
    // The depth range is what decides how two nearly coplanar surfaces resolve: a drape lies
    // centimetres above the ground it is drawn on, so a camera whose near plane is at half a
    // metre and whose far plane is a hundred kilometres apart spends its precision long before
    // those centimetres — which is what made a layer look offset at close range. Both limits
    // are set from the map's own size in `frameBounds`.
    camera.minZ = 0.25
    camera.maxZ = 20_000
    camera.lowerRadiusLimit = 5
    camera.upperRadiusLimit = 20_000
    // Below the ground the surface is culled and the reader sees its back faces; the
    // limits keep the camera in the hemisphere a map is read from.
    camera.lowerBetaLimit = (CAMERA_LIMITS.minPitchDeg * Math.PI) / 180
    camera.upperBetaLimit = (CAMERA_LIMITS.maxPitchDeg * Math.PI) / 180
    camera.useAutoRotationBehavior = false
    // Touch: one finger orbits (the same drag a mouse uses), two fingers pan and pinch.
    // Babylon's own gesture handling does all three; `useNaturalPinchZoom` is what makes
    // a pinch behave — it scales the radius by the change in finger separation, which is
    // what a reader expects, instead of the legacy delta form that depends on the
    // pinch's absolute distance.
    camera.useNaturalPinchZoom = true
    this.camera = camera

    // A white-model surface is shaped by light, so the fill term is low and the sun term
    // is not; the vertex colours already carry a hillshade, and a bright ambient would
    // flatten both.
    this.sky = new HemisphericLight('sky', new Vector3(0.2, 1, 0.1), scene)
    this.sky.intensity = 0.42
    this.sky.groundColor = new Color3(0.3, 0.3, 0.31)
    this.sunLight = new DirectionalLight('sun', this.lightVector(), scene)
    this.sunLight.intensity = 0.55

    // Depth cue: distant terrain fades into the page colour instead of ending at a
    // hard edge, which is most of what makes a large map readable at a glance.
    scene.fogMode = Scene.FOGMODE_LINEAR
    scene.fogColor = surfaceColor3()
    scene.fogStart = 400
    scene.fogEnd = 4_000

    // The optional unit grid shares the furniture colour; it is a ruler, not data.
    this.gridMaterial = new StandardMaterial('gridMaterial', scene)
    this.gridMaterial.emissiveColor = gridColor()
    this.gridMaterial.diffuseColor = new Color3(0, 0, 0)
    this.gridMaterial.specularColor = new Color3(0, 0, 0)
    this.gridMaterial.disableLighting = true

    // The loop is not started here: `start` and `stop` are the host's to call, because
    // a scene whose canvas is parked out of the layout has nothing to draw.
    this.renderLoop = () => {
      scene.render()
      debugState.frames += 1
      debugState.loaded = true
    }
  }

  /** Starts drawing, if the loop is not already running. */
  start(): void {
    if (this.loopRunning) {
      return
    }
    this.loopRunning = true
    this.engine.runRenderLoop(this.renderLoop)
    // A canvas that was moved between parents was resized without the engine noticing
    // in every browser; one call here keeps the viewport and the picking rays honest.
    this.engine.resize()
  }

  /**
   * Uses a reversed depth buffer when the backend supports one.
   *
   * With a reversed buffer the far end of the range keeps its precision instead of the near
   * end taking all of it, so a drape centimetres above the ground stops fighting it for depth.
   */
  enableReverseDepth(): void {
    try {
      this.engine.useReverseDepthBuffer = true
    } catch {
      // A backend without the extension keeps the plain depth buffer, which the tightened
      // near/far range already helps.
    }
  }

  /** Stops drawing. The context and every mesh survive. */
  stop(): void {
    if (!this.loopRunning) {
      return
    }
    this.loopRunning = false
    this.engine.stopRenderLoop()
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

  /** Zoom applied per wheel notch, as a fraction of the radius. */
  get zoomStep(): number {
    return this.camera.wheelDeltaPercentage
  }

  /** Sets the zoom step a wheel notch applies, clamped to a usable range. */
  setZoomStep(step: number): void {
    this.camera.wheelDeltaPercentage = clamp(
      step,
      CAMERA_LIMITS.minZoomStep,
      CAMERA_LIMITS.maxZoomStep,
    )
    this.camera.pinchDeltaPercentage = this.camera.wheelDeltaPercentage / 10
  }

  /** Whether the wheel zooms toward the pointer rather than the camera target. */
  get zoomToPointer(): boolean {
    return this.camera.zoomToMouseLocation
  }

  /** Reads the current pitch, degrees above the ground plane. */
  get pitchDeg(): number {
    return 90 - (this.camera.beta * 180) / Math.PI
  }

  /** Sets the pitch, clamped to the hemisphere a map is read from. */
  setPitch(deg: number): void {
    const clamped = clamp(deg, CAMERA_LIMITS.minPitchDeg, CAMERA_LIMITS.maxPitchDeg)
    this.camera.beta = ((90 - clamped) * Math.PI) / 180
  }

  /** Reads the vertical field of view, degrees. */
  get fovDeg(): number {
    return (this.camera.fov * 180) / Math.PI
  }

  /** Sets the vertical field of view, degrees. */
  setFov(deg: number): void {
    this.camera.fov = (clamp(deg, CAMERA_LIMITS.minFovDeg, CAMERA_LIMITS.maxFovDeg) * Math.PI) / 180
  }

  /**
   * Switches between the perspective view and a plan-like orthographic one.
   *
   * An orthographic camera is how a map is read for measurement: parallel walls stay
   * parallel and equal distances stay equal, which a perspective view cannot show.
   */
  setOrthographic(on: boolean): void {
    this.orthographic = on
    this.camera.mode = on ? Camera.ORTHOGRAPHIC_CAMERA : Camera.PERSPECTIVE_CAMERA
    this.applyOrthoWindow()
  }

  /** True while the camera renders without perspective. */
  get isOrthographic(): boolean {
    return this.orthographic
  }

  /** Whether the wheel zooms toward the pointer. */
  setZoomToPointer(on: boolean): void {
    this.camera.zoomToMouseLocation = on
  }

  /** Azimuth of the sun in the map plane, degrees clockwise from east. */
  get sunAzimuthDeg(): number {
    return this.azimuthDeg
  }

  /** Elevation of the sun above the horizon, degrees. */
  get sunElevationDeg(): number {
    return this.elevationDeg
  }

  /**
   * Points the sun, and thereby both the light and the baked hillshade.
   *
   * The direction is one value with two consumers: the light here and the vertex colours
   * the terrain rebuilds with. Neither is derived from the other, so a viewport that
   * moved only the light would draw shadows that disagree with the shading.
   */
  setSun(azimuthDeg: number, elevationDeg = this.elevationDeg): void {
    this.azimuthDeg = azimuthDeg
    this.elevationDeg = elevationDeg
    this.sunLight.direction = this.lightVector()
  }

  /** Direction from the surface toward the sun, for the vertex shading. */
  sunVector(): [number, number, number] {
    return sunDirection(this.azimuthDeg, this.elevationDeg)
  }

  /**
   * Frames a map extent: the camera and the fog.
   *
   * The radius is one and a quarter diagonals rather than more, so a map fills the frame
   * instead of sitting in the middle of empty background; the fog then starts well
   * beyond the far edge so it never washes out the map itself.
   */
  frameBounds(bounds: Aabb, maxElevation = 0): void {
    this.bounds = bounds
    const width = bounds.max_x - bounds.min_x
    const depth = bounds.max_y - bounds.min_y
    const centerX = (bounds.min_x + bounds.max_x) / 2
    const centerZ = (bounds.min_y + bounds.max_y) / 2
    const diagonal = Math.hypot(width, depth)
    this.camera.setTarget(new Vector3(centerX, (maxElevation * this.exaggeration) / 2, centerZ))
    // More than the diagonal: at a 45-degree tilt a radius of one diagonal puts the
    // map's near edge outside the viewport, so the extent has to be framed with room
    // for the perspective, not merely for its size.
    this.camera.radius = Math.max(30, diagonal * 1.25)
    this.camera.lowerRadiusLimit = Math.max(2, diagonal * 0.01)
    this.camera.upperRadiusLimit = Math.max(200, diagonal * 6)
    // Precision follows the map: the near plane sits at a fraction of the extent, so a
    // centimetre of lift is still a centimetre of depth where it matters.
    this.camera.minZ = Math.max(0.1, diagonal / 4000)
    this.camera.maxZ = Math.max(2000, diagonal * 4)
    this.scene.fogStart = diagonal * 1.4
    this.scene.fogEnd = diagonal * 5
    this.applyOrthoWindow()
  }

  /** Metre extent the camera was framed to, for a caller that needs it again. */
  get framedBounds(): Aabb | null {
    return this.bounds
  }

  /** Azimuth of the camera in the map plane, degrees clockwise from east. */
  get cameraAzimuthDeg(): number {
    return ((this.camera.alpha * 180) / Math.PI + 360) % 360
  }

  /**
   * Reads the camera back, so a control panel can show what the reader is looking at.
   *
   * The panel writes to the camera and the camera is also moved by the pointer, so without
   * this the two drift apart the moment anyone drags the view — the sliders then describe a
   * camera that no longer exists.
   */
  cameraState(): { pitchDeg: number; fovDeg: number; azimuthDeg: number; radius: number } {
    return {
      pitchDeg: this.pitchDeg,
      fovDeg: this.fovDeg,
      azimuthDeg: this.cameraAzimuthDeg,
      radius: this.camera.radius,
    }
  }

  /** Calls `listener` after the view matrix changed, throttled to once per animation frame. */
  observeCamera(listener: (state: ReturnType<MapScene['cameraState']>) => void): void {
    let pending = false
    this.camera.onViewMatrixChangedObservable.add(() => {
      if (pending) {
        return
      }
      pending = true
      // Once per frame: the observable fires several times per pointer move, and a control
      // panel does not need more than the frame it is drawn in.
      this.scene.onAfterRenderObservable.addOnce(() => {
        pending = false
        listener(this.cameraState())
      })
    })
  }

  /**
   * Puts the camera back where `frameBounds` left it.
   *
   * The map's own framing is the one orientation a reader can always return to: after a long
   * drag it is the only way to find the map again without hunting for it.
   */
  resetView(): void {
    const bounds = this.bounds
    if (bounds === null) {
      return
    }
    this.camera.alpha = -Math.PI / 2
    this.camera.beta = Math.PI / 4
    this.frameBounds(bounds)
  }

  /**
   * Length a scale bar should represent at a given ground resolution, metres.
   *
   * One, two or five times a power of ten, chosen so the bar spans a readable width:
   * a fixed length is a sliver on a large map and fills the screen on a small one.
   */
  scaleBarLength(metresPerPixel: number): number {
    const target = Math.max(1, metresPerPixel * 120)
    const magnitude = 10 ** Math.floor(Math.log10(target))
    const normalized = target / magnitude
    const step = normalized >= 5 ? 5 : normalized >= 2 ? 2 : 1
    return step * magnitude
  }

  /**
   * Draws a grid over the map, or removes it when switched off.
   *
   * Opt-in, and built from the terrain's own heights by the caller, so it follows the
   * ground instead of floating above the low parts and sinking into the high ones. Its
   * job is scale: a cell of a 1 m map is invisible at a whole-map view, and a 10 m grid
   * says at a glance how big what you are looking at is.
   */
  setCellGrid(mesh: LinesMesh | null): void {
    if (this.cellGrid !== null && this.cellGrid !== mesh) {
      this.untrackHeightMesh(this.cellGrid)
      this.cellGrid.dispose()
    }
    this.cellGrid = mesh
    if (mesh !== null) {
      mesh.material = this.gridMaterial
      mesh.isPickable = false
      this.trackHeightMesh(mesh)
    }
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
    this.resizeObserver?.disconnect()
    this.loopRunning = false
    this.engine.stopRenderLoop()
    this.scene.dispose()
    this.engine.dispose()
    updateDebug({ loaded: false, error: null, engine: null })
  }

  /** Direction from the surface toward the sun, in the scene's own terms. */
  private lightVector(): Vector3 {
    const [x, y, z] = sunDirection(this.azimuthDeg, this.elevationDeg)
    // The light travels from the sun toward the ground, so its direction is the
    // negative of the vector the shading uses.
    return new Vector3(-x, -y, -z)
  }

  /** Re-derives the orthographic window from the current radius and canvas shape. */
  private applyOrthoWindow(): void {
    if (!this.orthographic) {
      return
    }
    const aspect = this.engine.getRenderWidth() / Math.max(1, this.engine.getRenderHeight())
    const halfHeight = this.camera.radius * 0.5
    const halfWidth = halfHeight * aspect
    this.camera.orthoTop = halfHeight
    this.camera.orthoBottom = -halfHeight
    this.camera.orthoLeft = -halfWidth
    this.camera.orthoRight = halfWidth
  }
}

/** Creates and initialises a WebGPU engine. */
async function createWebGpuEngine(canvas: HTMLCanvasElement): Promise<WebGPUEngine> {
  const engine = new WebGPUEngine(canvas, { antialias: true })
  await engine.initAsync()
  return engine
}

/** Clamps a value into a range, mapping a non-finite value to the lower bound. */
function clamp(value: number, min: number, max: number): number {
  if (!Number.isFinite(value)) {
    return min
  }
  return Math.min(max, Math.max(min, value))
}

/** Reads a CSS custom property off the document, or the fallback when it is unset. */
export function tokenColor(name: string, fallback: string): string {
  if (typeof document === 'undefined') {
    return fallback
  }
  const value = getComputedStyle(document.documentElement).getPropertyValue(name).trim()
  return value === '' ? fallback : value
}

/** Scene background: the page's own surface colour, so the canvas does not look like a hole. */
function surfaceColor(): Color4 {
  const [r, g, b] = pageColour()
  return new Color4(r / 255, g / 255, b / 255, 1)
}

/** The same colour as a `Color3`, for the fog. */
function surfaceColor3(): Color3 {
  const [r, g, b] = pageColour()
  return new Color3(r / 255, g / 255, b / 255)
}

/** The page colour the canvas and the fog are both built from. */
function pageColour(): readonly [number, number, number] {
  return parseCssColor(tokenColor('--ourealis-page', '#ffffff'))
}

/** Grid line colour: a neutral that reads on both appearances. */
function gridColor(): Color3 {
  const [r, g, b] = parseCssColor(tokenColor('--ourealis-muted', '#6e6e73'))
  return new Color3((r / 255) * 0.6, (g / 255) * 0.6, (b / 255) * 0.6)
}
