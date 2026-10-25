import { expect, test } from "../bun-playwright";
import { installTauriMock } from "./tauri-mock";

for (const cancel of [false, true]) {
  test(`first navigation keeps real current content until the chunk is ready${cancel ? " and latest click wins" : ""}`, async ({ page }) => {
    await installTauriMock(page, { locale: "en", populated: true });
    // Test a genuinely cold click independently of automatic idle warming.
    await page.addInitScript(() => {
      window.requestIdleCallback = () => 1;
      window.cancelIdleCallback = () => undefined;
    });
    let release!: () => void;
    let requested = false;
    const waiting = new Promise<void>((resolve) => { release = resolve; });
    await page.route(/\/PoolPage-[^/]+\.js$/, async (route) => {
      requested = true;
      await waiting;
      await route.continue();
    });
    try {
      await page.goto("/");
      await expect(page.locator(".overview-chart-stack")).toBeVisible();
      const counters = await page.locator(".overview-metrics").textContent();
      await page.getByRole("button", { name: "Pool", exact: true }).click();
      await expect.poll(() => requested).toBe(true);
      await expect(page.locator(".relay-shell")).toHaveAttribute("data-page", "overview");
      await expect(page.locator(".overview-metrics")).toHaveText(counters!);
      await expect(page.locator(".overview-chart-canvas").first()).toHaveCSS("opacity", "1");
      await expect(page.locator(".data-placeholder")).toHaveCount(0);
      await expect(page.locator(".relay-content")).not.toContainText("Loading");
      if (cancel) {
        await page.getByRole("button", { name: "Settings", exact: true }).click();
        await expect(page.locator(".settings-page")).toBeVisible();
      }
      release();
      if (cancel) {
        await expect.poll(() => page.evaluate(() => performance.getEntriesByType("resource").some((entry) => /\/PoolPage-/.test(entry.name)))).toBe(true);
        await expect(page.locator(".relay-shell")).toHaveAttribute("data-page", "settings");
        await page.getByRole("button", { name: "Pool", exact: true }).click();
      }
      await expect(page.locator(".pool-controls")).toBeVisible();
      await expect(page.locator(".overview-chart-stack")).toHaveCount(0);
      await expect(page.locator(".relay-content .relay-page")).toHaveCount(1);
    } finally { release(); }
  });
}

test("failed chunk keeps the old screen and offers retry", async ({ page }) => {
  await installTauriMock(page, { locale: "en" });
  await page.addInitScript(() => {
    window.requestIdleCallback = () => 1;
    window.cancelIdleCallback = () => undefined;
  });
  await page.route(/\/PoolPage-[^/]+\.js$/, (route) => route.abort("failed"));
  await page.goto("/");
  await expect(page.locator(".overview-chart-stack")).toBeVisible();
  await page.getByRole("button", { name: "Pool", exact: true }).click();
  await expect(page.locator(".relay-content [role=alert]")).toBeVisible();
  await expect(page.getByRole("button", { name: "Retry", exact: true })).toBeVisible();
  await expect(page.locator(".overview-chart-stack")).toBeVisible();
  await page.getByRole("button", { name: "Settings", exact: true }).click();
  await expect(page.locator(".settings-page")).toBeVisible();
  await expect(page.locator(".relay-content [role=alert]")).toHaveCount(0);
});
