import { expect, test, type Page } from "../bun-playwright";
import { emitTauriEvent, installTauriMock } from "./tauri-mock";

type UsageLoadTestWindow = Window & {
  __TAURI_INTERNALS__: { invoke: (command: string, args?: unknown, options?: unknown) => Promise<unknown> };
  __USAGE_LOAD_TEST__: { pending: number; release: (fail: boolean) => void };
};

// Hold actual mock invocations, not an arbitrary timer: inspect each intermediate
// UI state, then release success or failure without touching a real database.
async function holdUsageReads(page: Page) {
  await page.addInitScript(() => {
    const target = window as unknown as UsageLoadTestWindow;
    const invoke = target.__TAURI_INTERNALS__.invoke.bind(target.__TAURI_INTERNALS__);
    let waiting: Array<(fail: boolean) => void> = [];
    const control = target.__USAGE_LOAD_TEST__ = {
      pending: 0,
      release(fail: boolean) {
        const current = waiting;
        waiting = [];
        control.pending = 0;
        current.forEach((settle) => settle(fail));
      },
    };
    target.__TAURI_INTERNALS__.invoke = (command, args, options) => {
      if (command !== "get_local_usage_page") return invoke(command, args, options);
      control.pending += 1;
      return new Promise((resolve, reject) => {
        waiting.push((fail) => {
          if (fail) reject({ code: "io", message: "Synthetic report failure" });
          else void invoke(command, args, options).then(resolve, reject);
        });
      });
    };
  });
}

async function releaseUsage(page: Page, fail = false) {
  await expect.poll(() => page.evaluate(() => (window as unknown as UsageLoadTestWindow).__USAGE_LOAD_TEST__.pending)).toBeGreaterThan(0);
  await page.evaluate((fail) => (window as unknown as UsageLoadTestWindow).__USAGE_LOAD_TEST__.release(fail), fail);
}

async function openUsage(page: Page, usagePresent = true) {
  await installTauriMock(page, { locale: "en", usagePresent });
  await holdUsageReads(page);
  await page.goto("/");
  await page.getByRole("button", { name: "Usage", exact: true }).click();
  await expect(page.locator(".usage-page")).toBeVisible();
  await expect(page.locator(".usage-page .data-placeholder")).toHaveCount(0);
  await expect(page.locator(".usage-page")).not.toContainText("Loading");
}

async function releaseWithStatistics(page: Page) {
  await releaseUsage(page);
  await expect(page.locator(".usage-page")).toHaveAttribute("aria-busy", "false");
  await expect(page.locator(".usage-overview")).toBeVisible();
}

test("initial Usage waits for its selected page without flashing zero or no matches", async ({ page }) => {
  await openUsage(page);
  await expect(page.locator(".usage-page")).toHaveAttribute("aria-busy", "true");
  await expect(page.locator(".usage-overview")).toHaveCount(0);
  await expect(page.getByText("No matching results", { exact: true })).toHaveCount(0);
  await page.screenshot({ path: "output/playwright/usage-initial-pending.png" });
  await releaseUsage(page);
  await expect(page.locator(".usage-request-table tbody tr")).toHaveCount(1);
  await expect(page.locator(".usage-overview")).toBeVisible();
  await expect(page.locator(".usage-page")).toHaveAttribute("aria-busy", "false");
  await expect(page.locator(".usage-page .data-placeholder")).toHaveCount(0);
});

