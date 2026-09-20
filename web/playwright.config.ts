import { defineConfig, devices } from '@playwright/test'

/**
 * One runner, four lanes. Each lane is selected by `--project` and its own
 * test directory, so a lane never compiles the specs of another:
 *
 *   pnpm test:unit    tests/unit    pure logic, no browser, no server
 *   pnpm test:e2e     tests/e2e    a browser against a real server (see below)
 *   pnpm test:fuzz    tests/fuzz    seeded random input against pure functions
 *   pnpm test:monkey  tests/monkey  seeded random operation sequences over the stores
 *
 * The browser lanes talk to `OUREALIS_SERVICE_URL` when it is set — pointing it at
 * the service exercises the real facades and the embedded page, which is the
 * acceptance path — and otherwise to the Vite dev server on
 * http://127.0.0.1:5173, whose `/api` proxy expects a service on port 8080. Either
 * way a page server is started only for the lanes that need one (`e2e`, `monkey`);
 * `unit` and `fuzz` are pure logic and start nothing.
 */
const serviceUrl = process.env.OUREALIS_SERVICE_URL
const baseURL = serviceUrl ?? 'http://127.0.0.1:5173'

/** Names passed through `--project=<name>` or `--project <name>` on the command line. */
function requestedProjects(argv: readonly string[]): string[] {
  const names: string[] = []
  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index]
    if (arg === undefined) {
      continue
    }
    if (arg === '--project' || arg === '-p') {
      const next = argv[index + 1]
      if (next !== undefined) {
        names.push(next)
      }
      continue
    }
    if (arg.startsWith('--project=')) {
      names.push(arg.slice('--project='.length))
    }
  }
  return names
}

export default defineConfig({
  testDir: './tests',
  outputDir: './test-results',
  fullyParallel: false,
  workers: 1,
  forbidOnly: Boolean(process.env.CI),
  retries: 0,
  timeout: 60_000,
  expect: { timeout: 15_000 },
  reporter: [['list']],
  use: {
    baseURL,
    trace: 'retain-on-failure',
    screenshot: 'only-on-failure',
    video: 'off',
  },
  projects: [
    { name: 'unit', testDir: './tests/unit' },
    {
      name: 'e2e',
      testDir: './tests/e2e',
      // The appearance the smoke tests start in is pinned here so the assertion is platform-independent.
      use: { ...devices['Desktop Chrome'], colorScheme: 'light' },
    },
    { name: 'fuzz', testDir: './tests/fuzz' },
    { name: 'monkey', testDir: './tests/monkey' },
  ],
  webServer:
    serviceUrl === undefined &&
    (requestedProjects(process.argv).includes('e2e') ||
      // The monkey lane drives the real interface, so it needs a page served too;
      // the unit and fuzz lanes are pure logic and start nothing.
      requestedProjects(process.argv).includes('monkey') ||
      requestedProjects(process.argv).length === 0)
      ? {
          // Port 5173, not 8080: the proxy in `vite.config.ts` forwards `/api` to
          // `127.0.0.1:8080`, where the service runs. A dev server on 8080 would both
          // collide with the service and make the app's own requests loop back to
          // itself, so the fallback could never reach an API.
          command: 'pnpm exec vite --host 127.0.0.1 --port 5173 --strictPort',
          url: 'http://127.0.0.1:5173',
          reuseExistingServer: !process.env.CI,
          timeout: 120_000,
          stdout: 'ignore',
        }
      : undefined,
})
