import type { PlaywrightTestConfig } from "@playwright/test";

export const sharedPlaywrightConfig: Pick<
  PlaywrightTestConfig,
  "outputDir" | "expect" | "reporter" | "fullyParallel" | "use" | "webServer"
> = {
  outputDir: "./test-results",
  expect: { timeout: 5_000 },
  reporter: "list",
  fullyParallel: false,
  use: {
    baseURL: "http://127.0.0.1:4173",
    viewport: { width: 1160, height: 760 },
  },
  webServer: {
    command: "bun run build && bun run preview",
    url: "http://127.0.0.1:4173",
    reuseExistingServer: false,
    timeout: 120_000,
  },
};