test("Usage remains navigable while history and its page-open metric are pending", async ({ page }) => {
  await installTauriMock(page, { locale: "en" });
  await holdUsageReads(page);
  await page.addInitScript(() => {
    const target = window as unknown as UsageLoadTestWindow & { __RELEASE_PAGE_METRIC__?: () => void };
    const invoke = target.__TAURI_INTERNALS__.invoke.bind(target.__TAURI_INTERNALS__);
    target.__TAURI_INTERNALS__.invoke = async (command, args, options) => {
      const metric = args as { name?: string; context?: string } | undefined;
      if (command === "record_local_performance_sample" && metric?.name === "page_open" && metric.context === "usage") {
        await new Promise<void>((resolve) => { target.__RELEASE_PAGE_METRIC__ = resolve; });
      }
      return invoke(command, args, options);
    };
  });
  await page.goto("/");
  await page.getByRole("button", { name: "Usage", exact: true }).click();
  await expect(page.locator(".usage-page")).toHaveAttribute("aria-busy", "true");
  await expect.poll(() => page.evaluate(() => typeof (window as unknown as { __RELEASE_PAGE_METRIC__?: unknown }).__RELEASE_PAGE_METRIC__)).toBe("function");
  await page.getByRole("button", { name: "More filters", exact: true }).click();
  await expect(page.getByRole("textbox", { name: "Request ID", exact: true })).toBeVisible();
  await page.getByRole("button", { name: "Settings", exact: true }).click();
  await expect(page.locator(".settings-page")).toBeVisible();
  await releaseUsage(page);
  await page.evaluate(() => (window as unknown as { __RELEASE_PAGE_METRIC__: () => void }).__RELEASE_PAGE_METRIC__());
  await expect(page.locator(".relay-shell")).toHaveAttribute("data-page", "settings");
  await expect(page.locator(".usage-page")).toHaveCount(0);
});

test("a history event retains displayed data through pending and failure, then retries", async ({ page }) => {
  await openUsage(page);
  await releaseWithStatistics(page);
  const summary = page.locator(".usage-overview");
  await expect(summary).toBeVisible();
  const before = await summary.textContent();
  await expect(page.locator(".usage-page").getByRole("button", { name: "Refresh", exact: true })).toHaveCount(0);
  await emitTauriEvent(page, "zenith-usage-recorded", null);
  await expect.poll(() => page.evaluate(() => (window as UsageLoadTestWindow).__USAGE_LOAD_TEST__.pending)).toBeGreaterThan(0);
  await expect(page.locator(".usage-page")).toHaveAttribute("aria-busy", "false");
  await expect(summary).toHaveText(before!);
  await expect(page.locator(".usage-request-table tbody tr")).toHaveCount(1);
  await expect(page.locator(".usage-page .data-placeholder")).toHaveCount(0);
  await releaseUsage(page, true);
  await expect(page.locator(".usage-page [role=alert]")).toBeVisible();
  await expect(summary).toHaveText(before!);
  await page.locator(".usage-page [role=alert]").getByRole("button", { name: "Retry", exact: true }).click();
  await releaseUsage(page);
  await expect(page.locator(".usage-page [role=alert]")).toHaveCount(0);
  await expect(page.locator(".usage-page")).toHaveAttribute("aria-busy", "false");
});

test("uncached tabs do not present another query's totals or false empty results", async ({ page }) => {
  await openUsage(page);
  await releaseWithStatistics(page);
  await expect(page.locator(".usage-overview")).toBeVisible();
  for (const name of ["Models", "Pool members", "Errors"]) {
    await page.getByRole("tab", { name, exact: true }).click();
    await expect(page.locator(".usage-page .data-placeholder")).toHaveCount(0);
    await expect(page.locator(".usage-overview")).toHaveCount(0);
    await expect(page.getByText("No matching results", { exact: true })).toHaveCount(0);
    await releaseUsage(page);
    await expect(page.locator(".usage-overview")).toBeVisible();
    await expect(page.locator(".usage-page")).toHaveAttribute("aria-busy", "false");
  }
});

test("a confirmed empty report still shows genuine zero and no matches", async ({ page }) => {
  await openUsage(page, false);
  await expect(page.getByText("No matching results", { exact: true })).toHaveCount(0);
  await releaseWithStatistics(page);
  await expect(page.getByText("No matching results", { exact: true })).toBeVisible();
  await expect(page.locator(".usage-overview")).toBeVisible();
  await expect(page.locator(".usage-page")).toHaveAttribute("aria-busy", "false");
});

