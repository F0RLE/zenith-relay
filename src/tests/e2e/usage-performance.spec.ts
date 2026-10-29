import { expect, test, type Page } from "../bun-playwright";
import { installTauriMock } from "./tauri-mock";

const searchedRequests = (page: Page) => page.evaluate(() => (window as unknown as {
  __TAURI_TEST_INVOKES__: Array<{ command: string; args: { input?: { requestIdQuery?: string } } }>;
}).__TAURI_TEST_INVOKES__
  .filter(({ command, args }) => command === "get_local_usage_page" && args.input?.requestIdQuery)
  .map(({ args }) => args.input!.requestIdQuery));

test("slow report reads coalesce intermediate filter selections instead of queuing native work", async ({ page }) => {
  await installTauriMock(page, { locale: "en" });
  await page.goto("/");
  await page.getByRole("button", { name: "Usage", exact: true }).click();
  await expect(page.locator(".usage-page")).toHaveAttribute("aria-busy", "false");
  await page.evaluate(() => {
    const target = window as unknown as {
      __TAURI_INTERNALS__: { invoke: (command: string, args?: unknown, options?: unknown) => Promise<unknown> };
      __FILTER_READS__: string[];
      __RELEASE_FILTER_READ__: () => void;
    };
    target.__FILTER_READS__ = [];
    const invoke = target.__TAURI_INTERNALS__.invoke.bind(target.__TAURI_INTERNALS__);
    let hold = true;
    target.__TAURI_INTERNALS__.invoke = async (command, args, options) => {
      const input = (args as { input?: { range?: string; includeEvents?: boolean } } | undefined)?.input;
      if (command === "get_local_usage_page" && input?.includeEvents !== false && input?.range) {
        target.__FILTER_READS__.push(input.range);
        if (hold) {
          hold = false;
          await new Promise<void>((resolve) => { target.__RELEASE_FILTER_READ__ = resolve; });
        }
      }
      return invoke(command, args, options);
    };
  });
  const choose = async (name: string) => {
    await page.locator(".usage-range-menu > button").click();
    await page.getByRole("option", { name, exact: true }).click();
  };
  await choose("24 hours");
  await expect.poll(() => page.evaluate(() => typeof (window as unknown as { __RELEASE_FILTER_READ__?: unknown }).__RELEASE_FILTER_READ__)).toBe("function");
  await choose("7 days");
  await choose("30 days");
  const reads = () => page.evaluate(() => (window as unknown as { __FILTER_READS__: string[] }).__FILTER_READS__);
  expect(await reads()).toEqual(["daily"]);
  await page.evaluate(() => (window as unknown as { __RELEASE_FILTER_READ__: () => void }).__RELEASE_FILTER_READ__());
  await expect(page.locator(".usage-page")).toHaveAttribute("aria-busy", "false");
  expect(await reads()).toEqual(["daily", "monthly"]);
});

test("typing a request ID issues one settled search instead of one query per character", async ({ page }) => {
  await page.clock.install();
  await page.addInitScript(() => {
    const state = window as unknown as { __USAGE_TIME_FORMAT_CALLS__: number };
    state.__USAGE_TIME_FORMAT_CALLS__ = 0;
    const track = (formatter: Intl.DateTimeFormat) => new Proxy(formatter, {
      get(target, property) {
        const value = Reflect.get(target, property, target);
        if (typeof value !== "function") return value;
        if (property !== "format") return value.bind(target);
        return (...args: Parameters<Intl.DateTimeFormat["format"]>) => {
          state.__USAGE_TIME_FORMAT_CALLS__ += 1;
          return value(...args);
        };
      },
    });
    Intl.DateTimeFormat = new Proxy(Intl.DateTimeFormat, {
      construct(target, args) { return track(Reflect.construct(target, args)); },
      apply(target, receiver, args) { return track(Reflect.apply(target, receiver, args)); },
    });
  });
  await installTauriMock(page, { locale: "en" });
  await page.goto("/");
  await page.getByRole("button", { name: "Usage", exact: true }).click();
  await expect(page.locator(".usage-page")).toHaveAttribute("aria-busy", "false");
  await page.getByRole("button", { name: "More filters", exact: true }).click();
  const input = page.getByRole("textbox", { name: "Request ID", exact: true });
  const formatCalls = await page.evaluate(() => (window as unknown as { __USAGE_TIME_FORMAT_CALLS__: number }).__USAGE_TIME_FORMAT_CALLS__);
  expect(formatCalls).toBeGreaterThan(0);
  await input.pressSequentially("req_synthetic_local", { delay: 5 });
  await expect(input).toHaveValue("req_synthetic_local");
  expect(await page.evaluate(() => (window as unknown as { __USAGE_TIME_FORMAT_CALLS__: number }).__USAGE_TIME_FORMAT_CALLS__)).toBe(formatCalls);
  await page.clock.fastForward(300);
  await expect.poll(() => searchedRequests(page)).toEqual(["req_synthetic_local"]);
  await expect(page.locator(".usage-page")).toHaveAttribute("aria-busy", "false");
  await expect(page.locator(".usage-request-table tbody tr")).toHaveCount(1);
  await page.clock.fastForward(500);
  expect(await searchedRequests(page)).toEqual(["req_synthetic_local"]);
});

test("clearing unfinished search cancels its timer and keeps the current report", async ({ page }) => {
  await page.clock.install();
  await installTauriMock(page, { locale: "en" });
  await page.goto("/");
  await page.getByRole("button", { name: "Usage", exact: true }).click();
  await expect(page.locator(".usage-page")).toHaveAttribute("aria-busy", "false");
  await page.getByRole("button", { name: "More filters", exact: true }).click();
  const input = page.getByRole("textbox", { name: "Request ID", exact: true });
  await input.fill("unfinished");
  await input.fill("");
  await page.clock.fastForward(1_000);
  expect(await searchedRequests(page)).toEqual([]);
  await expect(page.locator(".usage-page")).toHaveAttribute("aria-busy", "false");
  await expect(page.locator(".usage-request-table tbody tr")).toHaveCount(1);
});

test("leaving the request tab cancels a pending search without filtering aggregates", async ({ page }) => {
  await page.clock.install();
  await installTauriMock(page, { locale: "en" });
  await page.goto("/");
  await page.getByRole("button", { name: "Usage", exact: true }).click();
  await expect(page.locator(".usage-page")).toHaveAttribute("aria-busy", "false");
  await page.getByRole("button", { name: "More filters", exact: true }).click();
  await page.getByRole("textbox", { name: "Request ID", exact: true }).fill("req_synthetic_local");
  await page.getByRole("tab", { name: "Models", exact: true }).click();
  await page.clock.fastForward(1_000);
  expect(await searchedRequests(page)).toEqual([]);
  await expect(page.locator(".usage-page")).toHaveAttribute("aria-busy", "false");
  await page.getByRole("tab", { name: "Sends", exact: true }).click();
  await page.clock.fastForward(300);
  await expect.poll(() => searchedRequests(page)).toEqual(["req_synthetic_local"]);
});
