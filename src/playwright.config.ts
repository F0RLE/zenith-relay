import { defineConfig } from "@playwright/test";

export default defineConfig({
  testDir: "./tests/e2e",
  outputDir: "./test-results",
  timeout: 45_000,
  expect: { timeout: 5_000 },
  reporter: "list",
  fullyParallel: false,
  workers: process.platform === "win32" ? 1 : 2,
  use: {
    baseURL: "http://127.0.0.1:4173",
    permissions: ["clipboard-read", "clipboard-write"],
    viewport: { width: 1160, height: 760 },
    trace: "retain-on-failure",
  },
  webServer: {
    command: "bun run build && bun run preview",
    url: "http://127.0.0.1:4173",
    reuseExistingServer: false,
    timeout: 120_000,
  },
});
