import { expect, test } from "../bun-playwright";
import type { RuntimeSnapshot, SourceStats } from "../../src/features/relay/api/types";
import { emitTauriEvent, installTauriMock } from "./tauri-mock";

const sub2api: SourceStats = { provider: "sub2api", status: "available", balanceKind: "wallet", balanceMicroUsd: 12_340_000, spentMicroUsd: null, requests: null, totalTokens: null };

for (const [theme, width] of [["dark", 1160], ["light", 390]] as const) {
  test(`stale balance keeps its failure hint until newer evidence in ${theme} ${width}`, async ({ page }, testInfo) => {
    const observed = { ...sub2api, asOfMs: 100 };
    await page.setViewportSize({ width, height: 900 });
    await installTauriMock(page, { mode: "local", locale: "en", theme, sourceStats: [observed] });
    await page.goto("/");
    await page.getByRole("button", { name: "Pool", exact: true }).click();
    const source = page.locator('.pool-member-card[data-member-kind="source"]').first();
    const balance = source.locator('[data-metric="balance"]');
    await expect(balance).toContainText("$12.34");
    await page.evaluate((observed) => {
      const host = window as unknown as {
        __TAURI_INTERNALS__: { invoke: (command: string, args?: Record<string, unknown>, options?: unknown) => Promise<unknown> };
        __sourceStatsObservation: SourceStats;
      };
      host.__sourceStatsObservation = observed;
      const invoke = host.__TAURI_INTERNALS__.invoke.bind(host.__TAURI_INTERNALS__);
      let failed = false;
      host.__TAURI_INTERNALS__.invoke = async (command, args, options) => {
        if (command === "get_local_source_stats" && args?.force) {
          failed = true;
          throw { code: "upstream_error", message: "Synthetic balance failure" };
        }
        const result = await invoke(command, args, options);
        if (command === "get_local_runtime_state" && failed) {
          const snapshot = result as RuntimeSnapshot;
          snapshot.sources[0]!.providerStats = host.__sourceStatsObservation;
          snapshot.sources[0]!.name = "Synthetic refreshed source";
        }
        return result;
      };
    }, observed);
    await source.getByRole("button", { name: "Refresh balance", exact: true }).click();
    await expect(source).toContainText("Synthetic refreshed source");
    await expect(balance).toContainText("$12.34");
    await page.screenshot({ path: testInfo.outputPath("stale-balance.png"), fullPage: true });
    await expect(balance).toHaveAttribute("data-relay-tooltip", "Last known value — Temporarily unavailable");
    await expect(balance.locator("dd")).toHaveAttribute("data-muted", "true");
    await balance.hover();
    await expect(page.getByRole("tooltip")).toHaveText("Last known value — Temporarily unavailable");
    await expect(page.getByRole("tooltip")).toHaveCSS("opacity", "1");
    await page.screenshot({ path: testInfo.outputPath("stale-balance-tooltip.png"), fullPage: true });
    await page.evaluate((observed) => {
      (window as unknown as { __sourceStatsObservation: SourceStats }).__sourceStatsObservation = {
        ...observed, asOfMs: 200, balanceMicroUsd: 22_000_000,
      };
    }, observed);
    await emitTauriEvent(page, "zenith-state-changed", null);
    await expect(balance).toContainText("$22.00");
    await expect(balance).not.toHaveAttribute("data-relay-tooltip");
    await expect(balance.locator("dd")).not.toHaveAttribute("data-muted");
    await expect(page.getByRole("tooltip")).not.toBeVisible();
  });
}

test("an unversioned background balance cannot replace a timestamped observation", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", sourceStats: [{ ...sub2api, asOfMs: 100 }] });
  await page.goto("/");
  await page.getByRole("button", { name: "Pool", exact: true }).click();
  const source = page.locator('.pool-member-card[data-member-kind="source"]').first();
  const balance = source.locator('[data-metric="balance"]');
  await expect(balance).toContainText("$12.34");
  await page.evaluate((unversioned) => {
    const native = (window as unknown as { __TAURI_INTERNALS__: { invoke: (command: string, args?: unknown, options?: unknown) => Promise<unknown> } }).__TAURI_INTERNALS__;
    const invoke = native.invoke.bind(native);
    native.invoke = async (command, args, options) => {
      const result = await invoke(command, args, options);
      if (command === "get_local_runtime_state") {
        const snapshot = result as RuntimeSnapshot;
        snapshot.sources[0]!.providerStats = unversioned;
        snapshot.sources[0]!.name = "Synthetic unversioned snapshot";
      }
      return result;
    };
  }, { ...sub2api, balanceMicroUsd: 2_000_000 });
  await emitTauriEvent(page, "zenith-state-changed", null);
  await expect(source).toContainText("Synthetic unversioned snapshot");
  await expect(balance).toContainText("$12.34");
  await expect(balance).not.toContainText("$2.00");
});

