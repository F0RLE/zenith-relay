import { expect, test } from "../bun-playwright";
import { emitTauriEvent, installTauriMock } from "./tauri-mock";

type TestWindow = Window & {
  __TAURI_INTERNALS__: { invoke: (command: string, args?: unknown, options?: unknown) => Promise<unknown> };
  __TAURI_TEST_INVOKES__: Array<{ command: string }>;
  __ANALYTICS_PENDING__?: boolean;
  __RELEASE_ANALYTICS__?: () => void;
};

for (const mode of ["local"] as const) {
  test(`${mode} usage revalidates cached reports after the system clock moves back`, async ({ page }) => {
    await installTauriMock(page, { mode, locale: "en", populated: true });
    await page.goto("/");
    await page.getByRole("button", { name: "Usage", exact: true }).click();
    await expect(page.locator(".usage-request-table tbody tr")).toHaveCount(1);
    const command = "get_local_usage_page";
    const before = await page.evaluate((command) => (window as TestWindow).__TAURI_TEST_INVOKES__.filter((call) => call.command === command).length, command);
    await page.getByRole("button", { name: "Connections", exact: true }).click();
    await page.evaluate(() => { const now = Date.now; Date.now = () => now() - 60_000; });
    await page.getByRole("button", { name: "Usage", exact: true }).click();
    await expect.poll(() => page.evaluate((command) => (window as TestWindow).__TAURI_TEST_INVOKES__.filter((call) => call.command === command).length, command)).toBeGreaterThan(before);
  });
}

test("a new connection filter clears the previous report while loading", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true });
  await page.goto("/");
  await expect(page.locator(".overview-chart.tokens .overview-chart-summary")).toHaveText("28");
  await page.evaluate(() => {
    const host = window as TestWindow;
    const invoke = host.__TAURI_INTERNALS__.invoke.bind(host.__TAURI_INTERNALS__);
    host.__TAURI_INTERNALS__.invoke = (command, args, options) => {
      if (command !== "get_local_usage_page") return invoke(command, args, options);
      host.__ANALYTICS_PENDING__ = true;
      return new Promise((resolve, reject) => {
        host.__RELEASE_ANALYTICS__ = () => { void invoke(command, args, options).then(resolve, reject); };
      });
    };
  });
  await page.getByRole("button", { name: /Analytics connection:/ }).click();
  await page.getByRole("option", { name: "API · Example compatible API", exact: true }).click();
  await expect.poll(() => page.evaluate(() => (window as TestWindow).__ANALYTICS_PENDING__)).toBe(true);
  await expect(page.locator(".overview-analytics")).not.toHaveClass(/has-data/);
  await page.evaluate(() => (window as TestWindow).__RELEASE_ANALYTICS__?.());
  await expect(page.locator(".overview-analytics")).toHaveClass(/has-data/);
  await expect(page.locator(".overview-analytics")).toHaveAttribute("aria-busy", "false");
});


for (const legacy of [false, true]) {
  test(`${legacy ? "legacy" : "current"} valid persisted analytics remain available offline`, async ({ page }) => {
    await installTauriMock(page, { mode: "local", locale: "en", populated: true });
    await page.goto("/");
    await expect(page.locator(".overview-chart.tokens .overview-chart-summary")).toHaveText("28");
    if (legacy) await page.evaluate(() => {
      const key = "relay.overviewAnalyticsCache.v1";
      const entries = JSON.parse(localStorage.getItem(key)!) as Array<{ value: {
        totals: Record<string, unknown>; buckets: Array<{ totals: Record<string, unknown> }>;
      } }>;
      for (const { value } of entries) {
        for (const totals of [value.totals, ...value.buckets.map((bucket) => bucket.totals)]) {
          delete totals["cacheWriteInputTokens"];
          delete totals["cacheWriteInputSamples"];
        }
      }
      localStorage.setItem(key, JSON.stringify(entries));
    });
    await page.addInitScript(() => {
      const host = window as TestWindow;
      const invoke = host.__TAURI_INTERNALS__.invoke.bind(host.__TAURI_INTERNALS__);
      host.__TAURI_INTERNALS__.invoke = (command, args, options) => command === "get_local_usage_page"
        ? Promise.reject(new Error("Synthetic offline analytics")) : invoke(command, args, options);
    });
    await page.reload();
    await expect(page.locator(".overview-analytics").getByRole("alert")).toBeVisible();
    await expect(page.locator(".overview-chart.tokens .overview-chart-summary")).toHaveText("28");
  });
}

for (const invalid of ["bucket", "totals"] as const) {
  test(`a persisted invalid ${invalid} cannot crash Overview`, async ({ page }) => {
    await installTauriMock(page, { mode: "local", locale: "en", populated: true });
    await page.goto("/");
    await expect(page.locator(".overview-analytics")).toHaveClass(/has-data/);
    await page.evaluate((invalid) => {
      const key = "relay.overviewAnalyticsCache.v1";
      const entries = JSON.parse(localStorage.getItem(key)!) as Array<{ value: { totals: unknown; buckets: unknown[] } }>;
      for (const entry of entries) {
        if (invalid === "bucket") entry.value.buckets = [null];
        else entry.value.totals = {};
      }
      localStorage.setItem(key, JSON.stringify(entries));
    }, invalid);
    await page.addInitScript(() => {
      const host = window as TestWindow;
      const invoke = host.__TAURI_INTERNALS__.invoke.bind(host.__TAURI_INTERNALS__);
      host.__TAURI_INTERNALS__.invoke = (command, args, options) => command === "get_local_usage_page"
        ? Promise.reject(new Error("Synthetic offline analytics")) : invoke(command, args, options);
    });
    const errors: string[] = [];
    page.on("pageerror", (error) => errors.push(error.message));
    await page.reload();
    await expect(page.getByRole("heading", { name: "Overview", exact: true })).toBeVisible();
    await expect(page.locator(".overview-analytics").getByRole("alert")).toBeVisible();
    await expect(page.locator(".overview-analytics")).not.toHaveClass(/has-data/);
    expect(errors).toEqual([]);
  });
}
