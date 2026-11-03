import { expect, test } from "../bun-playwright";
import { emitTauriEvent, installTauriMock } from "./tauri-mock";

const CACHE_KEY = "relay.overviewAnalyticsCache.v1";

test("leaving Overview discards history refreshes queued behind a slow read", async ({ page }) => {
  await page.clock.install();
  await installTauriMock(page, { locale: "en" });
  await page.goto("/");
  await expect(page.locator(".overview-analytics")).toHaveAttribute("aria-busy", "false");
  await page.clock.pauseAt(new Date(Date.now() + 1_000));
  await page.evaluate(() => {
    const target = window as unknown as {
      __TAURI_INTERNALS__: { invoke: (command: string, args?: unknown, options?: unknown) => Promise<unknown> };
      __OVERVIEW_READS__: number;
      __RELEASE_OVERVIEW__?: () => void;
    };
    target.__OVERVIEW_READS__ = 0;
    const invoke = target.__TAURI_INTERNALS__.invoke.bind(target.__TAURI_INTERNALS__);
    target.__TAURI_INTERNALS__.invoke = async (command, args, options) => {
      const result = await invoke(command, args, options);
      if (command !== "get_local_usage_page") return result;
      target.__OVERVIEW_READS__ += 1;
      if (target.__OVERVIEW_READS__ > 1) return result;
      return new Promise((resolve) => { target.__RELEASE_OVERVIEW__ = () => resolve(result); });
    };
  });
  const reads = () => page.evaluate(() => (window as unknown as { __OVERVIEW_READS__: number }).__OVERVIEW_READS__);
  await emitTauriEvent(page, "zenith-usage-recorded", null);
  await page.clock.fastForward(2_000);
  await expect.poll(reads).toBe(1);
  await emitTauriEvent(page, "zenith-usage-recorded", null);
  await page.clock.fastForward(2_000);
  await page.getByRole("button", { name: "Settings", exact: true }).click();
  await expect(page.getByRole("heading", { name: "Settings", exact: true })).toBeVisible();
  await page.evaluate(() => (window as unknown as { __RELEASE_OVERVIEW__: () => void }).__RELEASE_OVERVIEW__());
  await page.clock.runFor(2_000);
  expect(await reads()).toBe(1);
});

test("saved Overview is visible before the first native snapshot after reload", async ({ page }) => {
  await installTauriMock(page, { locale: "en" });
  await page.goto("/");
  await expect(page.locator(".overview-analytics")).toHaveAttribute("aria-busy", "false");
  const requests = await page.locator(".overview-metrics strong").first().textContent();
  await page.evaluate(() => window.dispatchEvent(new Event("pagehide")));
  await page.addInitScript(() => {
    const scope = window as unknown as {
      __TAURI_INTERNALS__: { invoke: (command: string, args?: unknown, options?: unknown) => Promise<unknown> };
      __RELEASE_STARTUP_SNAPSHOT__?: () => void;
    };
    const invoke = scope.__TAURI_INTERNALS__.invoke.bind(scope.__TAURI_INTERNALS__);
    scope.__TAURI_INTERNALS__.invoke = (command, args, options) => {
      if (command !== "get_local_runtime_state") return invoke(command, args, options);
      return new Promise((resolve, reject) => {
        scope.__RELEASE_STARTUP_SNAPSHOT__ = () => { void invoke(command, args, options).then(resolve, reject); };
      });
    };
  });
  await page.reload();
  await expect(page.locator("#splash-screen")).toHaveCount(0);
  await expect(page.locator(".overview-chart-stack")).toBeVisible();
  await expect(page.locator(".overview-metrics strong").first()).toHaveText(requests!);
  await expect(page.locator(".relay-content .data-placeholder")).toHaveCount(0);
  await expect(page.locator(".relay-content")).not.toContainText("Loading");
  await expect(page.getByRole("button", { name: "Start API", exact: true })).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Stop API", exact: true })).toHaveCount(0);
  expect(await page.evaluate(() => performance.getEntriesByName("zenith:interactive", "measure"))).toEqual([]);
  await page.screenshot({ path: "output/playwright/overview-cached-startup.png" });
  await page.evaluate(() => (window as unknown as { __RELEASE_STARTUP_SNAPSHOT__: () => void }).__RELEASE_STARTUP_SNAPSHOT__());
  await expect(page.getByRole("button", { name: "Stop API", exact: true })).toBeVisible();
});

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
