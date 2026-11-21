import { expect, test, type Page } from "../bun-playwright";
import type { RuntimeSnapshot } from "../../src/features/relay/api/types";
import { installTauriMock } from "./tauri-mock";

async function openSummary(page: Page, auth: "active" | "missing-secret" | "reauth" | "recovery") {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, usageActive: false, accountCalibrationMissing: true });
  await page.addInitScript((auth) => {
    const native = (window as unknown as { __TAURI_INTERNALS__: { invoke: (command: string, args?: unknown, options?: unknown) => Promise<unknown> } }).__TAURI_INTERNALS__;
    const invoke = native.invoke.bind(native);
    const resetAtMs = Date.now() + 120_000;
    native.invoke = async (command, args, options) => {
      const result = await invoke(command, args, options);
      if (command === "get_local_runtime_state") {
        const account = (result as RuntimeSnapshot).accounts[0]!;
        account.label = "Synthetic usage account";
        account.secretAvailable = auth !== "missing-secret";
        account.authState = { state: auth === "reauth" ? "requires_reauth" : "active" };
        account.recoveryRequired = auth === "recovery";
        account.quota.primary = null;
        account.quota.secondary = {
          kind: "secondary", availableBasisPoints: 9_000, explicitlyFull: false,
          resetAtMs, windowMinutes: 10_080, observedAtMs: Date.now(),
        };
        account.quotaWindowUsage = null;
        account.economics = null;
        account.apiEquivalent = { microUsd: 12_000_000, pricedTokens: 100, unpricedTokens: 0 };
        account.purchaseCostMicroUsd = 6_000_000;
      }
      return result;
    };
  }, auth);
  await page.goto("/");
  await page.getByRole("button", { name: "Usage", exact: true }).click();
  await page.locator(".usage-pool-member-menu > button").click();
  await page.getByRole("option", { name: "Personal Plus", exact: true }).click();
  const summary = page.locator(".usage-account-value");
  await expect(summary).toBeVisible();
  return summary;
}

for (const auth of ["missing-secret", "reauth", "recovery"] as const) {
  test(`usage hides stale quota but retains economics for ${auth}`, async ({ page }) => {
    const summary = await openSummary(page, auth);
    await expect(summary.locator(".usage-window-table")).toHaveCount(0);
    await expect(summary).not.toContainText("90%");
    await expect(summary).toContainText("200%");
    await expect(summary.locator(".usage-account-metrics > *")).toHaveCount(3);
  });
}

test("quota reset countdown advances without valuation samples", async ({ page }) => {
  await page.clock.install({ time: new Date("2026-11-14T12:00:00Z") });
  const summary = await openSummary(page, "active");
  const reset = summary.locator(".usage-window-table tbody tr td").last();
  const initial = await reset.innerText();
  await expect(summary).toContainText("90%");
  await page.clock.runFor(65_000);
  await expect(reset).not.toHaveText(initial);
});
