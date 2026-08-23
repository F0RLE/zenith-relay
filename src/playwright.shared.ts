import type { PlaywrightTestConfig } from "@playwright/test";

const port = process.env.PLAYWRIGHT_PORT ?? "4175";
const baseURL = `http://127.0.0.1:${port}`;

export const sharedPlaywrightConfig: Pick<
  PlaywrightTestConfig,
  "outputDir" | "expect" | "reporter" | "fullyParallel" | "use" | "webServer"
> = {
  outputDir: "./test-results",
  expect: { timeout: 5_000 },
  reporter: "list",
  fullyParallel: false,
  use: {
    baseURL,
    viewport: { width: 1160, height: 760 },
  },
  webServer: {
    command: `bun run build && bun x vite preview --host 127.0.0.1 --port ${port} --strictPort`,
    url: baseURL,
    reuseExistingServer: false,
    timeout: 120_000,
  },
};
