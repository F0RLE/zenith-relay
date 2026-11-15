import { expect, test, type Page } from "../bun-playwright";
import { emitTauriEvent, installTauriMock } from "./tauri-mock";

const usageReads = (page: Page) => page.evaluate(() => (window as unknown as {
  __TAURI_TEST_INVOKES__: Array<{ command: string }>;
}).__TAURI_TEST_INVOKES__.filter(({ command }) => command === "get_local_usage_page").length);

async function openUsage(page: Page) {
  await page.clock.install();
  await installTauriMock(page, { locale: "en" });
  await page.goto("/");
  await page.getByRole("button", { name: "Usage", exact: true }).click();
  await expect(page.locator(".usage-page")).toHaveAttribute("aria-busy", "false");
  await page.clock.pauseAt(new Date(Date.now() + 1_000));
}

test("unrelated state events and idle time do not reload Usage", async ({ page }) => {
  await openUsage(page);
  const before = await usageReads(page);
  for (let index = 0; index < 5; index += 1) {
    await emitTauriEvent(page, "zenith-state-changed", null);
    await page.clock.fastForward(1_000);
  }
  await page.clock.fastForward(120_000);
  expect(await usageReads(page)).toBe(before);
  await expect(page.locator(".usage-page")).toHaveAttribute("aria-busy", "false");
});

test("a burst of history writes refreshes once without locking the displayed report", async ({ page }) => {
  await openUsage(page);
  const before = await usageReads(page);
  await page.evaluate(() => {
    const target = window as unknown as {
      __TAURI_INTERNALS__: { invoke: (command: string, args?: unknown, options?: unknown) => Promise<unknown> };
      __RELEASE_USAGE_REFRESH__?: () => void;
      __USAGE_FORMAT_CALLS__?: number;
    };
    target.__USAGE_FORMAT_CALLS__ = 0;
    const descriptor = Object.getOwnPropertyDescriptor(Intl.DateTimeFormat.prototype, "format")!;
    Object.defineProperty(Intl.DateTimeFormat.prototype, "format", {
      ...descriptor,
      get() {
        const format = descriptor.get!.call(this) as (date: Date) => string;
        return (date: Date) => {
          target.__USAGE_FORMAT_CALLS__! += 1;
          return format(date);
        };
      },
    });
    const invoke = target.__TAURI_INTERNALS__.invoke.bind(target.__TAURI_INTERNALS__);
    target.__TAURI_INTERNALS__.invoke = async (command, args, options) => {
      const result = await invoke(command, args, options);
      if (command !== "get_local_usage_page") return result;
      const updated = structuredClone(result) as { events: Array<{ latencyMs: number }> };
      updated.events[0]!.latencyMs = 16_157;
      return new Promise((resolve) => { target.__RELEASE_USAGE_REFRESH__ = () => resolve(updated); });
    };
  });
  for (let index = 0; index < 8; index += 1) {
    await emitTauriEvent(page, "zenith-usage-recorded", null);
    await emitTauriEvent(page, "zenith-state-changed", null);
  }
  await page.clock.fastForward(500);
  await expect.poll(() => usageReads(page)).toBe(before + 1);
  await expect(page.locator(".usage-page")).toHaveAttribute("aria-busy", "false");
  await expect(page.locator(".usage-request-table tbody tr")).toHaveCount(1);
  await expect(page.getByRole("button", { name: "More filters", exact: true })).toBeEnabled();
  expect(await page.evaluate(() => (window as unknown as { __USAGE_FORMAT_CALLS__: number }).__USAGE_FORMAT_CALLS__)).toBe(0);
  await page.evaluate(() => (window as unknown as { __RELEASE_USAGE_REFRESH__: () => void }).__RELEASE_USAGE_REFRESH__());
  await expect(page.locator('.usage-request-table td[data-column="timing"]')).toHaveText("128 ms / 16.2 s");
  await page.clock.fastForward(5_000);
  expect(await usageReads(page)).toBe(before + 1);
  const formatted = await page.evaluate(() => (window as unknown as { __USAGE_FORMAT_CALLS__: number }).__USAGE_FORMAT_CALLS__);
  await emitTauriEvent(page, "zenith-usage-recorded", null);
  await page.clock.fastForward(500);
  await expect.poll(() => usageReads(page)).toBe(before + 2);
  await page.evaluate(() => (window as unknown as { __RELEASE_USAGE_REFRESH__: () => void }).__RELEASE_USAGE_REFRESH__());
  await page.clock.runFor(100);
  expect(await page.evaluate(() => (window as unknown as { __USAGE_FORMAT_CALLS__: number }).__USAGE_FORMAT_CALLS__)).toBe(formatted);
});

