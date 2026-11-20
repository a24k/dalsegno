import { defineConfig, devices } from '@playwright/test'

const BIND = '127.0.0.1:8738'

export default defineConfig({
  testDir: 'e2e',
  forbidOnly: Boolean(process.env.CI),
  retries: process.env.CI ? 1 : 0,
  reporter: process.env.CI ? 'github' : 'list',
  use: {
    baseURL: `http://${BIND}`,
    trace: 'retain-on-failure',
  },
  projects: [{ name: 'chromium', use: { ...devices['Desktop Chrome'] } }],
  webServer: {
    command: 'node e2e/serve.mjs',
    url: `http://${BIND}/api/v1/streams`,
    env: { DALSEGNO_BIND: BIND },
    reuseExistingServer: false,
    // Let the launcher stop the service and remove its data directory.
    gracefulShutdown: { signal: 'SIGINT', timeout: 10_000 },
    timeout: 30_000,
  },
})
