import { expect, test } from "../bun-playwright";
import type { LocalUsagePage } from "../../src/features/relay/api/types";
import { installTauriMock } from "./tauri-mock";

for (const locale of ["en", "ru"] as const) {
  for (const requests of [1, 5]) {
    test(`${locale} cache rate survives ${requests} logical requests and two measured sends`, async ({ page }) => {
      await installTauriMock(page, {
        mode: "local", locale, populated: true,
        usageSummaryTotals: {
          requests, successfulRequests: 1,
          inputTokens: 500, cachedInputTokens: 250, cachedInputSamples: 2,
          cacheRateInputTokens: 400, cacheRateCachedInputTokens: 250,
          outputTokens: 100, totalTokens: 600,
        },
      });
      await page.addInitScript(() => {
        const host = window as unknown as {
          __TAURI_INTERNALS__: { invoke: (command: string, args: unknown, options?: unknown) => Promise<unknown> };
        };
        const invoke = host.__TAURI_INTERNALS__.invoke.bind(host.__TAURI_INTERNALS__);
        host.__TAURI_INTERNALS__.invoke = async (command, args, options) => {
          const result = await invoke(command, args, options);
          if (command !== "get_local_usage_page") return result;
          const report = result as LocalUsagePage;
          const current = report.buckets?.[0];
          if (!current) return report;
          const { bucketMs } = (args as { input: { bucketMs: number } }).input;
          return {
            ...report,
            buckets: [
              { startMs: current.startMs - bucketMs, totals: {
                ...report.totals, requests: 0, successfulRequests: 0,
                inputTokens: 100, cachedInputTokens: 25, cachedInputSamples: 1,
                cacheRateInputTokens: 100, cacheRateCachedInputTokens: 25,
              } },
              { startMs: current.startMs, totals: {
                ...report.totals, inputTokens: 400, cachedInputTokens: 225, cachedInputSamples: 1,
                cacheRateInputTokens: 300, cacheRateCachedInputTokens: 225,
              } },
            ],
          };
        };
      });
      await page.goto("/");
      const chart = page.locator(".overview-token-trend");
      await expect(page.locator(".overview-analytics")).toHaveAttribute("aria-busy", "false");
      await expect(chart.locator(".overview-token-trend-summary small")).toContainText("63%");
      await expect(chart.locator(".overview-token-trend-summary small")).not.toHaveAttribute("data-relay-tooltip");
      const dots = chart.locator(".overview-token-trend-dot.is-cache-rate");
      await expect(dots).toHaveCount(2);
      await expect(dots.nth(0)).toHaveAttribute("aria-label", /25%/);
      await expect(dots.nth(1)).toHaveAttribute("aria-label", /75%$/);
      await expect(chart.locator("path.is-cache-rate")).toHaveCount(1);
      await expect(chart.locator(".overview-token-trend-dot.is-input")).toHaveCount(2);
      await dots.nth(1).focus();
      await expect(dots.nth(1).getByRole("tooltip")).toBeVisible();
      await expect(dots.nth(1).getByRole("tooltip")).toHaveText(locale === "en"
        ? /.+Cache hit rate: 75%$/ : /.+Попадания в кэш: 75%$/);
      await expect(chart).not.toContainText(/Рассчитано|Based on|Missing measurements|Неизвестные измерения/);
      await chart.screenshot({ path: `output/playwright/overview-cache-rate-${locale}-${requests}.png` });
      await page.setViewportSize({ width: 390, height: 844 });
      await dots.nth(1).focus();
      const tooltip = await dots.nth(1).getByRole("tooltip").boundingBox();
      expect(tooltip).not.toBeNull();
      expect(tooltip!.x).toBeGreaterThanOrEqual(0);
      expect(tooltip!.x + tooltip!.width).toBeLessThanOrEqual(390);
    });
  }
}
