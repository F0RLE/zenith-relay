import { defineConfig } from "@playwright/test";
import { sharedPlaywrightConfig } from "./playwright.shared";

export default defineConfig({
  ...sharedPlaywrightConfig,
  testDir: "./tests/e2e",
  testMatch: "hosted-preview.spec.ts",
  workers: 1,
  use: { ...sharedPlaywrightConfig.use, baseURL: "http://127.0.0.1:4177" },
  webServer: {
    command: "bun x --no-install vite --host 127.0.0.1 --port 4177 --strictPort",
    url: "http://127.0.0.1:4177",
    reuseExistingServer: false,
  },
});
