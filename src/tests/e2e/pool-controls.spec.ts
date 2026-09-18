import { expect, test, type Page } from "../bun-playwright";
import { installTauriMock } from "./tauri-mock";

async function expectPanelFits(page: Page) {
  const panel = page.locator(".pool-controls");
  expect(await panel.evaluate((element) => {
    const bounds = element.getBoundingClientRect();
    const selectors = ".pool-priority-label, .pool-quota-actions, .pool-speed-control, .pool-control-group, .pool-current-route, .pool-next-route, .pool-active-models, .pool-summary > div";
    return Array.from(element.querySelectorAll<HTMLElement>(selectors)).every((item) => {
      const rect = item.getBoundingClientRect();
      return rect.left >= bounds.left - 1 && rect.right <= bounds.right + 1
        && rect.top >= bounds.top - 1 && rect.bottom <= bounds.bottom + 1
        && item.scrollWidth <= item.clientWidth + 1;
    }) && bounds.left >= 0 && bounds.right <= innerWidth;
  })).toBe(true);
  expect(await panel.locator(".pool-runtime-strip, .pool-summary").evaluateAll((rows) => rows.every((row) => {
    const children = Array.from(row.children).map((child) => child.getBoundingClientRect());
    return children.every((a, index) => children.slice(index + 1).every((b) =>
      a.right <= b.left + 1 || b.right <= a.left + 1 || a.bottom <= b.top + 1 || b.bottom <= a.top + 1,
    ));
  }))).toBe(true);
}

for (const theme of ["light", "dark"] as const) {
  for (const width of [1160, 840, 390, 360]) {
    test(`pool controls stay readable in ${theme} at ${width}px`, async ({ page }, testInfo) => {
      await page.setViewportSize({ width, height: 844 });
      await installTauriMock(page, { mode: "local", locale: "ru", theme, populated: true, accountCount: 3, providerCredits: 125.5 });
      await page.goto("/");
      await page.getByRole("button", { name: "Пул", exact: true }).click();
      const panel = page.getByRole("group", { name: "Порядок использования" });
      await expect(panel.getByRole("heading", { name: "Порядок использования" })).toBeVisible();
      await expect(panel.locator(".pool-current-route")).toContainText("Работает сейчас: Personal Plus");
      await expect(panel.locator(".pool-next-route")).toContainText("Следующий кандидат");
      await expect(panel.locator("[data-active-models]")).toHaveAttribute("data-active-request-count", "1");
      await expect(panel.locator(".pool-summary > div")).toHaveCount(5);
      await expect(panel.locator('[data-tone="error"] strong')).toHaveText("1");
      await expectPanelFits(page);
      if (width === 1160) {
        expect((await panel.boundingBox())!.height).toBeLessThanOrEqual(120);
      }
      await panel.getByRole("slider").press("End");
      await expect(panel.getByRole("slider")).toBeEnabled();
      await panel.screenshot({ path: testInfo.outputPath("panel.png"), animations: "disabled" });
      await page.screenshot({ path: testInfo.outputPath("pool.png"), animations: "disabled" });
    });
  }
}

for (const mode of ["local", "remote"] as const) {
  test(`pool controls wrap long active model names in ${mode} mode`, async ({ page }, testInfo) => {
    await page.setViewportSize({ width: 390, height: 844 });
    const model = `synthetic-model-${"extended".repeat(12)}`;
    await installTauriMock(page, {
      mode, locale: "en", populated: true, accountCount: 3,
      activeModelCounts: [{ model, requestCount: 12 }, { model: "gpt-5.4", requestCount: 2 }],
    });
    await page.goto("/");
    await page.getByRole("button", { name: "Pool", exact: true }).click();
    await expect(page.locator(".pool-active-models")).toContainText(model);
    await expect(page.locator(".pool-active-models")).toHaveAttribute("data-active-request-count", "14");
    await expectPanelFits(page);
    await page.locator(".pool-controls").screenshot({ path: testInfo.outputPath("long-model.png") });
  });
}

test("pool controls keep icon commands labelled and show quota refresh progress", async ({ page }, testInfo) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Pool", exact: true }).click();
  const panel = page.getByRole("group", { name: "Usage order" });
  await panel.getByRole("button", { name: "Hide account calculation" }).click();
  await expect(panel.getByRole("button", { name: "Show account calculation" })).toHaveAttribute("aria-pressed", "false");
  await panel.getByRole("button", { name: "Pool rotation settings" }).click();
  await expect(page.getByRole("dialog", { name: "Pool rotation", exact: true })).toBeVisible();
  await page.getByRole("dialog").getByRole("button", { name: "Cancel", exact: true }).click();
  await page.evaluate(() => {
    const scope = window as unknown as {
      __TAURI_INTERNALS__: { invoke: (command: string, args?: unknown, options?: unknown) => Promise<unknown> };
      __COMPLETE_POOL_REFRESH__: () => void;
    };
    const invoke = scope.__TAURI_INTERNALS__.invoke.bind(scope.__TAURI_INTERNALS__);
    scope.__TAURI_INTERNALS__.invoke = async (command, args, options) => {
      if (command === "refresh_all_local_account_quotas") await new Promise<void>((resolve) => { scope.__COMPLETE_POOL_REFRESH__ = resolve; });
      return invoke(command, args, options);
    };
  });
  const refresh = panel.getByRole("button", { name: "Refresh", exact: true });
  await refresh.hover();
  await expect(page.getByRole("tooltip", { name: "Refresh", exact: true })).toBeVisible();
  await refresh.click();
  await expect(refresh).toBeDisabled();
  await expect(refresh).toHaveAttribute("aria-busy", "true");
  await panel.screenshot({ path: testInfo.outputPath("refreshing.png"), animations: "disabled" });
  await page.evaluate(() => (window as unknown as { __COMPLETE_POOL_REFRESH__: () => void }).__COMPLETE_POOL_REFRESH__());
  await expect(refresh).toBeEnabled();
  await expect(refresh).toHaveAttribute("aria-busy", "false");
});

test("pool controls keep idle routes distinct from active requests", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, usageActive: false });
  await page.goto("/");
  await page.getByRole("button", { name: "Pool", exact: true }).click();
  const panel = page.getByRole("group", { name: "Usage order" });
  await expect(panel.locator(".pool-current-route")).toHaveAttribute("data-active", "false");
  await expect(panel.locator(".pool-current-route")).toContainText("Next candidate");
  await expect(panel.locator(".pool-active-models")).toHaveCount(0);
});
