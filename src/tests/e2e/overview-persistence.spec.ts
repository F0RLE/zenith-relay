import { expect, test } from "../bun-playwright";
import { emitTauriEvent, installTauriMock } from "./tauri-mock";

const CACHE_KEY = "relay.overviewAnalyticsCache.v1";

test("page hide flushes the latest Overview cache without a duplicate delayed write", async ({ page }) => {
  await page.clock.install();
  await installTauriMock(page, { locale: "en" });
  await page.addInitScript((key) => {
    const scope = window as unknown as { __CACHE_WRITES__: number };
    scope.__CACHE_WRITES__ = 0;
    const setItem = Storage.prototype.setItem;
    Storage.prototype.setItem = function (name, value) {
      if (name === key) scope.__CACHE_WRITES__ += 1;
      setItem.call(this, name, value);
    };
  }, CACHE_KEY);
  await page.goto("/");
  await expect(page.locator(".overview-analytics")).toHaveAttribute("aria-busy", "false");
  await page.clock.fastForward(500);
  await page.evaluate(() => {
    const scope = window as unknown as {
      __TAURI_INTERNALS__: { invoke: (command: string, args?: unknown, options?: unknown) => Promise<unknown> };
      __LATEST_ANALYTICS_READ__?: boolean;
    };
    const invoke = scope.__TAURI_INTERNALS__.invoke.bind(scope.__TAURI_INTERNALS__);
    scope.__TAURI_INTERNALS__.invoke = async (command, args, options) => {
      const result = await invoke(command, args, options);
      if (command !== "get_local_usage_page") return result;
      const page = structuredClone(result) as { totals: { requests: number } };
      page.totals.requests = 77;
      scope.__LATEST_ANALYTICS_READ__ = true;
      return page;
    };
  });
  await emitTauriEvent(page, "zenith-usage-recorded", null);
  await page.clock.fastForward(2_000);
  await expect.poll(() => page.evaluate(() => (window as unknown as { __LATEST_ANALYTICS_READ__?: boolean }).__LATEST_ANALYTICS_READ__)).toBe(true);
  await expect(page.locator(".overview-analytics")).toHaveAttribute("aria-busy", "false");
  await page.evaluate(() => window.dispatchEvent(new Event("pagehide")));
  expect(await page.evaluate((key) => {
    const stored = JSON.parse(localStorage.getItem(key) ?? "[]") as Array<{ value: { totals: { requests: number } } }>;
    return stored.some((entry) => entry.value.totals.requests === 77);
  }, CACHE_KEY)).toBe(true);
  const writes = await page.evaluate(() => (window as unknown as { __CACHE_WRITES__: number }).__CACHE_WRITES__);
  await page.clock.fastForward(1_000);
  expect(await page.evaluate(() => (window as unknown as { __CACHE_WRITES__: number }).__CACHE_WRITES__)).toBe(writes);
  await page.reload();
  await expect(page.locator(".overview-analytics")).toBeVisible();
  await expect(page.locator(".overview-analytics")).toHaveAttribute("aria-busy", "false");
});

test("activity-only pool updates do not reserialize the API statistics inventory", async ({ page }) => {
  await installTauriMock(page, { locale: "en", accountCount: 1, quotaAvailable: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Pool", exact: true }).click();
  await expect(page.locator(".pool-source-stats").first()).toContainText("$42.50");
  await page.evaluate(() => {
    const scope = window as unknown as { __STATS_SERIALIZATIONS__: number };
    scope.__STATS_SERIALIZATIONS__ = 0;
    const stringify = JSON.stringify;
    JSON.stringify = ((value: unknown, ...args: unknown[]) => {
      if (Array.isArray(value) && value[0] === "local" && Array.isArray(value[1])
        && value[1].some((row: unknown) => Array.isArray(row) && row[0] === "source_synthetic")) {
        scope.__STATS_SERIALIZATIONS__ += 1;
      }
      return Reflect.apply(stringify, JSON, [value, ...args]);
    }) as typeof JSON.stringify;
  });
  for (let revision = 1; revision <= 5; revision += 1) {
    await emitTauriEvent(page, "zenith-runtime-activity", {
      runtimeId: 1, revision, candidateId: "account_synthetic",
      inFlight: revision, activeRequestCount: revision,
      activeModels: [{ model: "gpt-5.4", requestCount: revision }],
    });
    await expect(page.locator('.pool-controls [data-active-request-count]')).toHaveAttribute("data-active-request-count", String(revision));
  }
  expect(await page.evaluate(() => (window as unknown as { __STATS_SERIALIZATIONS__: number }).__STATS_SERIALIZATIONS__)).toBe(0);
  await emitTauriEvent(page, "zenith-state-changed", null);
  await expect.poll(() => page.evaluate(() => (window as unknown as { __STATS_SERIALIZATIONS__: number }).__STATS_SERIALIZATIONS__)).toBeGreaterThan(0);
});
