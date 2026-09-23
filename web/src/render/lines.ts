/*
 * Ribbon polylines on the ground: routes, candidate paths and the scale grid.
 *
 * All three are the same object — a list of paths drawn as metre-wide bands — so they
 * share one builder. `GreasedLine` is what makes the width mean metres; a screen-space
 * line would be the only thing visible on a map where a route is a few hundred metres
 * long and a few metres wide.
 *
 * Every mesh built here registers with the scene's height exaggeration, so a line draped
 * on the terrain stays on it when the vertical scale changes.
 */
import { Color3 } from '@babylonjs/core/Maths/math.color'
import { CreateGreasedLine } from '@babylonjs/core/Meshes/Builders/greasedLineBuilder'
import { GreasedLineMeshColorMode } from '@babylonjs/core/Materials/GreasedLine/greasedLineMaterialInterfaces'
import type { AbstractMesh } from '@babylonjs/core/Meshes/abstractMesh'
import type { Scene } from '@babylonjs/core/scene'
import { PathFlow } from './pathFlow'
import type { MapScene } from './scene'
import type { WorldPoint } from './overlayGeometry'

/** One polyline to draw, with the style that tells it apart from its neighbours. */
export interface PathStyle {
  /** Draped points, world metres. */
  points: WorldPoint[]
  /** Line colour, `#rrggbb`. */
  colour: string
  /** Line width in metres. */
  widthM: number
  /** Opacity in `[0, 1]`. */
  alpha?: number
  /** Draw order inside the layer; higher wins a shared depth. */
  zOffset?: number
  /**
   * Send water running along the line, from its first point to its last.
   *
   * See `render/pathFlow.ts`: the water is geometry this module places rather than a dash pattern,
   * so it covers a multi-segment path in full and its speed is stated in metres per second.
   */
  flow?: {
    /** How many slugs are in the pipe at once. */
    slugs: number
    /** Length of one slug, metres. */
    lengthM: number
    /** Width of a slug, metres. */
    widthM: number
    /** How fast the water moves, metres per second. */
    speed: number
    /** Colour of the water; the line's own colour when absent. */
    colour?: string
  }
}

/**
 * A set of polylines drawn as one mesh each.
 *
 * The set owns what it builds: `set` disposes the previous paths, and `dispose` releases
 * everything, so a view that redraws on every pointer move cannot leak meshes.
 */
export class PathSet {
  private readonly mapScene: MapScene
  private readonly scene: Scene
  private items: AbstractMesh[] = []
  private flows: PathFlow[] = []
  private observer: { remove(): void } | null = null

  constructor(mapScene: MapScene) {
    this.mapScene = mapScene
    this.scene = mapScene.scene
  }

  /** Replaces every path. */
  set(paths: readonly PathStyle[]): void {
    this.clear()
    for (const path of paths) {
      if (path.points.length < 2) {
        continue
      }
      this.items.push(this.build(path))
    }
    this.startFlow()
  }

  /**
   * Animates the dashed paths.
   *
   * `dashOffset` is in metres along the line, so advancing it by `speed * dt` makes the dashes
   * travel at a stated speed rather than at whatever the frame rate happens to be.
   */
  private startFlow(): void {
    if (this.flows.length === 0 || this.observer !== null) {
      return
    }
    let last = Date.now()
    const observable = this.scene.onBeforeRenderObservable.add(() => {
      const now = Date.now()
      const dt = Math.min(0.1, (now - last) / 1000)
      last = now
      for (const flow of this.flows) {
        flow.advance(dt)
      }
    })
    this.observer = {
      remove: () => {
        this.scene.onBeforeRenderObservable.remove(observable)
      },
    }
  }

  /** Removes every mesh, keeping the set usable. */
  clear(): void {
    for (const flow of this.flows) {
      flow.dispose()
    }
    this.flows = []
    for (const mesh of this.items) {
      this.mapScene.untrackHeightMesh(mesh)
      mesh.dispose()
    }
    this.items = []
    this.flows = []
    this.observer?.remove()
    this.observer = null
  }

  /** Meshes currently drawn, for a caller that toggles their visibility. */
  get meshes(): readonly AbstractMesh[] {
    return this.items
  }

  /** True while at least one path is drawn. */
  get size(): number {
    return this.items.length
  }

  /** Disposes every mesh. */
  dispose(): void {
    this.clear()
  }

  /** Builds one ribbon mesh and registers it with the height exaggeration. */
  private build(path: PathStyle): AbstractMesh {
    const mesh = CreateGreasedLine(
      'path',
      { points: [path.points.flat()] },
      {
        width: path.widthM,
        color: Color3.FromHexString(path.colour),
        sizeAttenuation: false,
        colorMode: GreasedLineMeshColorMode.COLOR_MODE_SET,
      },
      this.scene,
    )
    mesh.isPickable = false
    // The highlighted route is drawn above its neighbours, which is the whole point of
    // highlighting it: a line half a metre below another is not "selected".
    mesh.renderingGroupId = path.zOffset !== undefined && path.zOffset <= -6 ? 3 : 1
    if (mesh.material !== null) {
      mesh.material.alpha = path.alpha ?? 1
      // A small depth offset keeps the band above the ground it hugs; a large one drew
      // it through the terrain where the ground was higher than the line.
      mesh.material.zOffset = path.zOffset ?? -2
    }
    this.mapScene.trackHeightMesh(mesh)
    if (path.flow !== undefined) {
      this.flows.push(
        new PathFlow(this.mapScene, path.points, {
          slugs: path.flow.slugs,
          lengthM: path.flow.lengthM,
          widthM: path.flow.widthM,
          speed: path.flow.speed,
          colour: path.flow.colour ?? path.colour,
          // Just above the water it runs inside, so the two do not fight for depth.
          liftM: 0.7,
        }),
      )
    }
    return mesh
  }
}

/** Colour of the scale grid: a neutral, quieter than any data. */
export const GRID_COLOUR = '#6e6e73'

/**
 * The scale grid over the map.
 *
 * Built from the terrain's own heights, so it is a ruler laid *on* the model rather than
 * a plane through it. Opt-in: at a whole-map view a fine grid is noise, and at a
 * zoomed-in view it is the only thing that says how big the cells are.
 */
export class SurfaceGrid {
  private readonly paths: PathSet
  private visible = false

  constructor(mapScene: MapScene) {
    this.paths = new PathSet(mapScene)
  }

  /**
   * Replaces the grid's lines, keeping its visibility.
   *
   * The width follows the step: a ruler whose lines are a fixed tenth of a metre is
   * invisible at any whole-map view, and one whose lines are metres wide is a wall when
   * the reader zooms in. A twentieth of the spacing reads as a line at both.
   */
  set(lines: WorldPoint[][], spacingM: number): void {
    const widthM = Math.min(4, Math.max(0.5, spacingM / 20))
    this.paths.set(
      lines.map((points) => ({
        points,
        colour: GRID_COLOUR,
        widthM,
        alpha: 0.5,
        zOffset: -1,
      })),
    )
    this.setVisible(this.visible)
  }

  /** Removes every line. */
  clear(): void {
    this.paths.clear()
  }

  /** Shows or hides the grid without discarding its lines. */
  setVisible(visible: boolean): void {
    this.visible = visible
    for (const mesh of this.paths.meshes) {
      mesh.setEnabled(visible)
    }
  }

  /** Disposes the grid's meshes. */
  dispose(): void {
    this.paths.dispose()
  }

  /** True while the grid is drawn. */
  get isVisible(): boolean {
    return this.visible
  }
}
