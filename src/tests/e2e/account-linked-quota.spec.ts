import { expect, test, type Page } from "../bun-playwright";
import { installTauriMock } from "./tauri-mock";
import type { AccountSummary } from "../../src/features/relay/api/types";

async function openSiwc(page: Page, locale = "en") {
  await page.goto("/");
  await page.getByRole("button", { name: locale === "ru" ? "Подключения" : "Connections", exact: true }).click();
  await page.locator(".account-route-selector > button").click();
  await page.getByRole("option", { name: "SIWC", exact: true }).click();
  await expect(page.locator(".account-route-selector > button")).toHaveAttribute("data-value", "siwc");
}

for (const locale of ["en", "ru"] as const) {
  for (const donor of ["codex", "excel_bps"] as const) {
    test(`linked weekly quota warns without blocking SIWC ${donor} ${locale}`, async ({ page }) => {
      const theme = locale === "ru" ? "dark" : "light";
      await installTauriMock(page, {
        populated: true, usageActive: false, locale, theme,
        accountRouteKinds: [donor, "siwc"], accountRoutesGrouped: true,
        exhaustedQuotaWindow: "secondary",
      });
      await openSiwc(page, locale);
      const client = donor === "codex" ? "Codex" : "Excel / BPS";
      const text = locale === "ru" ? `Недельная квота ${client} исчерпана` : `${client} weekly quota exhausted`;
      for (const surface of ["connections", "pool"] as const) {
        if (surface === "pool") await page.getByRole("button", { name: locale === "ru" ? "Пул" : "Pool", exact: true }).click();
        const card = page.locator(surface === "pool" ? '.pool-member-card[data-member-kind="account"]' : ".account-card");
        const indicator = card.locator(".account-kind-icon, .pool-member-kind-icon");
        await expect(indicator).toHaveAttribute("data-status", "warning");
        await expect(indicator).toHaveAttribute("aria-label", new RegExp(text));
        await expect(card.locator(".account-linked-quota-state")).toHaveCount(0);
        await indicator.hover();
        await expect(page.getByRole("tooltip")).toContainText(text);
        await page.mouse.move(1, 1);
        await expect(page.getByRole("tooltip")).toHaveCount(0);
        await indicator.focus();
        await page.keyboard.press("Tab");
        await page.keyboard.press("Shift+Tab");
        await expect(indicator).toBeFocused();
        await expect(page.getByRole("tooltip")).toContainText(text);
        await indicator.blur();
        await page.mouse.move(1, 1);
        await expect(page.getByRole("tooltip")).toHaveCount(0);
        await expect(card.locator(".quota-stack")).toHaveAttribute("data-stale", "false");
        await expect(card.locator(".quota-meter strong").last()).toHaveText("0%");
        await expect(card.locator(".account-route-selector > button")).toHaveAttribute("data-value", "siwc");
        for (const width of [480, 1160]) {
          await page.setViewportSize({ width, height: 760 });
          await expect(indicator).toBeVisible();
          await page.mouse.move(1, 1);
          await card.screenshot({ path: `output/playwright/linked-quota-${donor}-${locale}-${surface}-${width}.png`, animations: "disabled" });
        }
      }
      const state = await page.evaluate(async () => {
        const host = window as unknown as { __TAURI_INTERNALS__: { invoke: (command: string) => Promise<{ accounts: AccountSummary[] }> } };
        const account = (await host.__TAURI_INTERNALS__.invoke("get_local_runtime_state")).accounts.find((item) => item.oauthClientKind === "siwc")!;
        return { status: account.operationalStatus, quota: account.quota, auth: account.authState.state };
      });
      expect(state.status).toBe("rotation");
      expect(state.auth).toBe("active");
      expect(state.quota.primary).toBeNull();
      expect(state.quota.secondary).toBeNull();
      expect(state.quota.limitReached).toBe(false);
    });
  }
}

