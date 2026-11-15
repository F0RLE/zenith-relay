import { expect, test, type Page } from "../bun-playwright";
import { installTauriMock } from "./tauri-mock";

async function chooseWorkload(page: Page, value: string) {
  await page.locator(".usage-workload-menu button").click();
  await page.locator(`[role="option"][data-value="${value}"]`).click();
}

for (const locale of ["ru", "en"] as const) {
  for (const width of [1160, 390]) {
    test(`Overview keeps all requests and Usage has compact ordered workload filters: ${locale} ${width}`, async ({ page }) => {
      await page.setViewportSize({ width, height: 900 });
      await installTauriMock(page, {
        mode: "local", locale, theme: locale === "ru" ? "dark" : "light", populated: true,
        usageFilterHistory: [
          { wireApi: "responses", transport: "http", errorCategory: null, success: true, inputTokens: 10_000, cachedInputTokens: 9_500, outputTokens: 100 },
          { wireApi: "responses", transport: "http", errorCategory: "codex_memory_extraction", success: true, inputTokens: 500_000, cachedInputTokens: 10_000, outputTokens: 4_000 },
          { wireApi: "responses", transport: "http", errorCategory: "codex_wake", success: true, inputTokens: 1_000, cachedInputTokens: 500, outputTokens: 100 },
        ],
      });
      await page.goto("/");
      await page.getByRole("button", { name: locale === "ru" ? "Обзор" : "Overview", exact: true }).click();
      const cache = page.locator(".overview-token-trend-summary small");
      await expect(cache).toContainText("4%");
      await expect.poll(() => page.locator(".overview-analytics-controls .relay-tabs").evaluate((tabs) => {
        const controls = tabs.parentElement!.getBoundingClientRect();
        const rect = tabs.getBoundingClientRect();
        return rect.width > 150 && rect.left >= controls.left - 1 && rect.right <= controls.right + 1;
      })).toBe(true);
      await expect(page.locator(".overview-analytics-controls .usage-workload-menu")).toHaveCount(0);
      await expect.poll(() => page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
      await page.screenshot({ path: test.info().outputPath(`overview-workloads-${locale}-${width}.png`) });

      await page.getByRole("button", { name: locale === "ru" ? "Использование" : "Usage", exact: true }).click();
      const scopeControls = page.locator(".usage-scope-controls");
      await expect(scopeControls.locator(":scope > :nth-child(1)")).toHaveClass(/usage-range-menu/);
      await expect(scopeControls.locator(":scope > :nth-child(2)")).toHaveClass(/usage-workload-menu/);
      await expect.poll(() => scopeControls.evaluate((controls) => {
        const bounds = controls.getBoundingClientRect();
        return Array.from(controls.children).every((child) => {
          const rect = child.getBoundingClientRect();
          return rect.left >= bounds.left - 1 && rect.right <= bounds.right + 1;
        });
      })).toBe(true);
      if (width > 760) {
        await expect.poll(() => page.locator(".usage-workload-menu").evaluate((menu) => menu.getBoundingClientRect().width)).toBeLessThanOrEqual(160);
      }
      await expect.poll(() => scopeControls.locator(".usage-range-menu .relay-option-trigger > span, .usage-workload-menu .relay-option-trigger > span")
        .evaluateAll((labels) => labels.every((label) => label.scrollWidth <= label.clientWidth + 1))).toBe(true);
      const rows = page.locator(".usage-request-table tbody tr");
      await expect(rows).toHaveCount(3);
      await chooseWorkload(page, "memory");
      await expect(rows).toHaveCount(1);
      await expect(rows.first().getByRole("button", { name: /req_filter_2/ })).toBeVisible();
      await chooseWorkload(page, "conversation");
      await expect(rows).toHaveCount(1);
      await expect(rows.first().getByRole("button", { name: /req_filter_1/ })).toBeVisible();
      await chooseWorkload(page, "background");
      await expect(rows).toHaveCount(1);
      await expect(rows.first().getByRole("button", { name: /req_filter_3/ })).toBeVisible();
      await chooseWorkload(page, "");
      await expect(rows).toHaveCount(3);
      await expect.poll(() => page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
      await page.screenshot({ path: test.info().outputPath(`workloads-${locale}-${width}.png`) });
      await scopeControls.screenshot({ path: test.info().outputPath(`usage-controls-${locale}-${width}.png`) });
      await chooseWorkload(page, "memory");
      await page.getByRole("button", { name: locale === "ru" ? "Обзор" : "Overview", exact: true }).click();
      await expect(cache).toContainText("4%");
      await expect(page.locator(".overview-analytics-controls .usage-workload-menu")).toHaveCount(0);
    });
  }
}
