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
