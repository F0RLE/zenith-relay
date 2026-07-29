import { defineConfig } from "@playwright/test";

// Screenshot capture for docs/screenshots. Kept out of playwright.config.ts so
// `bun run test:e2e` never rewrites committed images.
export default defineConfig({
  testDir: "./tests/docs",
  outputDir: "./test-results",
  timeout: 60_000,
  expect: { timeout: 5_000 },
  reporter: "list",
  fullyParallel: false,
  workers: 1,
  use: {
    baseURL: "http://127.0.0.1:4173",
    viewport: { width: 1160, height: 760 },
    deviceScaleFactor: 2,
  },
  webServer: {
    command: "bun run build && bun run preview",
    url: "http://127.0.0.1:4173",
    reuseExistingServer: false,
    timeout: 120_000,
  },
});
