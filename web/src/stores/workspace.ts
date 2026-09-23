/*
 * The run workspace: one draft, one stage, one plan.
 *
 * The studio's problem was never the controls themselves but where the state lived:
 * the route lived on one page, the run on another, and moving between them threw the
 * plan away. So there is one draft here, and every stage of the workspace edits it —
 * a route built with the pointer is the same object the runner submits.
 *
 * Planning is automatic: as soon as the draft has a route, a preview is requested,
 * debounced and cancellable, and its candidate set is kept until the route changes.
 * The alternative — a button per planning stage — made the user choose between two
 * internals ("preview" and "plan") rather than between two outcomes.
 */
import { defineStore } from 'pinia'
import { computed, ref } from 'vue'
import { useMapsStore } from '@/stores/maps'
import { isApiError } from '@/api/errors'
import { planRoute, previewRoutes } from '@/api/routes'
import { checkFeasibility } from '@/api/maps'
import type { RoutePreview } from '@/types/result'
import {
  buildSimulationRequest,
  defaultFormState,
  pointToVec,
  validateForm,
  type PickTarget,
  type SimulationFormState,
} from '@/components/forms/request'
import { RECIPES, recipeById, type RecipeId } from '@/components/forms/recipes'

/** Stages of the workspace, in the order the rail shows them. */
export type Stage = 'map' | 'route' | 'runner' | 'sensors' | 'run'

/** Field of the draft that a control edits, for the change badges. */
type DraftGroup = 'route' | 'runner' | 'sensors' | 'run'

/** How much of the interface is shown. */
export type Mode = 'simple' | 'expert'

/** How the plan of the current draft is doing. */
export interface PlanState {
  /** Whether a request is in flight. */
  busy: boolean
  /** Milliseconds the service reported for the answer on screen. */
  durationMs: number | null
  /** The candidate set of the current route, or `null` while there is none. */
  preview: RoutePreview | null
  /** Index the service chose by its own Logit draw; the run takes this one. */
  chosen: number
  /**
   * Index the user is looking at.
   *
   * Separate from `chosen` on purpose: the service decides which candidate a run takes
   * from the route and the seed, so a reader who wants a different path changes those,
   * not a row in a table. Selecting a row only changes what is drawn.
   */
  inspected: number
  /** Failure of the last attempt, already translated. */
  error: string | null
  /** Whether the last answer included the smoothed path and its speed limits. */
  withProfile: boolean
}

/** One point's verdict, as the map's constraint query reported it. */
export interface PointVerdict {
  /** Whether the planner would accept the point. */
  legal: boolean
  /** Why, when it would not. */
  reason: 'ok' | 'outside' | 'forbidden' | 'too_close' | 'unknown'
  /** Distance to the nearest blocked cell, metres. */
  distanceM: number | null
}

/** Milliseconds a route change is coalesced before it is planned. */
const PLAN_DEBOUNCE_MS = 250

/** Local storage key of the interface mode. */
const MODE_KEY = 'ourealis.mode'

/** Which fields each group of the draft covers, for the change badges. */
const GROUP_FIELDS: Readonly<Record<DraftGroup, readonly string[]>> = {
  route: ['mode', 'start', 'goal', 'waypoints', 'reference', 'laps', 'checkpoints'],
  runner: ['preset', 'overrides', 'seed', 'individual'],
  sensors: ['sensors'],
  run: ['backend', 'motionMode', 'withMetrics', 'smooth'],
}

/** What the summary strip shows about the planned route. */
export interface RouteSummary {
  /** Planned length, metres. */
  lengthM: number
  /** Straight-line distance between the ends, metres. */
  straightLineM: number
  /** Path ratio: length over straight line. */
  pathRatio: number
  /** Estimated duration, seconds; `null` until the profile is known. */
  durationS: number | null
  /** How many candidates the planner offered. */
  candidates: number
}

