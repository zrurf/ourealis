/*
 * Recipes: whole configurations that are worth starting from.
 *
 * The workspace has around fifty controls, and the honest reason they are all there is
 * that the simulator has that many knobs. Almost nobody wants to set them one at a
 * time: a reader comes with a situation — a jog round campus, an interval session, a
 * phone-and-watch data set — and a recipe is that situation written down. Choosing one
 * fills the draft; everything stays editable afterwards, and the panels report how many
 * fields differ from the recipe that was applied.
 *
 * A recipe is data, not code: the table below is the whole definition, so adding one is
 * adding a row.
 */
import { defaultFormState, type SimulationFormState } from './request'

/** Identifier of a recipe. */
export type RecipeId = 'campus-jog' | 'track-intervals' | 'phone-field' | 'clean-truth'

/** One recipe. */
export interface Recipe {
  /** Identifier used in storage and in the test ids. */
  id: RecipeId
  /** Catalog key of its name. */
  nameKey: string
  /** Catalog key of the one-line description. */
  descriptionKey: string
  /** The draft it produces. */
  state: SimulationFormState
}

/** A draft with the person preset and metrics switch a recipe wants. */
function base(preset: string, withMetrics = true): SimulationFormState {
  return { ...defaultFormState(), preset, withMetrics }
}

/** The four recipes the workspace offers. */
export const RECIPES: readonly Recipe[] = [
  {
    id: 'campus-jog',
    nameKey: 'run.recipe.campusJog',
    descriptionKey: 'run.recipe.campusJogHint',
    // Everything at its default but the preset: the point of this recipe is that a
    // first run needs no decisions at all.
    state: base('moderate'),
  },
  {
    id: 'track-intervals',
    nameKey: 'run.recipe.track',
    descriptionKey: 'run.recipe.trackHint',
    state: {
      ...base('race'),
      mode: 'loop',
      laps: 4,
      smooth: true,
      overrides: { pace_strategy: 'positive_split' },
      seed: 20260214,
    },
  },
  {
    id: 'phone-field',
    nameKey: 'run.recipe.phone',
    descriptionKey: 'run.recipe.phoneHint',
    state: {
      ...base('moderate'),
      // The configuration the simulator's own regression runs use: a phone at 1 Hz and
      // a watch at 100 Hz, with events triggered by position rather than by a draw, so
      // two runs of the same seed produce the same sensor data.
      sensors: {
        ...defaultFormState().sensors,
        gnss_rate_hz: 1,
        imu_rate_hz: 100,
        mag_rate_hz: 50,
        baro_rate_hz: 10,
        multipath_enabled: true,
        magnetic_disturbance_enabled: true,
        jitter_enabled: true,
        mount: 'body',
        force_deterministic_events: true,
      },
    },
  },
  {
    id: 'clean-truth',
    nameKey: 'run.recipe.truth',
    descriptionKey: 'run.recipe.truthHint',
    state: {
      ...base('moderate', false),
      // No noise, no metrics: the run is a pure trajectory, which is what a consumer
      // that wants to integrate something of its own starts from.
      sensors: {
        ...defaultFormState().sensors,
        multipath_enabled: false,
        magnetic_disturbance_enabled: false,
        jitter_enabled: false,
      },
    },
  },
]

/** One recipe by id; throws for an id the table does not hold. */
export function recipeById(id: RecipeId): Recipe {
  const found = RECIPES.find((recipe) => recipe.id === id)
  if (found === undefined) {
    throw new Error(`no recipe named ${id}`)
  }
  return found
}
