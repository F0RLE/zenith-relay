import { defineConfig } from "@playwright/test";

const executablePath = process.env["PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH"];
export default defineConfig({
  testDir: "tests/e2e",
  fullyParallel: true,
  workers: 2,
  reporter: "list",
  use: {
    baseURL: "http://127.0.0.1:1442",
    browserName: "chromium",
    launchOptions: executablePath ? { executablePath } : {},
    screenshot: "only-on-failure",
    trace: "retain-on-failure",
  },
  projects: [
    { name: "desktop-light", use: { viewport: { width: 1440, height: 1000 }, colorScheme: "light" } },
    { name: "desktop-dark", use: { viewport: { width: 1024, height: 1000 }, colorScheme: "dark" } },
    { name: "tablet", use: { viewport: { width: 768, height: 1000 }, colorScheme: "light" } },
    { name: "mobile-light", use: { viewport: { width: 390, height: 844 }, colorScheme: "light" } },
    { name: "mobile-dark", use: { viewport: { width: 320, height: 740 }, colorScheme: "dark" } },
  ],
  webServer: {
    command: "bunx --no-install vite --host 127.0.0.1 --port 1442 --strictPort",
    url: "http://127.0.0.1:1442/preview.html",
    reuseExistingServer: false,
  },
});
