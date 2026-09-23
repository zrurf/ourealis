/*
 * The direction-constraint field, drawn as arrows.
 *
 * The direction layer packs a heading and a strength into one number, and a coloured cell can
 * never say a heading: a reader has to translate a hue into an angle. Arrows are the notation a
 * plan uses for a vector field, so this draws one per sample — a shaft and two barbs on the
 * ground — coloured by the strength of the constraint there.
 *
 * The arrows are grouped into a few strength buckets and drawn as one ribbon mesh each: a field
 * is hundreds of arrows, and a draw call per arrow would cost more than the whole terrain.
 */
import { Color3 } from '@babylonjs/core/Maths/math.color'
import type { AbstractMesh } from '@babylonjs/core/Meshes/abstractMesh'
import { CreateGreasedLine } from '@babylonjs/core/Meshes/Builders/greasedLineBuilder'
import { GreasedLineMeshColorMode } from '@babylonjs/core/Materials/GreasedLine/greasedLineMaterialInterfaces'
import { directionArrows, type DirectionSample, type SurfaceHeight } from './direction'
import type { MapScene } from './scene'

/** Buckets the field is drawn in: one mesh, and one colour, per bucket. */
export const ARROW_BUCKETS = 4

/** Colour of a weak constraint and of a strong one; strength reads as saturation. */
const WEAK = new Color3(0.36, 0.56, 0.82)
const STRONG = new Color3(0.05, 0.2, 0.6)

/** Options of {@link DirectionArrows}. */
export interface DirectionArrowOptions {
  /** Arrow length in metres. */
  lengthM?: number
  /** Height above the ground, metres. */
  liftM?: number
}

/** The arrow field of one map, rebuilt whenever the view it describes changes. */
export class DirectionArrows {
  private readonly mapScene: MapScene
  private readonly lengthM: number
  private parts: AbstractMesh[] = []
  private visible = false

  constructor(mapScene: MapScene, options: DirectionArrowOptions = {}) {
    this.mapScene = mapScene
    this.lengthM = options.lengthM ?? 14
  }

  /** Replaces the field with the arrows of the given samples. */
  set(samples: readonly DirectionSample[], surface: SurfaceHeight): void {
    this.clear()
    const arrows = directionArrows(samples, surface, {
      lengthM: this.lengthM,
      liftM: 0.35,
      barbRatio: 0.4,
    })
    const buckets: Array<typeof arrows> = Array.from({ length: ARROW_BUCKETS }, () => [])
    for (const arrow of arrows) {
      const index = Math.min(ARROW_BUCKETS - 1, Math.floor(arrow.strength * ARROW_BUCKETS))
      buckets[index]?.push(arrow)
    }
    buckets.forEach((bucket, index) => {
      if (bucket.length === 0) {
        return
      }
      const strength = (index + 0.5) / ARROW_BUCKETS
      const mesh = CreateGreasedLine(
        `direction-arrows:${index}`,
        { points: bucket.flatMap((arrow) => arrow.paths.map((path) => path.flat())) },
        {
          width: Math.max(0.4, this.lengthM * 0.1),
          color: Color3.Lerp(WEAK, STRONG, strength),
          sizeAttenuation: false,
          colorMode: GreasedLineMeshColorMode.COLOR_MODE_SET,
        },
        this.mapScene.scene,
      )
      mesh.isPickable = false
      mesh.renderingGroupId = 1
      if (mesh.material !== null) {
        mesh.material.alpha = 0.4 + strength * 0.55
        mesh.material.zOffset = -3
      }
      this.mapScene.trackHeightMesh(mesh)
      this.parts.push(mesh)
    })
    this.setVisible(this.visible)
  }

  /** Shows or hides the field without discarding it. */
  setVisible(visible: boolean): void {
    this.visible = visible
    for (const mesh of this.parts) {
      mesh.setEnabled(visible)
    }
  }

  /** Number of arrows currently drawn, for a test or a status line. */
  get size(): number {
    return this.parts.length
  }

  /** Removes every arrow. */
  clear(): void {
    for (const mesh of this.parts) {
      this.mapScene.untrackHeightMesh(mesh)
      mesh.dispose()
    }
    this.parts = []
  }

  /** Disposes the field. */
  dispose(): void {
    this.clear()
  }
}
