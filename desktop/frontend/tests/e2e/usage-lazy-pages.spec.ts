import { expect, test, type Page } from "../bun-playwright";
import { emitTauriEvent, installTauriMock } from "./tauri-mock";

const history = Array.from({ length: 120 }, (_, index) => ({
  wireApi: "responses" as const, transport: "http" as const, errorCategory: null,
  requestedModel: `synthetic-lazy-${index}`, resolvedModel: `synthetic-lazy-${index}`,
}));

const reads = (page: Page) => page.evaluate(() => (window as unknown as {
  __TAURI_TEST_INVOKES__: Array<{ command: string; args: { includeReport?: boolean; input?: { page?: number; requestId?: string; includeEvents?: boolean; includeFilterOptions?: boolean; includeModels?: boolean; includePoolMembers?: boolean } } }>;
}).__TAURI_TEST_INVOKES__.filter(({ command }) => command === "get_local_usage_page").map(({ args }) => args));

test("only the selected page is retained and other pages reload on navigation", async ({ page }) => {
  await page.clock.install();
  await installTauriMock(page, { locale: "en", usageFilterHistory: history });
  await page.goto("/");
  await page.getByRole("button", { name: "Usage", exact: true }).click();
  await expect(page.locator(".usage-page")).toHaveAttribute("aria-busy", "false");
  await expect(page.locator(".usage-request-table tbody tr")).toHaveCount(50);
  await expect(page.locator(".usage-overview")).toBeVisible();
  await expect(page.getByRole("button", { name: "Period statistics", exact: true })).toHaveCount(0);
  const first = await page.locator(".usage-request-table tbody tr").first().textContent();
  const eventReads = async () => (await reads(page)).filter(({ input }) => input?.includeEvents !== false && !input?.requestId);
  expect((await eventReads()).map(({ input }) => input?.page)).toEqual([1]);
  expect((await eventReads())[0]).toMatchObject({ includeReport: true, input: { includeFilterOptions: false, includeModels: false, includePoolMembers: false } });
  await page.clock.fastForward(2_000);
  expect((await eventReads()).map(({ input }) => input?.page)).toEqual([1]);
  await page.locator(".usage-pagination").getByRole("button", { name: "Page 2", exact: true }).click();
  await expect(page.locator(".usage-pagination [aria-current=page]")).toHaveText("2");
  await expect(page.locator(".usage-request-table tbody tr")).toHaveCount(50);
  expect((await eventReads()).map(({ input }) => input?.page)).toEqual([1, 2]);
  await page.clock.fastForward(2_000);
  await page.locator(".usage-pagination").getByRole("button", { name: "Page 1", exact: true }).click();
  await expect(page.locator(".usage-request-table tbody tr").first()).toHaveText(first!);
  await expect(page.locator(".usage-page")).toHaveAttribute("aria-busy", "false");
  expect((await eventReads()).map(({ input }) => input?.page)).toEqual([1, 2, 1]);
  await page.clock.fastForward(2_000);
  expect((await eventReads()).map(({ input }) => input?.page)).toEqual([1, 2, 1]);
  await page.locator(".usage-pagination").getByRole("button", { name: "Page 2", exact: true }).click();
  await expect(page.locator(".usage-page")).toHaveAttribute("aria-busy", "false");
  expect((await eventReads()).map(({ input }) => input?.page)).toEqual([1, 2, 1, 2]);
  await page.locator(".usage-pagination").getByRole("button", { name: "Page 3", exact: true }).click();
  await expect(page.locator(".usage-request-table tbody tr")).toHaveCount(20);
  await expect(page.locator(".usage-page")).toHaveAttribute("aria-busy", "false");
  expect((await eventReads()).map(({ input }) => input?.page)).toEqual([1, 2, 1, 2, 3]);
  await page.screenshot({ path: "output/playwright/usage-selected-page.png" });
});

test("totals accompany the selected page while history filter choices wait for explicit intent", async ({ page }) => {
  await installTauriMock(page, { locale: "en", usageFilterHistory: history });
  await page.goto("/");
  await page.getByRole("button", { name: "Usage", exact: true }).click();
  await expect(page.locator(".usage-page")).toHaveAttribute("aria-busy", "false");
  expect((await reads(page)).filter(({ input }) => input?.includeFilterOptions === true)).toHaveLength(0);
  await page.locator(".usage-model-menu > button").focus();
  await page.keyboard.press("ArrowDown");
  await expect(page.getByRole("option", { name: "synthetic-lazy-119", exact: true })).toBeVisible();
  const choices = (await reads(page)).filter(({ input }) => input?.includeFilterOptions === true);
  expect(choices).toHaveLength(1);
  expect(choices[0]).toMatchObject({ includeReport: false, input: { includeEvents: false, includeModels: false, includePoolMembers: false } });
  await page.keyboard.press("Escape");
  await expect(page.locator(".usage-overview")).toBeVisible();
  const statistics = (await reads(page)).filter(({ includeReport }) => includeReport === true);
  expect(statistics).toHaveLength(1);
  expect(statistics[0]?.input?.includeFilterOptions).toBe(false);
  await page.getByRole("tab", { name: "Models", exact: true }).click();
  await expect(page.locator(".usage-page")).toHaveAttribute("aria-busy", "false");
  const aggregate = (await reads(page)).at(-1)!;
  expect(aggregate).toMatchObject({ includeReport: true, input: { includeEvents: false, includeModels: true, includePoolMembers: false, includeFilterOptions: false } });
});

