import { expect, test } from "../bun-playwright";
import type { SourceStats } from "../../src/features/relay/api/types";
import { installTauriMock } from "./tauri-mock";

const sub2api: SourceStats = { provider: "sub2api", status: "available", balanceKind: "wallet", balanceMicroUsd: 12_340_000, spentMicroUsd: null, requests: null, totalTokens: null };

for (const mode of ["local", "remote", "zenith"] as const) {
  test(`${mode} uses provider balance and labels local estimate honestly`, async ({ page }) => {
    await installTauriMock(page, { mode, locale: "en", sourceStats: [sub2api] });
    await page.goto("/");
    if (mode !== "zenith") await page.getByRole("button", { name: "Pool", exact: true }).click();
    const panel = page.locator(".source-stats-panel");
    await expect(panel).toContainText("$12.34");
    await expect(panel).toContainText("Relay estimate");
    await expect(panel.getByText("Spent", { exact: true })).toHaveCount(0);
    await expect(panel.locator('[data-metric="requests"]')).toHaveCount(0);
    await expect(panel.locator(".source-stats-caption")).toHaveCount(0);
  });
  test(`${mode} keeps old balance with a visible warning on refresh failure`, async ({ page }) => {
    await installTauriMock(page, { mode, locale: "en", sourceStats: [sub2api, { ...sub2api, status: "rate_limited", balanceMicroUsd: null }] });
    await page.goto("/");
    if (mode !== "zenith") await page.getByRole("button", { name: "Pool", exact: true }).click();
    const panel = page.locator(".source-stats-panel");
    await expect(panel).toContainText("$12.34");
    const refresh = mode === "zenith" ? page.getByRole("button", { name: "Refresh", exact: true })
      : page.locator('.pool-member-card[data-member-kind="source"]').getByRole("button", { name: "Refresh balance", exact: true });
    await refresh.click();
    await expect(panel).toContainText("Not refreshed");
    await expect(panel).toContainText("$12.34");
  });
}

for (const [status, label] of [["unauthorized", "Stats access denied"], ["unsupported", "No balance API"], ["invalid_response", "Unknown format"]] as const) {
  test(`first ${status} response shows a clear state without invented money`, async ({ page }) => {
    await installTauriMock(page, { mode: "zenith", locale: "en", sourceStats: [{ ...sub2api, status, balanceMicroUsd: null }] });
    await page.goto("/");
    await expect(page.locator('[data-metric="balance"]')).toContainText(label);
    await expect(page.locator('[data-metric="balance"]')).not.toContainText("$0.00");
  });
}

for (const theme of ["light", "dark"] as const) {
  for (const width of [390, 780, 1440]) {
    test(`provider units and key limit fit ${theme} ${width}`, async ({ page }, testInfo) => {
      await page.setViewportSize({ width, height: 900 });
      await installTauriMock(page, { mode: "local", locale: "ru", theme, accountCount: 0, sourceStats: [{
        ...sub2api, provider: "new_api", balanceKind: "key_quota", balanceMicroUsd: null,
        amounts: [{ currency: "CREDITS", balanceMicros: 12_345_670_000_000, spentMicros: null }],
      }] });
      await page.goto("/");
      await page.getByRole("button", { name: "Пул", exact: true }).click();
      const panel = page.locator(".source-stats-panel");
      await expect(panel).toContainText("Остаток ключа");
      await expect(panel).toContainText("ед.");
      expect(await panel.locator("dd, dt").evaluateAll((elements) => elements.every((el) => el.scrollWidth <= el.clientWidth + 1 && el.scrollHeight <= el.clientHeight + 1))).toBe(true);
      await page.screenshot({ path: testInfo.outputPath("provider-stats.png"), fullPage: true });
    });
  }
}

test("unlimited key is not an unlimited wallet", async ({ page }) => {
  await installTauriMock(page, { mode: "zenith", locale: "en", sourceStats: [{ ...sub2api, provider: "openrouter", balanceKind: "key_quota", balanceUnlimited: true, balanceMicroUsd: null }] });
  await page.goto("/");
  const balance = page.locator('[data-metric="balance"]');
  await expect(balance).toContainText("Key remaining");
  await expect(balance).toContainText("No limit");
});

test("different sources retain their own amounts and currencies", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", sourceCount: 2, sourceStatsById: {
    source_synthetic: sub2api,
    source_synthetic_2: { ...sub2api, provider: "deepseek", balanceMicroUsd: null, amounts: [
      { currency: "CNY", balanceMicros: 110_000_000, spentMicros: null },
      { currency: "USD", balanceMicros: 2_500_000, spentMicros: null },
    ] },
  } });
  await page.goto("/");
  await page.getByRole("button", { name: "Pool", exact: true }).click();
  const panels = page.locator(".source-stats-panel");
  await expect(panels.nth(0)).toContainText("$12.34");
  await expect(panels.nth(1)).toContainText("CN¥110.00");
  await expect(panels.nth(1)).toContainText("$2.50");
  await expect(panels.nth(1)).not.toContainText("$12.34");
});

test("late initial stats cannot overwrite a newer bulk refresh", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", sourceStatsDelayMs: [2000, 0],
    sourceStats: [sub2api, { ...sub2api, balanceMicroUsd: 22_000_000 }],
  });
  await page.goto("/");
  await page.getByRole("button", { name: "Pool", exact: true }).click();
  await page.locator('[data-toolbar-group="refresh"]').getByRole("button", { name: "Refresh", exact: true }).click();
  const panel = page.locator(".source-stats-panel");
  await expect(panel).toContainText("$22.00");
  // Let the deliberately delayed first request finish after the newer result.
  await page.waitForTimeout(2100);
  await expect(panel).toContainText("$22.00");
  await expect(panel).not.toContainText("$12.34");
});

test("catalog failure does not prevent the independent balance refresh", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", sourceRefreshError: true,
    sourceStats: [sub2api, { ...sub2api, balanceMicroUsd: 22_000_000 }],
  });
  await page.goto("/");
  await page.getByRole("button", { name: "Pool", exact: true }).click();
  const panel = page.locator(".source-stats-panel");
  await expect(panel).toContainText("$12.34");
  await page.locator('[data-toolbar-group="refresh"]').getByRole("button", { name: "Refresh", exact: true }).click();
  await expect(panel).toContainText("$22.00");
  await expect(panel).not.toContainText("Not refreshed");
});
