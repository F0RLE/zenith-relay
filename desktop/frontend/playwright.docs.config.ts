import { defineConfig } from "@playwright/test";
import { sharedPlaywrightConfig } from "./playwright.shared";

// Screenshot capture for docs/screenshots. Kept out of playwright.config.ts so
// `bun run test:e2e` never rewrites committed images.
export default defineConfig({
  ...sharedPlaywrightConfig,
  testDir: "./tests/docs",
  timeout: 60_000,
  workers: 1,
  use: {
    ...sharedPlaywrightConfig.use,
    deviceScaleFactor: 2,
  },
});
