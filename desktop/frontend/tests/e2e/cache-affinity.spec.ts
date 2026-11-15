import { expect, test } from "../bun-playwright";
import type { CacheAffinityDiagnostics } from "../../src/features/relay/api/types";
import { installTauriMock } from "./tauri-mock";

function affinity(): CacheAffinityDiagnostics {
  return {
    selection: "other", reason: "quota_headroom",
    preferred: { quotaRemainingBasisPoints: 5700, providerCreditsMicroUnits: null, providerCreditsUnlimited: false, inFlight: 1, effectiveCapacity: 4 },
    selected: { quotaRemainingBasisPoints: 6300, providerCreditsMicroUnits: null, providerCreditsUnlimited: false, inFlight: 0, effectiveCapacity: 4 },
    bestQuotaRemainingBasisPoints: 6300, bestProviderCreditsMicroUnits: null, bestProviderCreditsUnlimited: false,
    quotaSwitchMarginBasisPoints: 500, providerCreditSwitchMarginMicroUnits: 15_000_000, loadAllowanceRequests: 1,
  };
}

for (const locale of ["ru", "en"] as const) {
  test(`Cache affinity ${locale} fits existing narrow technical details`, async ({ page }) => {
    await page.setViewportSize({ width: 390, height: 760 });
    await installTauriMock(page, { mode: "local", locale, populated: true, diagnosticDebug: true, usageCacheAffinity: affinity() });
    await page.goto("/");
    await page.getByRole("button", { name: locale === "ru" ? "Использование" : "Usage", exact: true }).click();
    await page.locator(".usage-request-table tbody tr button").first().click();
    const dialog = page.getByRole("dialog");
    await expect(dialog.locator(".request-cache-affinity")).toHaveCount(0);
    await dialog.locator(".request-technical-details > summary").click();
    const fields = dialog.locator(".request-cache-affinity");
    await expect(fields).toHaveCount(4);
    await expect(fields.nth(0)).toContainText(locale === "ru" ? "Выбор по свежей квоте" : "Fresh quota preference");
    await expect(fields.nth(1)).toContainText("57% → 63%");
    await expect(fields.nth(2)).toContainText("1/4 → 0/4");
    await expect(fields.nth(3)).toContainText(locale === "ru" ? "порог: 5 п.п." : "margin: 5 pp");
    expect(await dialog.locator(".request-cache-affinity, .request-cache-affinity dd").evaluateAll(elements =>
      elements.every(element => element.scrollWidth <= element.clientWidth + 1))).toBe(true);
    await expect(fields.locator("p, .form-note")).toHaveCount(0);
    if (locale === "ru") {
      await fields.first().scrollIntoViewIfNeeded();
      await page.screenshot({ path: "output/playwright/cache-affinity-ru-390.png" });
    }
  });
}

test("Unknown quota stays unknown and a warm fallback is not labelled a cache hit", async ({ page }) => {
  const diagnostics = affinity();
  diagnostics.selection = "warm_alternative";
  diagnostics.reason = "owner_unavailable";
  diagnostics.ownerBlockReason = "capacity_busy";
  diagnostics.preferred!.quotaRemainingBasisPoints = null;
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, diagnosticDebug: true, usageCacheAffinity: diagnostics });
  await page.goto("/");
  await page.getByRole("button", { name: "Usage", exact: true }).click();
  await page.locator(".usage-request-table tbody tr button").first().click();
  await page.locator(".request-technical-details > summary").click();
  const fields = page.getByRole("dialog").locator(".request-cache-affinity");
  await expect(fields.nth(0)).toContainText("Preferred member unavailable · Capacity occupied · Previously used alternative");
  await expect(fields.nth(1)).toContainText("— → 63%");
  await expect(fields.nth(0)).not.toContainText("Cache hit");
});

test("Old rows omit new affinity details", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, diagnosticDebug: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Usage", exact: true }).click();
  await page.locator(".usage-request-table tbody tr button").first().click();
  await page.locator(".request-technical-details > summary").click();
  await expect(page.locator(".request-cache-affinity")).toHaveCount(0);
});

test("Retained preferred member shows only the decision", async ({ page }) => {
  const diagnostics = affinity();
  diagnostics.selection = "preferred";
  diagnostics.reason = "retained";
  diagnostics.preferred = diagnostics.selected;
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, diagnosticDebug: true, usageCacheAffinity: diagnostics });
  await page.goto("/");
  await page.getByRole("button", { name: "Usage", exact: true }).click();
  await page.locator(".usage-request-table tbody tr button").first().click();
  await page.locator(".request-technical-details > summary").click();
  const fields = page.getByRole("dialog").locator(".request-cache-affinity");
  await expect(fields).toHaveCount(1);
  await expect(fields).toContainText("Preferred member retained");
});
