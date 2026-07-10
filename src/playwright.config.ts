import { defineConfig } from "@playwright/test";

export default defineConfig({
  testDir: "./tests/e2e",
  outputDir: "./test-results",
  timeout: 45_000,
  expect: { timeout: 5_000 },
  reporter: "list",
  fullyParallel: false,
  use: {
    baseURL: "http://127.0.0.1:1420",
    viewport: { width: 1160, height: 760 },
    trace: "retain-on-failure",
  },
  webServer: {
    command: "bun run dev",
    url: "http://127.0.0.1:1420",
    reuseExistingServer: true,
    timeout: 120_000,
  },
});