test("returning to the last selected Usage query never inserts a placeholder frame", async ({ page }) => {
  await openUsage(page);
  await releaseWithStatistics(page);
  await expect(page.locator(".usage-request-table tbody tr")).toHaveCount(1);
  const summary = await page.locator(".usage-overview").textContent();
  await page.evaluate(() => {
    const target = window as unknown as { __PLACEHOLDER_FRAMES__: number };
    target.__PLACEHOLDER_FRAMES__ = 0;
    new MutationObserver((changes) => {
      for (const change of changes) for (const node of change.addedNodes) {
        if (node instanceof Element && (node.matches(".data-placeholder") || node.querySelector(".data-placeholder"))) target.__PLACEHOLDER_FRAMES__ += 1;
      }
    }).observe(document.querySelector(".relay-content")!, { childList: true, subtree: true });
  });
  await page.getByRole("button", { name: "Pool", exact: true }).click();
  await expect(page.locator(".pool-controls")).toBeVisible();
  // Ignore the first lazy import of Pool; only the cached Usage return matters.
  await page.evaluate(() => { (window as unknown as { __PLACEHOLDER_FRAMES__: number }).__PLACEHOLDER_FRAMES__ = 0; });
  await page.getByRole("button", { name: "Usage", exact: true }).click();
  await expect(page.locator(".usage-overview")).toHaveText(summary!);
  await expect(page.locator(".usage-request-table tbody tr")).toHaveCount(1);
  expect(await page.evaluate(() => (window as unknown as { __PLACEHOLDER_FRAMES__: number }).__PLACEHOLDER_FRAMES__)).toBe(0);
});

test("initial read failure is not an empty report and retry can recover", async ({ page }) => {
  await openUsage(page);
  await releaseUsage(page, true);
  await expect(page.locator(".usage-page [role=alert]")).toBeVisible();
  await expect(page.locator(".usage-overview")).toHaveCount(0);
  await expect(page.getByText("No matching results", { exact: true })).toHaveCount(0);
  await expect(page.locator(".usage-page")).toHaveAttribute("aria-busy", "false");
  await page.locator(".usage-page [role=alert]").getByRole("button", { name: "Retry", exact: true }).click();
  await expect(page.locator(".usage-page .data-placeholder")).toHaveCount(0);
  await releaseUsage(page);
  await expect(page.locator(".usage-request-table tbody tr")).toHaveCount(1);
  await expect(page.locator(".usage-page [role=alert]")).toHaveCount(0);
});

test("Overview also distinguishes missing analytics from measured zero", async ({ page }) => {
  await installTauriMock(page, { locale: "en" });
  await holdUsageReads(page);
  await page.goto("/");
  await expect(page.locator(".overview-analytics .data-placeholder")).toHaveCount(0);
  const counters = page.locator(".overview-metrics strong");
  await expect(counters.nth(0)).toHaveText("—");
  await expect(counters.nth(3)).toHaveText("—");
  await expect(page.locator(".overview-chart:not(.weekly-capacity)")).toHaveCount(0);
  await expect(page.locator(".overview-chart.weekly-capacity")).toBeVisible();
  await releaseUsage(page, true);
  await expect(page.locator(".overview-analytics [role=alert]")).toBeVisible();
  await expect(counters.nth(0)).toHaveText("—");
  await expect(page.locator(".overview-chart:not(.weekly-capacity)")).toHaveCount(0);
  await expect(page.locator(".overview-chart.weekly-capacity")).toBeVisible();
  await page.getByRole("tab", { name: "Week", exact: true }).click();
  await expect(page.locator(".overview-analytics .data-placeholder")).toHaveCount(0);
  await releaseUsage(page);
  await expect(page.locator(".overview-chart-stack")).toBeVisible();
  await expect(counters.nth(0)).toHaveText("1");
});