test("history changes invalidate cached pages without reading inactive or hidden views", async ({ page }) => {
  await page.clock.install();
  await installTauriMock(page, { locale: "en", usageFilterHistory: history });
  await page.goto("/");
  await page.getByRole("button", { name: "Usage", exact: true }).click();
  await expect(page.locator(".usage-page")).toHaveAttribute("aria-busy", "false");
  const eventReads = async () => (await reads(page)).filter(({ input }) => input?.includeEvents !== false && !input?.requestId);
  await page.locator(".usage-pagination").getByRole("button", { name: "Page 2", exact: true }).click();
  await expect(page.locator(".usage-pagination [aria-current=page]")).toHaveText("2");
  expect((await eventReads()).map(({ input }) => input?.page)).toEqual([1, 2]);
  await page.getByRole("button", { name: "Pool", exact: true }).click();
  await emitTauriEvent(page, "zenith-usage-recorded", null);
  await page.clock.fastForward(1_500);
  expect((await eventReads()).map(({ input }) => input?.page)).toEqual([1, 2]);
  await page.getByRole("button", { name: "Usage", exact: true }).click();
  await expect(page.locator(".usage-page")).toHaveAttribute("aria-busy", "false");
  expect((await eventReads()).map(({ input }) => input?.page)).toEqual([1, 2, 1]);
  await page.evaluate(() => {
    Object.defineProperty(document, "visibilityState", { configurable: true, get: () => "hidden" });
    document.dispatchEvent(new Event("visibilitychange"));
  });
  await emitTauriEvent(page, "zenith-usage-recorded", null);
  await page.clock.fastForward(1_500);
  expect((await eventReads()).map(({ input }) => input?.page)).toEqual([1, 2, 1]);
  await page.evaluate(() => {
    Object.defineProperty(document, "visibilityState", { configurable: true, get: () => "visible" });
    document.dispatchEvent(new Event("visibilitychange"));
  });
  await page.clock.fastForward(1_500);
  await expect.poll(async () => (await eventReads()).map(({ input }) => input?.page)).toEqual([1, 2, 1, 1]);
});

test("writes during a pending page do not discard its display before the trailing refresh", async ({ page }) => {
  await page.clock.install();
  await installTauriMock(page, { locale: "en", usageFilterHistory: history });
  await page.addInitScript(() => {
    const target = window as unknown as {
      __TAURI_INTERNALS__: { invoke: (command: string, args?: unknown, options?: unknown) => Promise<unknown> };
      __RELEASE_USAGE_PAGE__?: () => void;
    };
    const invoke = target.__TAURI_INTERNALS__.invoke.bind(target.__TAURI_INTERNALS__);
    let hold = true;
    target.__TAURI_INTERNALS__.invoke = async (command, args, options) => {
      const result = await invoke(command, args, options);
      const input = (args as { input?: { includeEvents?: boolean; requestId?: string } } | undefined)?.input;
      if (command !== "get_local_usage_page" || !hold || input?.includeEvents === false || input?.requestId) return result;
      hold = false;
      return new Promise((resolve) => { target.__RELEASE_USAGE_PAGE__ = () => resolve(result); });
    };
  });
  await page.goto("/");
  await page.getByRole("button", { name: "Usage", exact: true }).click();
  const eventReads = async () => (await reads(page)).filter(({ input }) => input?.includeEvents !== false && !input?.requestId);
  await expect.poll(() => page.evaluate(() => typeof (window as unknown as { __RELEASE_USAGE_PAGE__?: () => void }).__RELEASE_USAGE_PAGE__)).toBe("function");
  await page.clock.pauseAt(new Date(Date.now() + 1_000));
  await emitTauriEvent(page, "zenith-usage-recorded", null);
  await page.evaluate(() => (window as unknown as { __RELEASE_USAGE_PAGE__: () => void }).__RELEASE_USAGE_PAGE__());
  await expect(page.locator(".usage-page")).toHaveAttribute("aria-busy", "false");
  await expect(page.locator(".usage-request-table tbody tr")).toHaveCount(50);
  expect((await eventReads()).map(({ input }) => input?.page)).toEqual([1]);
  await page.clock.fastForward(1_500);
  await expect.poll(async () => (await eventReads()).map(({ input }) => input?.page)).toEqual([1, 1]);
  await expect(page.locator(".usage-page")).toHaveAttribute("aria-busy", "false");
});
