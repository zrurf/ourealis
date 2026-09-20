/*
 * Individual presets and their parameter schema.
 *
 * The form generator reads this instead of carrying a copy of `PersonParams`, so
 * a field core adds shows up in the UI as soon as the service reports it. The
 * parameter vector itself is deliberately left as `unknown`: rendering it is the
 * form's business, and typing it here would duplicate the Rust struct.
 */
import { api, ApiClient, pageQuery } from './client'
import type { Page } from './types'
import type { SimulationSettings } from '@/types/simulation'

/** One preset with its resolved parameters and the overrides it accepts. */
export interface Preset {
  /** Preset name: `jog`, `moderate` or `race`. */
  preset: string
  /** Resolved parameter vector of the preset. */
  params: unknown
  /** Names the `person.overrides` object accepts. */
  override_fields: string[]
  /** Default simulator settings of a run that does not name any. */
  defaults: SimulationSettings
}

/** The presets this build accepts. */
export function listPresets(client: ApiClient = api, signal?: AbortSignal): Promise<Page<Preset>> {
  return client.get<Page<Preset>>('presets', { query: pageQuery(0, 100), signal })
}
