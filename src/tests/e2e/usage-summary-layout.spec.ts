import { expect, test } from "../bun-playwright";
import { installTauriMock } from "./tauri-mock";

for (const scenario of [
  { width: 1660, locale: "ru", theme: "dark" },
  { width: 1160, locale: "en", theme: "light" },
  { width: 840, locale: "ru", theme: "dark" },
  { width: 390, locale: "en", theme: "light" },
  { width: 360, locale: "ru", theme: "dark" },
] as const) {
  for (const cacheWriteKnown of [false, true]) {
    test(`usage summary centers and fills available space: ${scenario.width} ${scenario.locale}, cache write=${cacheWriteKnown}`, async ({ page }) => {
      await page.setViewportSize({ width: scenario.width, height: 1000 });
      await installTauriMock(page, {
        ...scenario, mode: "local", populated: true,
        usageSummaryTotals: {
          requests: 56_498, successfulRequests: 52_692,
          totalTokens: 8_338_700_000, inputTokens: 8_300_000_000, outputTokens: 38_700_000,
          cachedInputTokens: 7_500_000_000, cachedInputSamples: 56_498,
          cacheWriteInputTokens: cacheWriteKnown ? 190_100_000 : 0,
          cacheWriteInputSamples: cacheWriteKnown ? 56_498 : 0,
          generationOutputTokens: 38_700_000, generationMs: 834_051_724,
          speedOutputTokens: 38_700_000, speedDurationMs: 1_176_291_793,
          apiEquivalent: { microUsd: 3_112_864_200, pricedTokens: 8_178_000_000, unpricedTokens: 160_700_000 },
        },
      });
      await page.goto("/");
      await page.getByRole("button", { name: scenario.locale === "ru" ? "Использование" : "Usage", exact: true }).click();
      const summary = page.locator(".usage-overview");
      await expect(summary.locator(".usage-token-totals dd")).toHaveCount(cacheWriteKnown ? 4 : 3);
      const copyAlignment = await summary.locator(".usage-metric-copy").evaluateAll((copies) => copies.flatMap((copy) => {
        const bounds = copy.getBoundingClientRect();
        return Array.from(copy.children).map((child) => ({
          alignment: getComputedStyle(child).textAlign,
          offset: Math.abs(child.getBoundingClientRect().left + child.getBoundingClientRect().width / 2 - bounds.left - bounds.width / 2),
          fits: child.getBoundingClientRect().left >= bounds.left - 1 && child.getBoundingClientRect().right <= bounds.right + 1,
        }));
      }));
      for (const value of copyAlignment) {
        expect(value.alignment).toBe("center");
        expect(value.offset).toBeLessThanOrEqual(1);
        expect(value.fits).toBe(true);
      }
      expect(await summary.locator("dt, dd").evaluateAll((elements) => elements.every((element) => getComputedStyle(element).textAlign === "center"))).toBe(true);
      expect(await summary.locator(".usage-metrics strong").evaluateAll((elements) => elements.every((element) => element.clientHeight <= parseFloat(getComputedStyle(element).lineHeight) + 1))).toBe(true);
      const secondary = summary.locator(".usage-secondary-metrics");
      expect(await secondary.evaluate((element) => getComputedStyle(element).gridTemplateColumns.split(" ").length)).toBe(2);
      const tokens = summary.locator(".usage-token-totals");
      if (scenario.width >= 840) {
        expect(await tokens.evaluate((element) => getComputedStyle(element).gridTemplateColumns.split(" ").filter((track) => parseFloat(track) > 0).length)).toBe(cacheWriteKnown ? 4 : 3);
      } else if (!cacheWriteKnown) {
        expect(await tokens.locator(":scope > div").last().evaluate((element) => element.getBoundingClientRect().width / element.parentElement!.getBoundingClientRect().width)).toBeGreaterThan(0.99);
      }
      expect(await summary.locator("div, dt, dd, span, strong, small").evaluateAll((elements) => elements
        .filter((element) => element.getBoundingClientRect().width > 0)
        .every((element) => element.scrollWidth <= element.clientWidth + 1))).toBe(true);
      await page.mouse.move(0, 0);
      await summary.screenshot({ path: `output/playwright/usage-summary-centered-${scenario.locale}-${scenario.width}-${cacheWriteKnown ? "known" : "unknown"}.png` });
      if (scenario.width === 1660 && cacheWriteKnown) {
        await page.screenshot({ path: "output/playwright/usage-centered-full-ru-1660.png" });
      }
    });
  }
}