async function holdNextUsageRead(page: Page, fail = false) {
  await page.evaluate((fail) => {
    const target = window as unknown as {
      __TAURI_INTERNALS__: { invoke: (command: string, args?: unknown, options?: unknown) => Promise<unknown> };
      __RELEASE_SLOW_USAGE__?: () => void;
    };
    const invoke = target.__TAURI_INTERNALS__.invoke.bind(target.__TAURI_INTERNALS__);
    let reads = 0;
    target.__TAURI_INTERNALS__.invoke = async (command, args, options) => {
      const result = await invoke(command, args, options);
      if (command !== "get_local_usage_page") return result;
      reads += 1;
      const updated = structuredClone(result) as { events: Array<{ latencyMs: number }> };
      if (updated.events[0]) updated.events[0].latencyMs = reads === 1 ? 1_000 : 16_157;
      if (reads !== 1) return updated;
      return new Promise((resolve, reject) => {
        target.__RELEASE_SLOW_USAGE__ = () => fail
          ? reject(new Error("Synthetic slow usage failure")) : resolve(updated);
      });
    };
  }, fail);
}

async function releaseSlowUsage(page: Page) {
  await page.evaluate(() => (window as unknown as { __RELEASE_SLOW_USAGE__: () => void }).__RELEASE_SLOW_USAGE__());
}

test("returning to Usage shares its pending read instead of starting another", async ({ page }) => {
  await openUsage(page);
  const before = await usageReads(page);
  await holdNextUsageRead(page);
  await emitTauriEvent(page, "zenith-usage-recorded", null);
  await page.clock.fastForward(500);
  await expect.poll(() => usageReads(page)).toBe(before + 1);
  await page.getByRole("button", { name: "Settings", exact: true }).click();
  await expect(page.getByRole("heading", { name: "Settings", exact: true })).toBeVisible();
  await page.getByRole("button", { name: "Usage", exact: true }).click();
  await expect(page.locator(".usage-page")).toBeVisible();
  await page.clock.runFor(100);
  expect(await usageReads(page)).toBe(before + 1);
  await releaseSlowUsage(page);
  await expect(page.locator(".usage-page")).toHaveAttribute("aria-busy", "false");
  await expect(page.locator('.usage-request-table td[data-column="timing"]')).toHaveText(/128 ms \/ 1(?:\.0)? s/);
  await page.clock.fastForward(5_000);
  expect(await usageReads(page)).toBe(before + 1);
});

for (const fail of [false, true]) {
  test(`writes during a slow report read queue one fresh trailing read after ${fail ? "failure" : "success"}`, async ({ page }) => {
    await openUsage(page);
    const before = await usageReads(page);
    await holdNextUsageRead(page, fail);
    await emitTauriEvent(page, "zenith-usage-recorded", null);
    await page.clock.fastForward(500);
    await expect.poll(() => usageReads(page)).toBe(before + 1);
    for (let index = 0; index < 5; index += 1) {
      await emitTauriEvent(page, "zenith-usage-recorded", null);
      await page.clock.fastForward(500);
    }
    expect(await usageReads(page)).toBe(before + 1);
    await releaseSlowUsage(page);
    await expect.poll(() => usageReads(page)).toBe(before + 2);
    await expect(page.locator('.usage-request-table td[data-column="timing"]')).toHaveText("128 ms / 16.2 s");
    await page.clock.fastForward(5_000);
    expect(await usageReads(page)).toBe(before + 2);
  });
}

for (const destination of ["settings", "models"] as const) {
  test(`queued history refresh is discarded after switching to ${destination}`, async ({ page }) => {
    await openUsage(page);
    const before = await usageReads(page);
    await holdNextUsageRead(page);
    await emitTauriEvent(page, "zenith-usage-recorded", null);
    await page.clock.fastForward(500);
    await expect.poll(() => usageReads(page)).toBe(before + 1);
    await emitTauriEvent(page, "zenith-usage-recorded", null);
    await page.clock.fastForward(500);
    if (destination === "settings") {
      await page.getByRole("button", { name: "Settings", exact: true }).click();
      await expect(page.getByRole("heading", { name: "Settings", exact: true })).toBeVisible();
    } else {
      await page.getByRole("tab", { name: "Models", exact: true }).click();
      await expect(page.locator(".usage-page")).toHaveAttribute("aria-busy", "true");
    }
    expect(await usageReads(page)).toBe(before + 1);
    await releaseSlowUsage(page);
    const expectedReads = before + (destination === "models" ? 2 : 1);
    await expect.poll(() => usageReads(page)).toBe(expectedReads);
    if (destination === "models") {
      await expect(page.locator(".usage-page")).toHaveAttribute("aria-busy", "false");
    }
    await page.clock.fastForward(5_000);
    expect(await usageReads(page)).toBe(expectedReads);
  });
}