for (const failure of ["catalog-status", "catalog-rejection", "balance-rejection", "balance-status"] as const) {
  test(`bulk source refresh reports ${failure} instead of success`, async ({ page }) => {
    await installTauriMock(page, { mode: "local", locale: "en", sourceStats: [sub2api] });
    const errors: string[] = [];
    page.on("pageerror", (error) => errors.push(error.message));
    await page.goto("/");
    await page.getByRole("button", { name: "Connections", exact: true }).click();
    await page.getByRole("tab", { name: "Sources", exact: true }).click();
    await expect(page.locator(".source-card").locator('[data-metric="balance"]')).toContainText("$12.34");
    await page.evaluate((failure) => {
      const native = (window as unknown as { __TAURI_INTERNALS__: { invoke: (command: string, args?: Record<string, unknown>, options?: unknown) => Promise<unknown> } }).__TAURI_INTERNALS__;
      const invoke = native.invoke.bind(native);
      let refreshed = false;
      native.invoke = async (command, args, options) => {
        if (command === "get_local_runtime_state" && refreshed) {
          const snapshot = await invoke(command, args, options) as RuntimeSnapshot;
          snapshot.sources[0]!.name = "Synthetic refreshed source";
          return snapshot;
        }
        if (command === "refresh_local_source_data") {
          refreshed = true;
          if (failure === "catalog-rejection") throw new Error("Synthetic catalog failure");
          return { sourceId: args?.sourceId, status: failure === "catalog-status" ? "failed" : "complete", addedModels: 0, removedModels: 0 };
        }
        if (command === "get_local_source_stats" && args?.force) {
          if (failure === "balance-rejection") throw new Error("Synthetic balance failure");
          if (failure === "balance-status") return { provider: "sub2api", status: "rate_limited", balanceMicroUsd: null };
        }
        return invoke(command, args, options);
      };
    }, failure);
    await page.locator(".sources-controls").getByRole("button", { name: "Refresh API data", exact: true }).click();
    await expect(page.getByRole("button", { name: "Show details", exact: true })).toBeVisible();
    await expect(page.locator(".source-card")).toContainText("Synthetic refreshed source");
    await expect(page.locator(".source-card").locator('[data-metric="balance"]')).toContainText("$12.34");
    expect(errors).toEqual([]);
  });
}

for (const mode of ["local"] as const) {
  test(`${mode} uses provider balance and labels local estimate honestly`, async ({ page }) => {
    await installTauriMock(page, { mode, locale: "en", sourceStats: [sub2api] });
    await page.goto("/");
    await page.getByRole("button", { name: "Pool", exact: true }).click();
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
    await page.getByRole("button", { name: "Pool", exact: true }).click();
    const panel = page.locator(".source-stats-panel");
    await expect(panel).toContainText("$12.34");
    const refresh = page.locator('.pool-member-card[data-member-kind="source"]').getByRole("button", { name: "Refresh balance", exact: true });
    await refresh.click();
    await expect(panel.getByText("Not refreshed", { exact: true })).toHaveCount(0);
    await expect(panel).toContainText("$12.34");
  });
}

for (const [status, label] of [["unauthorized", "Stats access denied"], ["unsupported", "No balance API"], ["invalid_response", "Unknown format"]] as const) {
  test(`first ${status} response shows a clear state without invented money`, async ({ page }) => {
    await installTauriMock(page, { mode: "local", locale: "en", sourceStats: [{ ...sub2api, status, balanceMicroUsd: null }] });
    await page.goto("/");
    await page.getByRole("button", { name: "Pool", exact: true }).click();
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
  await installTauriMock(page, { mode: "local", locale: "en", sourceStats: [{ ...sub2api, provider: "openrouter", balanceKind: "key_quota", balanceUnlimited: true, balanceMicroUsd: null }] });
  await page.goto("/");
  await page.getByRole("button", { name: "Pool", exact: true }).click();
  const balance = page.locator('[data-metric="balance"]');
  await expect(balance).toContainText("Key remaining");
  await expect(balance).toContainText("No limit");
});

test("API source connection cards show model totals without pool activity", async ({ page }) => {
  await installTauriMock(page, {
    mode: "local",
    locale: "en",
    accountCount: 0,
    sourceStats: [{ ...sub2api, spentMicroUsd: 3_210_000, requests: 27 }],
  });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await page.getByRole("tab", { name: "Sources", exact: true }).click();

  const card = page.locator(".source-card");
  await expect(card.locator('[data-metric="balance"]')).toContainText("$12.34");
  await expect(card.locator('[data-metric="spend"]')).toContainText("$3.21");
  await expect(card.locator('[data-metric="requests"]')).toContainText("27");
  await expect(card.locator('[data-metric="models"]')).toContainText("2");
  await expect(card.locator(".pool-member-context")).toHaveCount(0);
  await expect(card).not.toContainText("Active requests");
  await expect(card).not.toContainText("Operation mode");
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
