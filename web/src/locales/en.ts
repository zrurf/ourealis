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

  tasks: {
    tray: 'Background tasks ({count})',
    empty: 'Nothing is running.',
    clearFinished: 'Clear finished',
    elapsed: 'Running for {seconds} s',
    slowHint:
      'This can take a while on a large map or at a fine resolution; the service is still working.',
    done: '{what} finished.',
    failed: '{what} failed.',
    cancelled: 'The task was cancelled.',
    kind: {
      simulation: 'Run',
      route_preview: 'Route preview',
      route_plan: 'Route plan',
      synthetic_map: 'Map generation',
    },
    state: {
      queued: 'Queued',
      running: 'Running',
      succeeded: 'Done',
      failed: 'Failed',
      cancelled: 'Cancelled',
    },
    labels: {
      preview: 'Planning the candidate routes',
      plan: 'Planning the route and its speed limits',
      synthetic: 'Generating the map',
      run: 'Running the simulation',
    },
  },
  run: {
    stage: {
      map: 'Map',
      route: 'Route',
      runner: 'Runner',
      sensors: 'Sensors',
      run: 'Run',
    },
    stages: 'Run stages',
    progress: 'Progress towards a runnable draft',
    blocked: {
      noMap: 'Choose a map first',
      noStart: 'Place a start',
      noGoal: 'Place a goal',
    },
    mode: { simple: 'Simple', expert: 'Expert' },
    changed: '{count} changed',
    route: {
      hint: 'Click the map to place the start and the goal, then drag a handle to move it. Double-click a handle to remove it.',
      armed: 'Click the map to place the armed point.',
      refused: '{count} point(s) cannot be used: {reason}.',
      reason: {
        forbidden: 'the map marks it as blocked',
        outside: 'it lies outside the map',
        too_close: 'it is {distance} m from an obstacle, closer than a runner can pass',
        unknown: 'the map does not say',
      },
    },
    plan: {
      title: 'Plan',
      planning: 'Planning…',
      took: 'planned in {ms} ms',
      length: 'Length',
      duration: 'Estimate',
      pathRatio: 'Path ratio',
      candidates: 'Candidates',
      empty: 'A complete route is planned as soon as it is drawn.',
      chosen: 'chosen',
      inspectHint:
        'The planner picks by the route and the seed. Selecting a row changes what is drawn, not what will run.',
    },
    recipe: {
      campusJog: 'Campus jog',
      campusJogHint: 'A moderate run with every setting at its default.',
      track: 'Track intervals',
      trackHint: 'Four laps of a circuit at racing pace, starting fast.',
      phone: 'Phone and watch',
      phoneHint: 'GNSS at 1 Hz and an inertial unit at 100 Hz, with repeatable events.',
      truth: 'Clean truth',
      truthHint: 'No sensor noise and no metrics: the trajectory on its own.',
    },
    ready: 'Ready',
    canvasLabel:
      'Map: click to place a route point, Tab to cycle the points, arrow keys to move the selected one',
    menu: {
      setStart: 'Set the start here{replace}',
      setGoal: 'Set the goal here{replace}',
      setReference: 'Set the reference point here',
      addWaypoint: 'Add a waypoint here',
      replace: ' (replacing the current one)',
      remove: 'Remove the {what}',
      clear: 'Clear the {what}',
    },
    handle: {
      selected: '{role} selected at {x} m east, {y} m north',
      role: {
        start: 'Start',
        goal: 'Goal',
        reference: 'Reference point',
        waypoint: 'Waypoint',
        checkpoint: 'Checkpoint',
      },
    },
    submit: 'Start the run',
  },
  app: {
    title: 'Ourealis',
  },
  nav: {
    collapse: 'Collapse the sidebar',
    expand: 'Expand the sidebar',

    dashboard: 'Dashboard',
    maps: 'Maps',
    run: 'New run',
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
    yes: 'Yes',
    no: 'No',
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
    run: { title: 'Run workspace' },
    mapViewer: { title: 'Map preview' },
    mapStudio: { title: 'Map studio' },
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
