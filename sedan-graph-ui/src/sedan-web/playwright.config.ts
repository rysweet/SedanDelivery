import { defineConfig } from '@playwright/test'

export default defineConfig({
  testDir: './e2e',
  timeout: 60_000,
  retries: 0,
  // These suites share fixed graph names against a single local backend.
  // Keep one worker to avoid cross-file state interference.
  workers: 1,
  use: {
    baseURL: 'http://localhost:5175',
    headless: true,
    screenshot: 'on',
    trace: 'retain-on-failure',
    viewport: { width: 1440, height: 900 },
  },
  projects: [
    {
      name: 'chromium',
      use: { browserName: 'chromium' },
    },
  ],
  webServer: {
    command: 'VITE_PROXY_TARGET=http://localhost:3001 VITE_PROXY_REWRITE=true VITE_E2E_TEST_AUTH=true npx vite --port 5175',
    port: 5175,
    timeout: 30_000,
    reuseExistingServer: true,
  },
})
