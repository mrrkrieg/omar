import { defineConfig, devices } from "@playwright/test";

/** Fixed so the app can be launched pointing at it before any test runs. */
export const FAKE_SERVE_PORT = 7399;
export const FAKE_SERVE_URL = `http://127.0.0.1:${FAKE_SERVE_PORT}`;

// Local validation can use a free port without reusing an unrelated service.
const appPort = process.env.OMAR_E2E_PORT ?? "3100";
const appUrl = `http://127.0.0.1:${appPort}`;

/**
 * Drives the built application, not the dev server, so the E2E test exercises
 * what actually ships. The fake `omar serve` is started per test file rather
 * than here, so each test owns its run lifecycle.
 */
export default defineConfig({
  testDir: "./tests/e2e",
  fullyParallel: false,
  forbidOnly: !!process.env.CI,
  retries: process.env.CI ? 1 : 0,
  workers: 1,
  reporter: process.env.CI ? "line" : "list",
  timeout: 60_000,
  use: {
    baseURL: appUrl,
    trace: "retain-on-failure",
  },
  projects: [{ name: "chromium", use: { ...devices["Desktop Chrome"] } }],
  webServer: {
    command: `npm run start -- --port ${appPort}`,
    url: appUrl,
    reuseExistingServer: !process.env.CI && !process.env.OMAR_E2E_PORT,
    timeout: 120_000,
    // Mode is a launch flag, so the app must be started pointing at the fake
    // daemon. Tests bind it to this fixed port.
    env: { OMAR_SERVE_URL: FAKE_SERVE_URL },
  },
});
