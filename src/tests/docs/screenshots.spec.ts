import { expect, test, type Page } from "@playwright/test";
import { installTauriMock, type MockOptions } from "../e2e/tauri-mock";

// Regenerates the README screenshots from the mocked desktop shell so the
// documentation always shows the shipped light theme and the current
// navigation labels. Run with `bun run screenshots`.
const shots: Array<{ file: string; nav: string; mock: MockOptions; prepare?: (page: Page) => Promise<void> }> = [
  {
    file: "overview.png",
    nav: "Overview",
    mock: { mode: "local", populated: true, quotaAvailable: true, gatewayRunning: true },
    prepare: async (page) => {
      await expect(page.locator(".overview-metrics")).toBeVisible();
      await expect(page.locator(".activity-section li").first()).toBeVisible();
    },
  },
  {
    file: "connections.png",
    nav: "Connections",
    mock: { mode: "local", populated: true, accountCount: 4, quotaAvailable: true, supplementalQuota: true },
    prepare: async (page) => {
      await expect(page.locator(".account-list .account-card").first()).toBeVisible();
      await expect(page.locator(".account-card-quota").first()).toBeVisible();
    },
  },
  {
    file: "pool.png",
    nav: "Pool",
    mock: { mode: "local", populated: true, accountCount: 4, quotaAvailable: true, mixedModels: true },
    prepare: async (page) => {
      await expect(page.locator(".pool-member-list .pool-member-card").first()).toBeVisible();
      await expect(page.locator(".pool-summary")).toBeVisible();
    },
  },
  {
    file: "usage.png",
    nav: "Usage",
    mock: { mode: "local", populated: true, accountCount: 4, quotaAvailable: true, usageActive: true },
    prepare: async (page) => {
      await expect(page.locator(".usage-overview")).toBeVisible();
      await expect(page.locator(".usage-request-table tbody tr").first()).toBeVisible();
      await expect(page.locator(".usage-overflow")).toBeVisible();
      await expect(page.getByRole("button", { name: "Export", exact: true })).toHaveCount(0);
    },
  },
];

const width = 1160;
const minHeight = 760;
const maxHeight = 1600;

// Grows the window until the page scroller no longer clips content, so a
// screenshot never ends mid-card. Width stays fixed because the card grids
// reflow with it.
async function fitViewportToContent(page: Page) {
  for (let attempt = 0; attempt < 4; attempt += 1) {
    const overflow = await page.locator(".relay-content").evaluate((element) => element.scrollHeight - element.clientHeight);
    const height = await page.evaluate(() => innerHeight);
    if (overflow <= 0 || height >= maxHeight) return;
    // Add a small margin so the last card keeps its border and shadow.
    await page.setViewportSize({ width, height: Math.min(maxHeight, height + overflow + 16) });
    await page.waitForTimeout(150);
  }
}

for (const shot of shots) {
  test(`capture ${shot.file}`, async ({ page }) => {
    await installTauriMock(page, { locale: "en", theme: "light", ...shot.mock });
    await page.setViewportSize({ width, height: minHeight });
    await page.goto("/");
    await page.getByRole("button", { name: shot.nav, exact: true }).click();
    await expect(page.locator(".relay-page-header h1")).toBeVisible();
    await expect(page.locator(".relay-loading")).toHaveCount(0);
    await shot.prepare?.(page);
    await fitViewportToContent(page);
    // Settle icon fonts and the metric band transitions before capturing.
    await page.waitForTimeout(400);
    await page.screenshot({ path: `../docs/screenshots/${shot.file}` });
  });
}
