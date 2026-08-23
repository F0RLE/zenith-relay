import { defineConfig } from "@playwright/test";
import { sharedPlaywrightConfig } from "./playwright.shared";

export default defineConfig({
  ...sharedPlaywrightConfig,
  testDir: "./tests/e2e",
  timeout: 45_000,
  workers: process.platform === "win32" ? 1 : 2,
  use: {
    ...sharedPlaywrightConfig.use,
    permissions: ["clipboard-read", "clipboard-write"],
    trace: "retain-on-failure",
  },
});
