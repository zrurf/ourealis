/**
 * English message catalog.
 *
 * This file is the authoritative key set for the interface: the Chinese catalog
 * is annotated with `typeof import('./en').default`, so a key added here fails
 * type checking until `zh-CN.ts` catches up, and a key removed here fails in the
 * other direction. Nothing in the catalog may be empty.
 */
import map from './parts/en/map'
import omf from './parts/en/omf'
import simulation from './parts/en/simulation'
const en = {
  // Area namespaces, spread in so each part file can be owned separately.
  ...map,
  ...simulation,
  ...omf,

  app: {
    title: 'Ourealis',
  },
  nav: {
    dashboard: 'Dashboard',
    maps: 'Maps',
    routes: 'Routes',
    batch: 'Batch',
    omf: 'OMF inspector',
    settings: 'Settings',
  },
  theme: {
    label: 'Theme',
    light: 'Light',
    dark: 'Dark',
  },
  language: {
    label: 'Language',
    en: 'English',
    'zh-CN': '简体中文',
  },
  common: {
    empty: 'Nothing to show yet.',
    loading: 'Loading…',
    error: 'Something went wrong.',
    retry: 'Retry',
    cancel: 'Cancel',
    confirm: 'Confirm',
    save: 'Save',
    delete: 'Delete',
    close: 'Close',
    refresh: 'Refresh',
    search: 'Search',
    copy: 'Copy',
    back: 'Back',
  },
  error: {
    fromService: 'from service',
  },
  views: {
    dashboard: { title: 'Overview' },
    mapList: { title: 'Maps' },
    mapViewer: { title: 'Map preview' },
    mapStudio: { title: 'Map studio' },
    routeStudio: { title: 'Route studio' },
    simulation: { title: 'Simulation' },
    trajectory: { title: 'Trajectory' },
    sensor: { title: 'Sensors' },
    audit: { title: 'Audit' },
    batch: { title: 'Batch runs' },
    omf: { title: 'OMF inspector' },
    settings: { title: 'Settings' },
    notFound: { title: 'Page not found' },
  },
}

export default en
