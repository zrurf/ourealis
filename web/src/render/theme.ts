/*
 * Ties the scene's appearance to the interface's own light/dark switch.
 *
 * The engine lives for the whole session while views come and go, so the binding is made
 * once by the render host rather than by a view: a watcher created inside a component would
 * die with the component and leave the map on the appearance it was created with.
 *
 * The watcher flushes after the DOM update, because the scene reads its colours from the
 * CSS custom properties the switch rewrites — it has to see the new tokens, not the old.
 */
import { watch } from 'vue'
import { useThemeStore } from '@/stores/theme'
import type { MapScene } from './scene'

/** Makes a scene follow the theme store; returns the handle that stops it. */
export function bindSceneAppearance(scene: MapScene): () => void {
  const theme = useThemeStore()
  return watch(
    () => theme.isDark,
    (dark) => {
      scene.setAppearance(dark)
    },
    { immediate: true, flush: 'post' },
  )
}