/** The workspace. */
export const useWorkspaceStore = defineStore('workspace', () => {
  const maps = useMapsStore()

  const draft = ref<SimulationFormState>(defaultFormState())
  const stage = ref<Stage>('route')
  const mode = ref<Mode>(readMode())
  const recipe = ref<RecipeId | null>(null)
  const plan = ref<PlanState>({
    busy: false,
    durationMs: null,
    preview: null,
    chosen: 0,
    inspected: 0,
    error: null,
    withProfile: false,
  })
  /** The field the pointer is currently placing, when one is armed. */
  const armed = ref<PickTarget | null>(null)
  /**
   * Verdicts of the route's points, keyed by `role:index`.
   *
   * Filled by the map's own constraint query rather than by the planner, so a point
   * dropped inside a building says so at once. The planner remains the authority on
   * whether a *route* can be run — this is the answer for one point at a time.
   */
  const pointChecks = ref<Record<string, PointVerdict>>({})
  /**
   * Whether a handle is being dragged.
   *
   * While a drag is in progress the *whole* route is not re-checked: the point under
   * the pointer is, one at a time, and re-asking about the other points eight times a
   * second would be traffic for an answer that has not changed.
   */
  const dragging = ref(false)

  let planTimer: ReturnType<typeof setTimeout> | null = null
  let planGeneration = 0

  /** Validation issues of the draft as it stands. */
  const issues = computed(() => validateForm(draft.value))

  /** Whether the route is complete enough to plan and to run. */
  const runnable = computed(() => issues.value.length === 0)

  /** The map the draft runs on, resolved the way the service resolves it. */
  const mapId = computed(() => draft.value.mapId ?? maps.summaries[0]?.id ?? null)

  /** Every point of the route, keyed by the field it belongs to. */
  const routePoints = computed<Array<{ key: string; x: number; y: number }>>(() => {
    const current = draft.value
    const points: Array<{ key: string; x: number; y: number }> = []
    const push = (key: string, point: { x: number; y: number } | null): void => {
      if (point !== null) {
        points.push({ key, x: point.x, y: point.y })
      }
    }
    push('start:0', pointToVec(current.start))
    push(
      current.mode === 'loop' ? 'reference:0' : 'goal:0',
      pointToVec(current.mode === 'loop' ? current.reference : current.goal),
    )
    current.waypoints.forEach((waypoint, index) => {
      push(`waypoint:${index}`, pointToVec(waypoint.position))
    })
    return points
  })

  /** Points the map refuses, in the order the route lists them. */
  const illegalPoints = computed(() =>
    routePoints.value.filter((point) => pointChecks.value[point.key]?.legal === false),
  )

  /** A hash of everything the plan depends on, so a redundant replan is skipped. */
  const planKey = computed(() =>
    JSON.stringify([
      mapId.value,
      draft.value.mode,
      draft.value.start,
      draft.value.goal,
      draft.value.waypoints,
      draft.value.reference,
      draft.value.laps,
      draft.value.checkpoints,
      draft.value.preset,
      draft.value.overrides,
      draft.value.smooth,
    ]),
  )
  let plannedKey = ''

  /** The candidate the planner would take, or `null` while there is none. */
  const chosenCandidate = computed(() => {
    const preview = plan.value.preview
    if (preview === null) {
      return null
    }
    return preview.candidates[plan.value.chosen] ?? preview.candidates[0] ?? null
  })

  /** The candidate the reader is looking at. */
  const inspectedCandidate = computed(() => {
    const preview = plan.value.preview
    if (preview === null) {
      return null
    }
    return preview.candidates[plan.value.inspected] ?? chosenCandidate.value
  })

  /** What the summary strip shows. */
  const summary = computed<RouteSummary | null>(() => {
    const preview = plan.value.preview
    if (preview === null) {
      return null
    }
    return {
      lengthM: preview.length_m,
      straightLineM: preview.straight_line_m,
      pathRatio: preview.straight_line_m > 0 ? preview.length_m / preview.straight_line_m : 1,
      durationS: durationOf(preview),
      candidates: preview.candidates.length,
    }
  })

  /** How many fields of a group differ from the recipe the draft came from. */
  function changedIn(group: DraftGroup): number {
    const template = recipe.value === null ? defaultFormState() : recipeById(recipe.value).state
    return GROUP_FIELDS[group].filter(
      (field) =>
        JSON.stringify(draft.value[field as keyof SimulationFormState]) !==
        JSON.stringify(template[field as keyof SimulationFormState]),
    ).length
  }

  /** Replaces the draft, e.g. from a form component's two-way binding. */
  function setDraft(next: SimulationFormState): void {
    draft.value = next
    schedulePlan()
  }

  /** Applies a patch to the draft. */
  function patch(changes: Partial<SimulationFormState>): void {
    setDraft({ ...draft.value, ...changes })
  }

  /** Sets one coordinate of a route point. */
  function setPoint(target: PickTarget, x: number, y: number): void {
    const rounded = { x: round(x), y: round(y) }
    switch (target.kind) {
      case 'start':
        patch({ start: rounded })
        break
      case 'goal':
        patch({ goal: rounded })
        break
      case 'reference':
        patch({ reference: rounded })
        break
      case 'waypoint':
        patch({
          waypoints: draft.value.waypoints.map((waypoint, index) =>
            index === target.index ? { ...waypoint, position: rounded } : waypoint,
          ),
        })
        break
      case 'checkpoint':
        patch({
          checkpoints: draft.value.checkpoints.map((checkpoint, index) =>
            index === target.index ? { ...checkpoint, position: rounded } : checkpoint,
          ),
        })
        break
    }
  }

  /** Inserts a waypoint at the end of the ordered list, returning its index. */
  function addWaypoint(x: number, y: number): number {
    const index = draft.value.waypoints.length
    patch({
      waypoints: [
        ...draft.value.waypoints,
        {
          position: { x: round(x), y: round(y) },
          semantics: 'pass',
          duration_s: 0,
          radius_m: 5,
        },
      ],
    })
    return index
  }

  /** Removes a route point. */
  function removePoint(target: PickTarget): void {
    switch (target.kind) {
      case 'start':
        patch({ start: { x: null, y: null } })
        break
      case 'goal':
        patch({ goal: { x: null, y: null } })
        break
      case 'reference':
        patch({ reference: { x: null, y: null } })
        break
      case 'waypoint':
        patch({
          waypoints: draft.value.waypoints.filter((_, index) => index !== target.index),
        })
        break
      case 'checkpoint':
        patch({
          checkpoints: draft.value.checkpoints.filter((_, index) => index !== target.index),
        })
        break
    }
  }

  /**
   * Copies the draft's route into the fields a pickup leaves alone.
   *
   * Placing the first point of a route is the common case, so a goal placed before a
   * start (or the other way round) must not overwrite what is already there. Used by
   * the canvas's "click empty ground" behaviour.
   */
  function placeNext(x: number, y: number): PickTarget | null {
    if (pointToVec(draft.value.start) === null) {
      setPoint({ kind: 'start' }, x, y)
      return { kind: 'start' }
    }
    if (draft.value.mode !== 'loop' && pointToVec(draft.value.goal) === null) {
      setPoint({ kind: 'goal' }, x, y)
      return { kind: 'goal' }
    }
    if (draft.value.mode === 'loop' && pointToVec(draft.value.reference) === null) {
      setPoint({ kind: 'reference' }, x, y)
      return { kind: 'reference' }
    }
    return null
  }

  /** Applies a recipe: a named starting point for a whole configuration. */
  function applyRecipe(id: RecipeId): void {
    const next = recipeById(id)
    recipe.value = id
    draft.value = next.state
    stage.value = 'route'
    schedulePlan()
  }

  /** Switches between the two levels of detail, remembering the choice. */
  function setMode(next: Mode): void {
    mode.value = next
    writeMode(next)
  }

  /** Resets the draft and the plan. */
  function reset(): void {
    cancelPlan()
    recipe.value = null
    draft.value = defaultFormState()
    plan.value = {
      busy: false,
      durationMs: null,
      preview: null,
      chosen: 0,
      inspected: 0,
      error: null,
      withProfile: false,
    }
    plannedKey = ''
  }

  /** Cancels any planning in flight and clears the debounce. */
  function cancelPlan(): void {
    if (planTimer !== null) {
      clearTimeout(planTimer)
      planTimer = null
    }
    // Bumping the generation makes any answer still in flight a no-op.
    planGeneration += 1
    plan.value = { ...plan.value, busy: false }
  }

  /** Schedules a plan of the current draft, coalescing a burst of edits. */
  function schedulePlan(): void {
    if (planTimer !== null) {
      clearTimeout(planTimer)
    }
    planTimer = setTimeout(() => {
      planTimer = null
      if (!dragging.value) {
        void checkRoutePoints()
      }
      void planNow()
    }, PLAN_DEBOUNCE_MS)
  }

  /**
   * Asks the map whether it allows each point of the route.
   *
   * One request for the whole route: the endpoint answers a list, and a route is a
   * handful of points, so there is nothing to be gained by asking per point.
   */
  async function checkRoutePoints(): Promise<void> {
    const id = mapId.value
    const points = routePoints.value
    if (id === null || points.length === 0) {
      pointChecks.value = {}
      return
    }
    const generation = planGeneration
    try {
      const answers = await checkFeasibility(
        id,
        points.map((point) => ({ x: point.x, y: point.y })),
      )
      if (generation !== planGeneration) {
        return
      }
      const verdicts: Record<string, PointVerdict> = {}
      answers.forEach((answer, index) => {
        const point = points[index]
        if (point === undefined) {
          return
        }
        verdicts[point.key] = {
          legal: answer.legal,
          reason: answer.reason,
          distanceM: answer.distance_m,
        }
      })
      pointChecks.value = verdicts
    } catch {
      // The query is an early warning, not a gate: a map the service could not read
      // here will fail the plan with a message of its own.
      pointChecks.value = {}
    }
  }

  /**
   * Checks one point, for the feedback a drag needs while it is happening.
   *
   * The same query as the route-wide check, for a single point: a reader dragging a
   * handle has to see that the place is refused before letting go, not 250 ms after.
   */
  async function checkPoint(key: string, x: number, y: number): Promise<void> {
    const id = mapId.value
    if (id === null) {
      return
    }
    try {
      const [answer] = await checkFeasibility(id, [{ x, y }])
      if (answer === undefined) {
        return
      }
      pointChecks.value = {
        ...pointChecks.value,
        [key]: {
          legal: answer.legal,
          reason: answer.reason,
          distanceM: answer.distance_m,
        },
      }
    } catch {
      // As for the route-wide check: an early warning that fails is not a gate.
    }
  }

  /** Records whether a handle is being dragged. */
  function setDragging(value: boolean): void {
    dragging.value = value
  }

  /**
   * Plans the draft, replacing the previous answer.
   *
   * A draft with no complete route clears the answer instead of failing: an empty form
   * is not an error, and the previous route's candidates would be a lie about what is
   * on screen.
   */
  async function planNow(): Promise<void> {
    const key = planKey.value
    const routeIssue = validateForm(draft.value).some((issue) => issue.field !== 'name')
    if (routeIssue || mapId.value === null) {
      plan.value = {
        busy: false,
        durationMs: null,
        preview: null,
        chosen: 0,
        inspected: 0,
        error: null,
        withProfile: false,
      }
      plannedKey = ''
      return
    }
    if (key === plannedKey && plan.value.preview !== null) {
      return
    }
    const generation = ++planGeneration
    plan.value = { ...plan.value, busy: true, error: null }
    try {
      const request = buildSimulationRequest(draft.value)
      const preview = await previewRoutes(request)
      if (generation !== planGeneration) {
        // A newer draft has been planned since; this answer describes a route that is
        // no longer on screen.
        return
      }
      plannedKey = key
      plan.value = {
        busy: false,
        durationMs: preview.planning_ms,
        preview,
        chosen: preview.chosen,
        inspected: preview.chosen,
        error: null,
        withProfile: false,
      }
      void loadProfile(generation)
    } catch (error) {
      if (generation !== planGeneration) {
        return
      }
      plan.value = {
        busy: false,
        durationMs: null,
        preview: null,
        chosen: 0,
        inspected: 0,
        error: isApiError(error) ? error.message : String(error),
        withProfile: false,
      }
      plannedKey = ''
    }
  }

  /**
   * Fetches the smoothed path and its speed limits for the chosen candidate.
   *
   * A second task rather than part of the first: the profile costs much more than the
   * candidate search, and the map is usable — the route is drawn — while it runs.
   */
  async function loadProfile(generation: number): Promise<void> {
    try {
      const request = buildSimulationRequest(draft.value)
      const planned = await planRoute(request)
      if (generation !== planGeneration) {
        return
      }
      const preview = plan.value.preview
      plan.value = {
        ...plan.value,
        withProfile: true,
        preview: preview === null ? planned : { ...preview, ...profileOf(planned) },
      }
    } catch {
      // The candidate set is already on screen; a profile that does not arrive leaves
      // the estimate blank rather than replacing the plan with an error.
    }
  }

  /** Changes which candidate is drawn; the planner's own choice is untouched. */
  function inspectCandidate(index: number): void {
    plan.value = { ...plan.value, inspected: index }
  }

  /** Arms the pointer to place one field, or disarms it when given the same one. */
  function arm(target: PickTarget | null): void {
    armed.value =
      target !== null && armed.value !== null && sameTarget(armed.value, target) ? null : target
  }

  /** A click on the surface, in the mode the workspace is in. */
  function handleGroundClick(x: number, y: number): PickTarget | null {
    const target = armed.value
    if (target !== null) {
      setPoint(target, x, y)
      armed.value = null
      return target
    }
    return placeNext(x, y)
  }

  return {
    draft,
    stage,
    mode,
    recipe,
    plan,
    armed,
    issues,
    runnable,
    mapId,
    planKey,
    chosenCandidate,
    inspectedCandidate,
    pointChecks,
    routePoints,
    illegalPoints,
    dragging,
    checkRoutePoints,
    checkPoint,
    setDragging,
    summary,
    recipes: RECIPES,
    changedIn,
    setDraft,
    patch,
    setPoint,
    addWaypoint,
    removePoint,
    placeNext,
    applyRecipe,
    setMode,
    reset,
    cancelPlan,
    schedulePlan,
    planNow,
    inspectCandidate,
    arm,
    handleGroundClick,
  }
})