for (const condition of ["stale", "failed", "small_positive"] as const) {
  test(`linked quota ${condition} does not become current exhaustion`, async ({ page }) => {
    await installTauriMock(page, {
      populated: true, usageActive: false, accountRouteKinds: ["codex", "siwc"],
      accountRoutesGrouped: true, exhaustedQuotaWindow: "secondary",
    });
    await page.addInitScript((condition) => {
      const host = window as unknown as { __TAURI_INTERNALS__: { invoke: (command: string, args?: unknown, options?: unknown) => Promise<unknown> } };
      const original = host.__TAURI_INTERNALS__.invoke.bind(host.__TAURI_INTERNALS__);
      host.__TAURI_INTERNALS__.invoke = async (command, args, options) => {
        const result = await original(command, args, options);
        if (command === "get_local_runtime_state") {
          const source = (result as { accounts: AccountSummary[] }).accounts.find((account) => account.oauthClientKind === "codex")!;
          if (condition === "stale") {
            for (const window of [source.quota.primary, source.quota.secondary]) {
              if (window) window.observedAtMs = Date.now() - 21 * 60_000;
            }
          } else if (condition === "failed") {
            source.quotaRefreshStatus = "failed";
          } else {
            source.quota.secondary!.availableBasisPoints = 1;
            source.quota.limitReached = false;
          }
        }
        return result;
      };
    }, condition);
    await openSiwc(page);
    for (const surface of ["connections", "pool"] as const) {
      if (surface === "pool") await page.getByRole("button", { name: "Pool", exact: true }).click();
      const card = page.locator(surface === "pool" ? '.pool-member-card[data-member-kind="account"]' : ".account-card");
      const indicator = card.locator(".account-kind-icon, .pool-member-kind-icon");
      await expect(card.locator(".account-linked-quota-state")).toHaveCount(0);
      await expect(indicator).toHaveAttribute("data-status", condition === "small_positive" ? "ready" : "warning");
      if (condition === "small_positive") {
        await expect(indicator).not.toHaveAttribute("aria-label", /quota exhausted|Stale/);
      } else {
        await expect(indicator).toHaveAttribute("aria-label", /Source: Codex.*Stale/);
        await expect(indicator).not.toHaveAttribute("aria-label", /quota exhausted/);
      }
      await expect(card.locator(".quota-stack")).toHaveAttribute("data-stale", condition === "small_positive" ? "false" : "true");
      await expect(card.locator(".account-route-selector > button")).toHaveAttribute("data-value", "siwc");
    }
  });
}

test("SIWC own quota exhaustion remains authoritative over linked available counters", async ({ page }) => {
  await installTauriMock(page, {
    populated: true, usageActive: false, quotaAvailable: true,
    accountRouteKinds: ["codex", "siwc"], accountRoutesGrouped: true,
  });
  await page.addInitScript(() => {
    const host = window as unknown as { __TAURI_INTERNALS__: { invoke: (command: string, args?: unknown, options?: unknown) => Promise<unknown> } };
    const original = host.__TAURI_INTERNALS__.invoke.bind(host.__TAURI_INTERNALS__);
    host.__TAURI_INTERNALS__.invoke = async (command, args, options) => {
      const result = await original(command, args, options);
      if (command === "get_local_runtime_state") {
        const account = (result as { accounts: AccountSummary[] }).accounts.find((item) => item.oauthClientKind === "siwc")!;
        account.quota.limitReached = true;
        account.operationalStatus = "quotaWait";
        account.routingBlockReason = "quota_exhausted";
      }
      return result;
    };
  });
  await openSiwc(page);
  for (const surface of ["connections", "pool"] as const) {
    if (surface === "pool") await page.getByRole("button", { name: "Pool", exact: true }).click();
    const card = page.locator(surface === "pool" ? '.pool-member-card[data-member-kind="account"]' : ".account-card");
    await expect(card.locator(".account-linked-quota-state, .quota-meter")).toHaveCount(0);
    await expect(card.locator(".account-quota-refresh-state")).toHaveText("Waiting for quota");
  }
});
