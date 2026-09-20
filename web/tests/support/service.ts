/*
 * The service a browser lane runs against, decided in one place.
 *
 * The e2e and fuzz lanes exist to exercise a real service, so a lane that skips
 * every test on a machine without one reports success while asserting nothing. The
 * gate below owns both the decision and its message: by default an absent service
 * fails the lane with the command that starts it, and `OUREALIS_ALLOW_SKIP=1`
 * restores the skip-with-reason behaviour for a machine that has none.
 *
 * `tests/unit` and `tests/monkey` never touch the API — the unit lane is pure
 * logic and the monkey walk passes against a front-end server with no service
 * behind it — so they do not gate on this.
 */
import { expect, test, type APIRequestContext } from '@playwright/test'

/** Base URL the lanes talk to, from `OUREALIS_SERVICE_URL` and the service's default port. */
export const SERVICE_URL = process.env.OUREALIS_SERVICE_URL ?? 'http://127.0.0.1:8080'

/** Health endpoint that decides whether the service is there. */
const HEALTH_URL = `${SERVICE_URL}/api/v1/health`

/** Message a lane shows when nothing answered, including how to start the service. */
export const SERVICE_REASON = `no service answered at ${HEALTH_URL}: start one from the repository root with
  cargo run -p ourealis -- --config web/tests/fixtures/service.toml
or run the built binary with ./target/debug/ourealis --config web/tests/fixtures/service.toml, then point the lane at it with OUREALIS_SERVICE_URL=${SERVICE_URL}. Set OUREALIS_ALLOW_SKIP=1 to skip the lane instead of failing it.`

/** One spec file's view of the service. */
export interface ServiceGate {
  /** True once the probe found a healthy service. */
  readonly available: boolean
  /** Probes the health endpoint; call once from `beforeAll`. */
  probe(request: APIRequestContext): Promise<void>
  /** Applies the skip at the top of a test; a no-op when the service answered. */
  skipUnlessAvailable(): void
}

/** Whether a lane without a service skips instead of failing. */
function skipAllowed(): boolean {
  return process.env.OUREALIS_ALLOW_SKIP === '1'
}

/**
 * Builds the gate for one spec file.
 *
 * ```ts
 * const service = serviceGate()
 * test.beforeAll(async ({ request }) => service.probe(request))
 * test('…', async ({ page }) => {
 *   service.skipUnlessAvailable()
 *   …
 * })
 * ```
 */
export function serviceGate(): ServiceGate {
  let available = false
  return {
    get available(): boolean {
      return available
    },
    async probe(request: APIRequestContext): Promise<void> {
      available = await healthy(request)
      if (available) {
        return
      }
      // Printed as well as asserted: the reporter shows the assertion, but a log
      // line is what a reader of the terminal sees next to the lane's name.
      console.log(`[service] ${SERVICE_REASON}`)
      if (!skipAllowed()) {
        expect(available, SERVICE_REASON).toBe(true)
      }
    },
    skipUnlessAvailable(): void {
      test.skip(!available, SERVICE_REASON)
    },
  }
}

/** True when `/api/v1/health` answered with the service's own `status: "ok"`. */
async function healthy(request: APIRequestContext): Promise<boolean> {
  const body = await request
    .get(HEALTH_URL, { failOnStatusCode: false, timeout: 5_000 })
    .then((response) => (response.ok() ? response.json() : null))
    .catch(() => null)
  return typeof body === 'object' && body !== null && (body as { status?: unknown }).status === 'ok'
}