/** Whether two pick targets name the same field. */
function sameTarget(a: PickTarget, b: PickTarget): boolean {
  if (a.kind !== b.kind) {
    return false
  }
  return (
    (a.kind !== 'waypoint' && a.kind !== 'checkpoint') ||
    (b.kind !== 'waypoint' && b.kind !== 'checkpoint') ||
    a.index === b.index
  )
}

/** Rounds a coordinate to the centimetre the plan is drawn at. */
function round(value: number): number {
  return Math.round(value * 100) / 100
}

/** The profile fields of a planned answer, which is all the second task adds. */
function profileOf(planned: RoutePreview): Partial<RoutePreview> {
  return {
    path: planned.path,
    speed_limit_mps: planned.speed_limit_mps,
    speed_limit_s: planned.speed_limit_s,
    planning_ms: planned.planning_ms,
  }
}

/** Duration estimate from the speed limits, when the profile is known. */
function durationOf(preview: RoutePreview): number | null {
  const arcs = preview.speed_limit_s
  const speeds = preview.speed_limit_mps
  if (arcs.length < 2 || arcs.length !== speeds.length) {
    return null
  }
  let seconds = 0
  for (let index = 1; index < arcs.length; index += 1) {
    const span = (arcs[index] ?? 0) - (arcs[index - 1] ?? 0)
    const speed = Math.max(0.5, ((speeds[index] ?? 0) + (speeds[index - 1] ?? 0)) / 2)
    seconds += span / speed
  }
  return seconds
}

/** Reads the remembered interface mode, falling back to the simple one. */
function readMode(): Mode {
  try {
    const stored = globalThis.localStorage?.getItem(MODE_KEY)
    return stored === 'expert' ? 'expert' : 'simple'
  } catch {
    return 'simple'
  }
}

/** Remembers the interface mode. */
function writeMode(mode: Mode): void {
  try {
    globalThis.localStorage?.setItem(MODE_KEY, mode)
  } catch {
    // A blocked storage is not a reason to fail a mode switch.
  }
}
