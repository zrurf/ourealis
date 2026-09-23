/*
 * Inline SVG icons.
 *
 * The interface needs a dozen small, unambiguous glyphs — a camera, a sun, a grid, a cube —
 * and a set of inline paths is the whole dependency: no icon package, no font, no sprite, and
 * every glyph is a `currentColor` stroke that follows the theme. Each path is drawn on a 24×24
 * canvas with a 1.7-unit stroke, so the icons stay optically consistent at the 16–18 px the
 * viewport rails use.
 */

/** One icon: the path data of a 24×24 glyph, stroked with `currentColor`. */
export interface IconPath {
  /** `d` attributes, one per sub-path. */
  paths: readonly string[]
  /** Whether the glyph is filled rather than stroked. */
  filled?: boolean
  /** Circles to draw as well, as `[cx, cy, r]`. */
  circles?: ReadonlyArray<readonly [number, number, number]>
}

/** Names of the icons the interface draws. */
export type IconName =
  | 'camera'
  | 'sun'
  | 'surface'
  | 'grid'
  | 'layers'
  | 'cube'
  | 'target'
  | 'refresh'
  | 'chevron-left'
  | 'chevron-right'
  | 'panel-left'
  | 'dashboard'
  | 'maps'
  | 'run'
  | 'batch'
  | 'inspect'
  | 'settings'
  | 'flag'
  | 'pin'
  | 'route'
  | 'play'
  | 'menu'

/** The glyphs, on a 24×24 canvas. */
export const ICONS: Readonly<Record<IconName, IconPath>> = {
  camera: {
    paths: ['M3 8.5h4l1.6-2.5h6.8L17 8.5h4v9.5H3z'],
    circles: [[12, 13, 3.4]],
  },
  sun: {
    paths: [
      'M12 5.5v-3',
      'M12 21.5v-3',
      'M5.5 12h-3',
      'M21.5 12h-3',
      'M7.2 7.2 5 5',
      'M19 19l-2.2-2.2',
      'M16.8 7.2 19 5',
      'M5 19l2.2-2.2',
    ],
    circles: [[12, 12, 3.6]],
  },
  surface: {
    paths: ['M3 15.5 12 19l9-3.5', 'M3 10.5 12 14l9-3.5'],
    circles: [],
  },
  grid: {
    paths: ['M3 3h18v18H3z', 'M9 3v18', 'M15 3v18', 'M3 9h18', 'M3 15h18'],
  },
  layers: {
    paths: ['M12 3 3 7.5 12 12l9-4.5z', 'M3 12.5 12 17l9-4.5', 'M3 17 12 21.5 21 17'],
  },
  cube: {
    paths: ['M12 2.6 3.5 7v10L12 21.4 20.5 17V7z', 'M3.5 7 12 11.5 20.5 7', 'M12 11.5v9.9'],
  },
  target: {
    paths: [],
    circles: [
      [12, 12, 7.5],
      [12, 12, 2.6],
    ],
  },
  refresh: {
    paths: ['M20 12a8 8 0 1 1-2.6-5.9', 'M20 4v4.4h-4.4'],
  },
  'chevron-left': { paths: ['M14.5 5 7.5 12l7 7'] },
  'chevron-right': { paths: ['M9.5 5l7 7-7 7'] },
  'panel-left': { paths: ['M3 4h18v16H3z', 'M9 4v16'] },
  dashboard: { paths: ['M3 13h7V3H3z', 'M14 21h7V11h-7z', 'M3 21h7v-5H3z', 'M14 8h7V3h-7z'] },
  maps: { paths: ['M3 6.5 9 4l6 2.5L21 4v13.5L15 20l-6-2.5L3 20z', 'M9 4v13.5', 'M15 6.5V20'] },
  run: { paths: ['M13 3.5 6 13h4.5L9.5 20.5 17 11h-4.5z'] },
  batch: { paths: ['M4 4h12v12H4z', 'M8 8h12v12H8z'] },
  inspect: {
    paths: ['M11 4.5a6.5 6.5 0 1 1 0 13 6.5 6.5 0 0 1 0-13z', 'M15.6 15.6 20.5 20.5'],
  },
  settings: {
    paths: [
      'M12 8.4a3.6 3.6 0 1 1 0 7.2 3.6 3.6 0 0 1 0-7.2z',
      'M4 12h2.2',
      'M17.8 12H20',
      'M12 4v2.2',
      'M12 17.8V20',
    ],
    circles: [],
  },
  flag: { paths: ['M6 21V3.5', 'M6 4.5h11l-2 3.5 2 3.5H6z'] },
  pin: {
    paths: ['M12 21.5S5.5 14.8 5.5 10a6.5 6.5 0 0 1 13 0c0 4.8-6.5 11.5-6.5 11.5z'],
    circles: [[12, 10, 2.4]],
  },
  route: {
    paths: [
      'M6.5 18.5a2.5 2.5 0 1 1 0-5 2.5 2.5 0 0 1 0 5z',
      'M17.5 10.5a2.5 2.5 0 1 1 0-5 2.5 2.5 0 0 1 0 5z',
      'M9 16h4.5a4 4 0 0 0 0-8H13',
    ],
  },
  play: { paths: ['M7 4.5 19 12 7 19.5z'] },
  menu: { paths: ['M4 7h16', 'M4 12h16', 'M4 17h16'] },
}
