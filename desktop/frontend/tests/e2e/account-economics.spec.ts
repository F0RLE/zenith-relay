import { expect, test, type Page } from "../bun-playwright";
import type { RuntimeSnapshot } from "../../src/features/relay/api/types";
import { en } from "../../src/i18n/locales/en";
import { ru } from "../../src/i18n/locales/ru";
import { installTauriMock } from "./tauri-mock";

async function installEconomics(page: Page, locale: "en" | "ru", unpriced = false) {
  await installTauriMock(page, {
    mode: "local", locale, theme: locale === "en" ? "light" : "dark", populated: true, usageActive: false,
    accountRouteKinds: ["codex", "excel_bps", "siwc"], accountRoutesGrouped: true,
  });
  await page.addInitScript(({ unpriced }) => {
    const native = (window as unknown as { __TAURI_INTERNALS__: { invoke: (command: string, args?: unknown, options?: unknown) => Promise<unknown> } }).__TAURI_INTERNALS__;
    const invoke = native.invoke.bind(native);
    native.invoke = async (command, args, options) => {
      const result = await invoke(command, args, options);
      if (command === "get_local_runtime_state") {
        const snapshot = result as RuntimeSnapshot;
        const methods = ["codex", "excel_bps", "siwc"].map((clientKind, index) => ({
          clientKind: clientKind as "codex" | "excel_bps" | "siwc",
          apiEquivalent: { microUsd: unpriced ? 0 : (index + 1) * 10_000_000, pricedTokens: unpriced ? 0 : 100, unpricedTokens: unpriced ? 100 : 0 },
        }));
        for (const account of snapshot.accounts) {
          account.apiEquivalent = methods.find((method) => method.clientKind === account.oauthClientKind)!.apiEquivalent;
          account.economics = {
            groupId: "route:synthetic", apiEquivalent: { microUsd: unpriced ? 0 : 60_000_000, pricedTokens: unpriced ? 0 : 300, unpricedTokens: unpriced ? 300 : 0 },
            purchaseCostMicroUsd: 20_000_000, methods,
          };
          if (account.oauthClientKind === "excel_bps") {
            account.secretAvailable = false;
            account.authState = { state: "requires_reauth" };
          }
        }
      }
      return result;
    };
  }, { unpriced });
}

for (const locale of ["en", "ru"] as const) {
  test(`${locale} linked account totals survive switching and pool display without a method footer`, async ({ page }, testInfo) => {
    const labels = locale === "en" ? en : ru;
    await page.setViewportSize({ width: locale === "en" ? 1160 : 390, height: 920 });
    await installEconomics(page, locale);
    await page.goto("/");
    await page.getByRole("button", { name: labels.nav.connections, exact: true }).click();
    await expect(page.locator(".account-card")).toHaveCount(1);
    const card = page.locator(".account-card");
    const strip = card.locator(".account-value-strip");
    const methods = card.locator(".account-value-methods");
    await expect(strip.locator("dd").first()).toHaveText(/60/);
    await expect(strip.locator("dd").last()).toHaveText(/300\s*%/);
    await expect(methods).toHaveCount(0);
    await card.locator('.account-route-selector > button[data-value="codex"]').click();
    await page.getByRole("option", { name: "SIWC", exact: true }).click();
    await expect(card.locator(".account-route-selector > button")).toHaveAttribute("data-value", "siwc");
    await expect(strip.locator("dd").first()).toHaveText(/60/);
    await expect(strip.locator("dd").last()).toHaveText(/300\s*%/);
    await expect(methods).toHaveCount(0);
    expect(await card.evaluate((node) => node.scrollWidth <= node.clientWidth + 1)).toBe(true);
    await card.screenshot({ path: testInfo.outputPath(`account-economics-${locale}.png`), animations: "disabled" });

    await page.getByRole("button", { name: labels.nav.pool, exact: true }).click();
    const pool = page.locator('.pool-member-card[data-member-kind="account"]');
    await expect(pool).toHaveCount(1);
    await expect(pool.locator(".account-value-strip dd").first()).toHaveText(/60/);
    await expect(pool.locator(".account-value-strip dd").last()).toHaveText(/300\s*%/);
    await expect(pool.locator(".account-value-methods")).toHaveCount(0);
    await pool.screenshot({ path: testInfo.outputPath(`pool-account-economics-${locale}.png`), animations: "disabled" });
    await page.getByRole("button", { name: labels.pool.hideCalculation }).click();
    await expect(page.locator(".account-value-strip, .account-value-methods")).toHaveCount(0);
  });
}

test("unpriced linked history does not display free usage or zero payback", async ({ page }) => {
  await installEconomics(page, "en", true);
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  const strip = page.locator(".account-card .account-value-strip");
  await expect(strip.locator("dd").first()).toHaveText("—");
  await expect(strip.locator("dd").last()).toHaveText("—");
  await expect(page.locator(".account-value-methods")).toHaveCount(0);
});
