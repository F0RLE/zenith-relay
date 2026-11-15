import { expect, test } from "../bun-playwright";
import { emitTauriEvent, installTauriMock } from "./tauri-mock";

test("hidden activity is coalesced without rendering and reconciles on return", async ({ page }) => {
  await installTauriMock(page, { locale: "en", accountCount: 1, quotaAvailable: true, usagePresent: false });
  await page.goto("/");
  await page.getByRole("button", { name: "Pool", exact: true }).click();
  const summary = page.locator(".pool-controls");
  const activeRequests = summary.locator("[data-active-request-count]");
  const activity = (candidateId: string, revision: number, count: number) => emitTauriEvent(page, "zenith-runtime-activity", {
    runtimeId: 1, revision, candidateId, inFlight: count, activeRequestCount: count,
    activeModels: count ? [{ model: "gpt-5.4", requestCount: count }] : [],
  });
  await activity("account_synthetic", 1, 1);
  await expect(activeRequests).toHaveAttribute("data-active-request-count", "1");
  await page.evaluate(() => {
    Object.defineProperty(document, "visibilityState", { configurable: true, value: "hidden" });
    document.dispatchEvent(new Event("visibilitychange"));
  });
  await activity("account_synthetic", 3, 0);
  await activity("account_synthetic", 2, 1);
  for (let revision = 4; revision <= 40; revision += 1) {
    await activity("source_synthetic", revision, 2);
  }
  await page.waitForTimeout(100);
  await expect(activeRequests).toHaveAttribute("data-active-request-count", "1");
  await page.evaluate(() => {
    Object.defineProperty(document, "visibilityState", { configurable: true, value: "visible" });
    document.dispatchEvent(new Event("visibilitychange"));
  });
  await expect(activeRequests).toHaveAttribute("data-active-request-count", "2");
  await expect(summary).toContainText("Active now: Example compatible API");
  await expect(page.locator('[data-member-label="Personal Plus"]')).toHaveAttribute("data-current", "false");
});

test("countdowns pause in a hidden window and catch up even after expiry", async ({ page }) => {
  await page.clock.install();
  await installTauriMock(page, {
    locale: "en", accountCount: 1, quotaAvailable: true, subscriptionExpiresInMs: 30_000,
  });
  await page.goto("/");
  await page.getByRole("button", { name: "Pool", exact: true }).click();
  const subscription = page.locator(".pool-member-card .account-subscription-line").first();
  const countdown = subscription.locator(".account-subscription-countdown");
  await expect(countdown).toBeVisible();
  const initial = await countdown.textContent();
  await page.clock.runFor(2_000);
  await expect(countdown).not.toHaveText(initial!);
  const beforeHide = await countdown.textContent();
  await page.evaluate(() => {
    Object.defineProperty(document, "visibilityState", { configurable: true, value: "hidden" });
    document.dispatchEvent(new Event("visibilitychange"));
  });
  await page.clock.runFor(35_000);
  expect(await countdown.textContent()).toBe(beforeHide);
  await page.evaluate(() => {
    Object.defineProperty(document, "visibilityState", { configurable: true, value: "visible" });
    document.dispatchEvent(new Event("visibilitychange"));
  });
  await expect(subscription).toHaveClass(/expired/);
  await expect(countdown).toHaveText("0 s");
});

test("countdown ticks reuse date formatters across account cards", async ({ page }) => {
  await page.clock.install();
  await page.addInitScript(() => {
    const state = window as unknown as { __DATE_FORMATTERS__: number };
    state.__DATE_FORMATTERS__ = 0;
    Intl.DateTimeFormat = new Proxy(Intl.DateTimeFormat, {
      construct(target, args) {
        state.__DATE_FORMATTERS__ += 1;
        return Reflect.construct(target, args);
      },
    });
  });
  await installTauriMock(page, {
    locale: "en", accountCount: 8, quotaAvailable: true, subscriptionExpiresInMs: 60_000,
  });
  await page.goto("/");
  await page.getByRole("button", { name: "Pool", exact: true }).click();
  const countdowns = page.locator(".pool-member-card .account-subscription-countdown");
  await expect(countdowns).toHaveCount(4);
  const initial = await countdowns.first().textContent();
  const constructions = () => page.evaluate(() => (window as unknown as { __DATE_FORMATTERS__: number }).__DATE_FORMATTERS__);
  const before = await constructions();
  expect(before).toBeGreaterThan(0);
  await page.clock.runFor(2_000);
  await expect(countdowns.first()).not.toHaveText(initial!);
  expect(await constructions()).toBe(before);
});

test("request cache countdown pauses while hidden and catches up after expiry", async ({ page }) => {
  await page.clock.install();
  await installTauriMock(page, {
    mode: "local", locale: "en", populated: true,
    usageCache: { cachedInputTokens: 10, cacheWriteInputTokens: 0, cacheWriteTtl: "2m" },
  });
  await page.goto("/");
  await page.getByRole("button", { name: "Usage", exact: true }).click();
  await page.getByRole("button", { name: "Request details: req_synthetic_local" }).click();
  const retention = page.locator(".request-cache-retention dd");
  await expect(retention).toHaveText("≈ 2 min left");
  await page.clock.runFor(46_000);
  await expect(retention).toHaveText("≈ 1 min left");
  await page.evaluate(() => {
    Object.defineProperty(document, "visibilityState", { configurable: true, value: "hidden" });
    document.dispatchEvent(new Event("visibilitychange"));
  });
  await page.clock.runFor(180_000);
  expect(await retention.textContent()).toBe("≈ 1 min left");
  await page.evaluate(() => {
    Object.defineProperty(document, "visibilityState", { configurable: true, value: "visible" });
    document.dispatchEvent(new Event("visibilitychange"));
  });
  await expect(retention).toHaveText("estimated window elapsed");
  await page.getByRole("dialog").getByRole("button", { name: "Close", exact: true }).click();
  await page.clock.runFor(30_000);
  await expect(page.getByRole("dialog")).toHaveCount(0);
});
