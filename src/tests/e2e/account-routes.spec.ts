import { expect, test, type Page } from "../bun-playwright";
import { emitTauriEvent, installTauriMock } from "./tauri-mock";

async function openAccounts(page: Page, locale = "en") {
  await page.goto("/");
  await page.getByRole("button", { name: locale === "ru" ? "Подключения" : "Connections", exact: true }).click();
  await expect(page.locator("#splash-screen")).toHaveCount(0);
}

async function calls(page: Page) {
  return page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: Record<string, unknown> }> }).__TAURI_TEST_INVOKES__);
}

for (const locale of ["en", "ru"] as const) {
  for (const theme of ["light", "dark"] as const) {
    test(`one grouped account keeps SIWC selected while displaying linked metadata ${locale} ${theme}`, async ({ page }) => {
      await installTauriMock(page, { populated: true, locale, theme, usageActive: false, accountRouteKinds: ["codex", "siwc", "excel_bps"], accountRoutesGrouped: true });
      await openAccounts(page, locale);
      const card = page.locator(".account-card").filter({ has: page.locator(".account-route-selector") });
      await expect(card).toHaveCount(1);
      const selector = card.locator(".account-route-selector > button");
      await expect(selector).toHaveAttribute("data-value", "codex");
      await selector.click();
      await expect(page.getByRole("option")).toHaveCount(3);
      await expect(page.getByRole("option", { name: locale === "ru" ? "Подключения аккаунта" : "Account connections", exact: true })).toHaveCount(0);
      await page.mouse.move(1, 1);
      await page.screenshot({ path: `output/playwright/account-route-menu-${locale}-${theme}.png`, animations: "disabled" });
      await page.getByRole("option", { name: "SIWC", exact: true }).click();
      await expect(card.locator(".account-route-selector > button")).toHaveAttribute("data-value", "siwc");
      await expect(card).toHaveCount(1);
      await expect(card.locator(".account-plan-badge")).toHaveText("Plus");
      await expect(card.locator(".account-plan-badge")).toHaveAttribute("data-relay-tooltip", /Codex/);
      await expect(card.locator(".quota-stack")).toHaveAttribute("data-relay-tooltip", /Codex/);
      await expect(card.locator(".account-route-metadata, .account-quota-refresh-state")).toHaveCount(0);
      await expect(card.locator(".quota-meter")).toHaveCount(2);
      const changes = (await calls(page)).filter((call) => call.command === "select_local_account_route");
      expect(changes).toHaveLength(1);
      expect(changes[0]!.args).toEqual({ accountId: "account_synthetic_1", connectionId: "account_synthetic_2", attach: false });
      await page.mouse.move(1, 1);
      await expect(page.getByRole("tooltip")).toHaveCount(0);
      await card.screenshot({ path: `output/playwright/account-route-card-${locale}-${theme}.png`, animations: "disabled" });
    });

    test(`route connect opens sign-in directly without an intermediate dialog ${locale} ${theme}`, async ({ page }) => {
      await installTauriMock(page, { populated: true, locale, theme, usageActive: false, accountRouteKinds: ["codex"] });
      await openAccounts(page, locale);
      await page.locator(".account-route-selector > button").click();
      await page.getByRole("option", { name: locale === "ru" ? "SIWC · Подключить" : "SIWC · Connect", exact: true }).click();
      const dialog = page.getByRole("dialog", { name: locale === "ru" ? "Войти" : "Sign in", exact: true });
      await expect(dialog).toBeVisible();
      await expect(page.locator(".account-route-dialog")).toHaveCount(0);
      await expect.poll(async () => (await calls(page)).filter((call) => call.command === "start_codex_oauth").length).toBe(1);
      expect((await calls(page)).find((call) => call.command === "start_codex_oauth")?.args.clientKind).toBe("siwc");
      for (const width of [480, 1160]) {
        await page.setViewportSize({ width, height: 760 });
        expect(await dialog.evaluate((element) => {
          const bounds = element.getBoundingClientRect();
          const body = element.querySelector(".relay-dialog-body")!;
          return bounds.left >= 0 && bounds.right <= innerWidth && bounds.top >= 36
            && bounds.bottom <= innerHeight && body.scrollWidth <= body.clientWidth;
        })).toBe(true);
        await dialog.screenshot({ path: `output/playwright/account-route-sign-in-${locale}-${theme}-${width}.png`, animations: "disabled" });
      }
      await page.keyboard.press("Escape");
      await expect(dialog).toHaveCount(0);
      await expect.poll(async () => (await calls(page)).filter((call) => call.command === "cancel_codex_oauth").length).toBe(1);
      expect((await calls(page)).some((call) => call.command === "select_local_account_route")).toBe(false);
    });
  }
}

for (const donor of ["codex", "excel_bps"] as const) {
  test(`SIWC displays and manually refreshes ${donor} metadata without changing the selected route`, async ({ page }) => {
    await installTauriMock(page, { populated: true, usageActive: false, accountRouteKinds: [donor, "siwc"], accountRoutesGrouped: true });
    await openAccounts(page);
    const card = page.locator(".account-card").filter({ has: page.locator(".account-route-selector") });
    await card.locator(".account-route-selector > button").click();
    await page.getByRole("option", { name: "SIWC", exact: true }).click();
    await expect(card.locator(".account-route-selector > button")).toHaveAttribute("data-value", "siwc");
    const source = donor === "codex" ? "Codex" : "Excel / BPS";
    await expect(card.locator(".account-plan-badge")).toHaveText("Plus");
    await expect(card.locator(".account-plan-badge")).toHaveAttribute("data-relay-tooltip", new RegExp(source));
    await expect(card.locator(".quota-stack")).toHaveAttribute("data-relay-tooltip", new RegExp(source));
    await expect(card.locator(".account-route-metadata, .account-quota-refresh-state")).toHaveCount(0);
    await card.locator(".account-route-selector > button").focus();
    await page.keyboard.press("Shift+Tab");
    await expect(card.locator(".account-plan-badge")).toBeFocused();
    await expect(page.getByRole("tooltip")).toContainText(source);
    await expect(page.getByRole("tooltip")).toContainText("limits may differ");
    await card.locator(".account-plan-badge").blur();
    expect((await calls(page)).filter((call) => call.command === "refresh_local_account_quota")).toHaveLength(0);
    await card.getByRole("button", { name: "Refresh", exact: true }).click();
    await expect.poll(async () => (await calls(page)).filter((call) => call.command === "refresh_local_account_quota").length).toBe(1);
    expect((await calls(page)).find((call) => call.command === "refresh_local_account_quota")?.args).toEqual({ accountId: "account_synthetic_1" });
    await page.getByRole("button", { name: "Pool", exact: true }).click();
    const member = page.locator('.pool-member-card[data-member-kind="account"]');
    await expect(member).toHaveCount(1);
    await expect(member.locator(".account-route-selector > button")).toHaveAttribute("data-value", "siwc");
    await expect(member.locator(".account-plan-badge")).toHaveText("Plus");
    await expect(member.locator(".quota-stack")).toHaveAttribute("data-relay-tooltip", new RegExp(source));
    await expect(member.locator(".account-route-metadata, .account-quota-refresh-state")).toHaveCount(0);
    await expect(member.locator(".quota-meter")).toHaveCount(2);
    const records = await page.evaluate(async () => {
      const native = (window as unknown as { __TAURI_INTERNALS__: { invoke: (command: string) => Promise<{ accounts: Array<{ id: string; routeInactive?: boolean; subscription: { planType: string | null }; quota: { primary: unknown } }> }> } }).__TAURI_INTERNALS__;
      const state = await native.invoke("get_local_runtime_state");
      return state.accounts.map((account) => ({ id: account.id, inactive: account.routeInactive, plan: account.subscription.planType, primary: account.quota.primary }));
    });
    expect(records.find((account) => account.id === "account_synthetic_2")).toMatchObject({ inactive: false, plan: null, primary: null });
    expect(records.find((account) => account.id === "account_synthetic_1")?.inactive).toBe(true);
    const selected = (await calls(page)).filter((call) => call.command === "select_local_account_route");
    expect(selected).toHaveLength(1);
    expect(selected[0]!.args.connectionId).toBe("account_synthetic_2");
    await member.getByRole("button", { name: "Refresh", exact: true }).click();
    await expect.poll(async () => (await calls(page)).filter((call) => call.command === "refresh_local_account_quota").length).toBe(2);
    expect((await calls(page)).filter((call) => call.command === "refresh_local_account_quota").map((call) => call.args)).toEqual([
      { accountId: "account_synthetic_1" }, { accountId: "account_synthetic_1" },
    ]);
    await expect(member.locator(".account-route-selector > button")).toHaveAttribute("data-value", "siwc");
    await page.mouse.move(1, 1);
    for (const width of [480, 1160]) {
      await page.setViewportSize({ width, height: 900 });
      expect(await member.evaluate((element) => element.scrollWidth <= element.clientWidth)).toBe(true);
      await member.screenshot({ path: `output/playwright/siwc-metadata-${donor}-${width}.png`, animations: "disabled" });
    }
  });
}

test("switching grouped routes keeps the same-plan subscription date and each route's own token", async ({ page }) => {
  const now = Date.UTC(2026, 9, 22, 12);
  await page.clock.install({ time: now });
  await page.clock.setFixedTime(now);
  await installTauriMock(page, {
    populated: true, usageActive: false, accountRouteKinds: ["excel_bps", "siwc"], accountRoutesGrouped: true,
    accountCredentialDiagnostics: { status: "not_expired", expiresAtMs: now + 10 * 86_400_000, hasRefreshToken: true, recovery: "none" },
  });
  await openAccounts(page);
  await page.evaluate((now) => {
    const native = (window as unknown as { __TAURI_INTERNALS__: { invoke: (command: string, args?: unknown, options?: unknown) => Promise<unknown> } }).__TAURI_INTERNALS__;
    const original = native.invoke.bind(native);
    native.invoke = async (command, args, options) => {
      const result = await original(command, args, options);
      if (command === "get_local_runtime_state") {
        const accounts = (result as { accounts: import("../../src/features/relay/api/types").AccountSummary[] }).accounts;
        const siwc = accounts.find((account) => account.oauthClientKind === "siwc")!;
        siwc.subscription.planType = "plus";
        siwc.credentialDiagnostics = { status: "not_expired", expiresAtMs: now + 5 * 86_400_000, hasRefreshToken: true, recovery: "none" };
      }
      return result;
    };
  }, now);
  let card = page.locator(".account-card");
  const subscriptionDate = await card.locator(".account-subscription-line").textContent();
  for (const surface of ["Connections", "Pool"] as const) {
    if (surface === "Pool") {
      await page.getByRole("button", { name: "Pool", exact: true }).click();
      card = page.locator('.pool-member-card[data-member-kind="account"]');
    }
    for (const [route, expiry] of [["SIWC", "5 d 0 h 0 min"], ["Excel / BPS", "10 d 0 h 0 min"]] as const) {
      await card.locator(".account-route-selector > button").click();
      await page.getByRole("option", { name: route, exact: true }).click();
      await expect(card.locator(".account-subscription-line")).toHaveText(subscriptionDate!);
      await expect(card.locator(".account-token-expiry")).toHaveText(expiry);
      await expect(card.locator(".account-subscription-line")).toBeVisible();
      await expect(card.locator(".account-token-expiry")).toBeVisible();
      await expect(card.locator(".account-plan-badge svg, .quota-meter-heading svg")).toHaveCount(0);
      await expect(card.locator(".account-plan-badge")).toHaveText("Plus");
      if (route === "SIWC") {
        await page.mouse.move(0, 0);
        await expect(page.getByRole("tooltip")).toHaveCount(0);
        await card.screenshot({ path: `output/playwright/grouped-account-metadata-${surface.toLowerCase()}.png`, animations: "disabled" });
      }
    }
  }
});

for (const scenario of ["stale", "unknown-time", "different-plan", "own-limit", "quota-poll-error", "route-model-error"] as const) {
  test(`linked SIWC reference preserves ${scenario} without promoting donor evidence`, async ({ page }) => {
    await installTauriMock(page, { populated: true, usageActive: false, accountRouteKinds: ["codex", "siwc"], accountRoutesGrouped: true });
    await openAccounts(page);
    await page.evaluate((scenario) => {
      const native = (window as unknown as { __TAURI_INTERNALS__: { invoke: (command: string, args?: unknown, options?: unknown) => Promise<unknown> } }).__TAURI_INTERNALS__;
      const original = native.invoke.bind(native);
      native.invoke = async (command, args, options) => {
        const result = await original(command, args, options);
        if (command === "get_local_runtime_state") {
          const accounts = (result as { accounts: import("../../src/features/relay/api/types").AccountSummary[] }).accounts;
          const donor = accounts.find((account) => account.oauthClientKind === "codex")!;
          const siwc = accounts.find((account) => account.oauthClientKind === "siwc")!;
          siwc.subscription.activeUntilMs = null;
          if (scenario === "stale" || scenario === "unknown-time") {
            const observed = scenario === "stale" ? Date.now() - 21 * 60_000 : 0;
            donor.subscription.updatedAtMs = observed || null;
            donor.quota.primary!.observedAtMs = observed;
            donor.quota.secondary!.observedAtMs = observed;
            // A fresh snapshot must not renew old per-window observations.
            donor.quota.updatedAtMs = Date.now();
          }
          if (scenario === "different-plan") {
            donor.subscription.planType = "business";
            siwc.subscription.planType = "free";
          }
          if (scenario === "own-limit") siwc.quota.limitReached = true;
          if (scenario === "quota-poll-error" || scenario === "route-model-error") {
            siwc.quota.error = { code: "quota_prepare", occurredAtMs: Date.now() };
            siwc.quotaRefreshStatus = "failed";
            siwc.health = "degraded";
            siwc.lastErrorCode = scenario === "route-model-error" ? "models_transport" : "quota_prepare";
          }
        }
        return result;
      };
    }, scenario);
    const card = page.locator(".account-card").filter({ has: page.locator(".account-route-selector") });
    await card.locator(".account-route-selector > button").click();
    await page.getByRole("option", { name: "SIWC", exact: true }).click();
    for (const surface of ["Connections", "Pool"] as const) {
      if (surface === "Pool") await page.getByRole("button", { name: "Pool", exact: true }).click();
      const item = surface === "Pool" ? page.locator('.pool-member-card[data-member-kind="account"]') : card;
      await expect(item.locator(".account-subscription-line")).toHaveCount(scenario === "different-plan" ? 0 : 1);
      await expect(item.locator(".account-route-selector > button")).toHaveAttribute("data-value", "siwc");
      if (scenario === "stale" || scenario === "unknown-time") {
        await expect(item.locator('.account-plan-badge[data-stale="true"], .quota-stack[data-stale="true"]')).toHaveCount(2);
        await expect(item.locator('.account-metadata-indicator')).toHaveCount(0);
        await expect(item.locator(".quota-stack")).toHaveAttribute("data-relay-tooltip", /Stale/);
        if (scenario === "unknown-time") await expect(item.locator(".quota-stack")).toHaveAttribute("data-relay-tooltip", /Time unknown/);
      } else if (scenario === "different-plan") {
        await expect(item.locator(".account-plan-badge")).toHaveText("Free");
        await expect(item.locator('.account-plan-badge [data-stale]')).toHaveCount(0);
        await expect(item.locator('.account-plan-badge[data-relay-tooltip]')).toHaveCount(0);
      } else if (scenario === "quota-poll-error" || scenario === "route-model-error") {
        await expect(item.locator(".quota-stack")).toHaveCount(1);
        await expect(item.locator('.quota-stack[data-stale="true"]')).toHaveCount(0);
        await expect(item.locator(".account-quota-refresh-state")).toHaveCount(0);
        await expect(item.locator('[data-status="error"]')).toHaveCount(0);
        await expect(item.locator('[data-status="warning"]')).toHaveCount(scenario === "route-model-error" ? 1 : 0);
        if (scenario === "quota-poll-error") {
          await page.mouse.move(0, 0);
          await item.screenshot({ path: `output/playwright/siwc-quota-owner-${surface.toLowerCase()}.png`, animations: "disabled" });
        }
      } else {
        await expect(item.locator(".quota-stack")).toHaveCount(0);
        await expect(item.locator(".account-quota-refresh-state")).toBeVisible();
      }
    }
    expect((await calls(page)).filter((call) => call.command === "refresh_local_account_quota")).toHaveLength(0);
  });
}

test("linked metadata becomes stale while idle without a provider refresh", async ({ page }) => {
  await page.clock.install({ time: new Date("2026-10-21T12:00:00Z") });
  await installTauriMock(page, { populated: true, usageActive: false, accountRouteKinds: ["codex", "siwc"], accountRoutesGrouped: true });
  await openAccounts(page);
  const card = page.locator(".account-card").filter({ has: page.locator(".account-route-selector") });
  await card.locator(".account-route-selector > button").click();
  await page.getByRole("option", { name: "SIWC", exact: true }).click();
  await expect(card.locator('.account-plan-badge[data-stale="false"], .quota-stack[data-stale="false"]')).toHaveCount(2);
  await page.clock.fastForward(21 * 60_000);
  await expect(card.locator('.account-plan-badge[data-stale="true"], .quota-stack[data-stale="true"]')).toHaveCount(2);
  expect((await calls(page)).filter((call) => call.command === "refresh_local_account_quota")).toHaveLength(0);
});

for (const locale of ["en", "ru"] as const) {
  test(`account actions omit connection management and keep saved notes ${locale}`, async ({ page }) => {
    await installTauriMock(page, { populated: true, locale, usageActive: false, accountRouteKinds: ["codex", "siwc"], accountRoutesGrouped: true });
    await openAccounts(page, locale);
    const title = locale === "ru" ? "Подключения аккаунта" : "Account connections";
    const menu = page.locator(".account-card .account-row-menu");
    await menu.locator("summary").click();
    await expect(page.getByRole("menuitem", { name: title, exact: true })).toHaveCount(0);
    await expect(page.getByRole("dialog", { name: title, exact: true })).toHaveCount(0);
    const notes = locale === "ru" ? "Заметки" : "Notes";
    await expect(page.getByRole("menuitem", { name: notes, exact: true })).toBeVisible();
    await page.screenshot({ path: `output/playwright/account-menu-clean-${locale}.png`, animations: "disabled" });
    await page.getByRole("menuitem", { name: notes, exact: true }).click();
    await expect(page.getByRole("dialog", { name: notes, exact: true })).toBeVisible();
    expect((await calls(page)).some((call) => call.command === "local_account_route_candidates")).toBe(false);
    expect((await calls(page)).some((call) => call.command === "start_codex_oauth")).toBe(false);
  });
}

test("linked SIWC quota never hides a sign-in requirement on the selected route", async ({ page }) => {
  await installTauriMock(page, { populated: true, usageActive: false, accountAuthReason: "expired_refresh_token", accountRouteKinds: ["codex", "siwc"], accountRoutesGrouped: true });
  await openAccounts(page);
  const card = page.locator(".account-card").filter({ has: page.locator(".account-route-selector") });
  await card.locator(".account-route-selector > button").click();
  await page.getByRole("option", { name: "SIWC", exact: true }).click();
  await expect(card.locator(".account-route-selector > button")).toHaveAttribute("data-value", "siwc");
  await expect(card.getByRole("button", { name: "Sign-in required", exact: true })).toBeVisible();
  await expect(card.locator(".quota-stack")).toHaveCount(0);
  await page.getByRole("button", { name: "Pool", exact: true }).click();
  const member = page.locator('.pool-member-card[data-member-kind="account"]');
  await expect(member.getByRole("button", { name: "Sign-in required", exact: true })).toBeVisible();
  await expect(member.locator(".quota-stack")).toHaveCount(0);
});

for (const [kind, label] of [["codex", "Codex"], ["siwc", "SIWC"], ["excel_bps", "Excel / BPS"]] as const) {
test(`a missing ${kind} route starts its own authorization without reusing the current grant`, async ({ page }) => {
  await installTauriMock(page, { populated: true, usageActive: false, accountRouteKinds: [kind === "codex" ? "excel_bps" : "codex"] });
  await openAccounts(page);
  await page.locator(".account-route-selector > button").click();
  await page.getByRole("option", { name: `${label} · Connect`, exact: true }).click();
  await expect.poll(async () => (await calls(page)).some((call) => call.command === "start_codex_oauth")).toBe(true);
  const call = (await calls(page)).find((call) => call.command === "start_codex_oauth")!;
  expect(call.args.clientKind).toBe(kind);
  expect(call.args.accountId).toBeUndefined();
  expect((await calls(page)).some((call) => call.command === "select_local_account_route")).toBe(false);
  await expect(page.locator(".account-route-dialog")).toHaveCount(0);
});
}

test("a rejected switch leaves the original route selected", async ({ page }) => {
  await installTauriMock(page, { populated: true, usageActive: false, accountRouteKinds: ["codex", "siwc"], accountRoutesGrouped: true, accountRouteSelectError: true });
  await openAccounts(page);
  await page.locator(".account-route-selector > button").click();
  await page.getByRole("option", { name: "SIWC", exact: true }).click();
  await expect.poll(async () => (await calls(page)).filter((call) => call.command === "select_local_account_route").length).toBe(1);
  await expect(page.locator(".account-route-selector > button")).toHaveAttribute("data-value", "codex");
});

for (const platform of ["windows", "macos", "linux"] as const) {
for (const [kind, label] of [["codex", "Codex"], ["excel_bps", "Excel / BPS"]] as const) {
  test(`left click on missing ${kind} starts one automatic sign-in from source notes on ${platform}`, async ({ page }) => {
    await installTauriMock(page, { platform, populated: true, usageActive: false, accountRouteKinds: ["siwc"], oauthCompletedRouteKind: kind });
    await openAccounts(page);
    await page.locator(".account-route-selector > button").click();
    await page.getByRole("option", { name: `${label} · Connect`, exact: true }).click();
    await expect(page.getByRole("dialog", { name: "Sign in", exact: true })).toBeVisible();
    await expect(page.locator(".app-context-menu, .account-route-dialog")).toHaveCount(0);
    await expect(page.getByText("Trying your saved email", { exact: false })).toHaveCount(0);
    const started = (await calls(page)).filter((call) => call.command === "start_codex_oauth");
    expect(started).toHaveLength(1);
    expect(started[0]!.args).toMatchObject({ openBrowser: true, clientKind: kind, automaticLogin: { notesAccountId: "account_synthetic_1" } });
    expect(started[0]!.args.accountId).toBeUndefined();
    expect((await calls(page)).some((call) => call.command === "reveal_local_account_login")).toBe(false);
    await emitTauriEvent(page, "relay-oauth-status", { loginId: "oauth_synthetic", status: "callback_received" });
    await expect(page.locator(".account-route-selector > button")).toHaveAttribute("data-value", kind);
    expect((await calls(page)).filter((call) => call.command === "select_local_account_route")).toHaveLength(1);
  });
}

test(`native missing-notes fallback opens manual route sign-in without the automatic hint on ${platform}`, async ({ page }) => {
  await installTauriMock(page, { platform, populated: true, usageActive: false, accountRouteKinds: ["siwc"] });
  await page.addInitScript(() => {
    const host = window as unknown as { __TAURI_INTERNALS__: { invoke: (command: string, args?: Record<string, unknown>) => Promise<unknown> } };
    const invoke = host.__TAURI_INTERNALS__.invoke.bind(host.__TAURI_INTERNALS__);
    host.__TAURI_INTERNALS__.invoke = async (command, args) => {
      const result = await invoke(command, args);
      return command === "start_codex_oauth" ? { ...result as object, automaticLogin: false } : result;
    };
  });
  await openAccounts(page);
  await page.locator(".account-route-selector > button").click();
  await page.getByRole("option", { name: "Excel / BPS · Connect", exact: true }).click();
  await expect(page.getByRole("dialog", { name: "Sign in", exact: true })).toBeVisible();
  await expect(page.getByText("Trying your saved email", { exact: false })).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Open sign-in window", exact: true })).toBeEnabled();
});
}

for (const platform of ["windows", "macos", "linux"] as const) {
  test(`left click keeps manual SIWC sign-in on ${platform}`, async ({ page }) => {
    await installTauriMock(page, { platform, populated: true, usageActive: false, accountRouteKinds: ["codex"] });
    await openAccounts(page);
    await page.locator(".account-route-selector > button").click();
    await page.getByRole("option", { name: "SIWC · Connect", exact: true }).click();
    await expect(page.getByRole("dialog", { name: "Sign in", exact: true })).toBeVisible();
    const started = (await calls(page)).filter((call) => call.command === "start_codex_oauth");
    expect(started).toHaveLength(1);
    expect(started[0]!.args).toMatchObject({ openBrowser: false });
    expect(started[0]!.args).not.toHaveProperty("automaticLogin");
  });
}

for (const [kind, label] of [["codex", "Codex"], ["siwc", "SIWC"], ["excel_bps", "Excel / BPS"]] as const) {
  test(`right click on missing ${kind} no longer starts sign-in`, async ({ page }) => {
    await installTauriMock(page, { platform: "windows", populated: true, usageActive: false, accountRouteKinds: [kind === "codex" ? "siwc" : "codex"] });
    await openAccounts(page);
    await page.locator(".account-route-selector > button").click();
    await page.getByRole("option", { name: `${label} · Connect`, exact: true }).click({ button: "right" });
    expect((await calls(page)).some((call) => call.command === "start_codex_oauth")).toBe(false);
    await expect(page.getByRole("dialog", { name: "Sign in", exact: true })).toHaveCount(0);
  });
}

test("a newly authorized route is associated only after OAuth completion", async ({ page }) => {
  await installTauriMock(page, { populated: true, usageActive: false, accountRouteKinds: ["codex"], oauthCompletedRouteKind: "siwc" });
  await openAccounts(page);
  await page.locator(".account-route-selector > button").click();
  await page.getByRole("option", { name: "SIWC · Connect", exact: true }).click();
  await expect.poll(async () => (await calls(page)).some((call) => call.command === "start_codex_oauth")).toBe(true);
  expect((await calls(page)).some((call) => call.command === "select_local_account_route")).toBe(false);
  await emitTauriEvent(page, "relay-oauth-status", { loginId: "oauth_synthetic", status: "callback_received" });
  await expect(page.locator(".account-route-selector > button")).toHaveCount(1);
  await expect(page.locator(".account-route-selector > button")).toHaveAttribute("data-value", "siwc");
  const commands = await calls(page);
  expect(commands.findIndex((call) => call.command === "complete_codex_oauth")).toBeLessThan(commands.findIndex((call) => call.command === "select_local_account_route"));
  expect(commands.find((call) => call.command === "select_local_account_route")?.args).toEqual({ accountId: "account_synthetic_1", connectionId: "account_synthetic_route", attach: true });
});

test("an account-targeted automation follows the selected route and remains editable", async ({ page }) => {
  await installTauriMock(page, { populated: true, usageActive: false, accountRouteKinds: ["codex", "excel_bps"], accountRoutesGrouped: true,
    automation: { accountSelector: { kind: "account_ids", values: ["account_synthetic_1"] } },
  });
  await openAccounts(page);
  await page.locator(".account-route-selector > button").click();
  await page.getByRole("option", { name: "Excel / BPS", exact: true }).click();
  await expect(page.locator(".account-route-selector > button")).toHaveAttribute("data-value", "excel_bps");
  await page.getByRole("tab", { name: "Automations", exact: true }).click();
  await page.getByRole("button", { name: "Edit", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "Edit automation", exact: true });
  await expect(dialog.locator('input[type="checkbox"]:checked')).toHaveCount(1);
  await expect(dialog.getByRole("button", { name: "Save", exact: true })).toBeEnabled();
  await dialog.getByRole("button", { name: "Save", exact: true }).click();
  const update = (await calls(page)).find((call) => call.command === "update_quota_wake_automation");
  expect(update?.args.input).toMatchObject({ accountSelector: { kind: "account_ids", values: ["account_synthetic_2"] } });
});

test("an active request prevents route switching", async ({ page }) => {
  await installTauriMock(page, { populated: true, usageActive: true, accountRouteKinds: ["codex", "siwc"], accountRoutesGrouped: true });
  await openAccounts(page);
  await expect(page.locator(".account-route-selector > button")).toBeDisabled();
  await expect(page.locator(".account-route-selector > button")).toHaveAccessibleName(/Wait for active requests/);
  expect((await calls(page)).some((call) => call.command === "select_local_account_route")).toBe(false);
});

test("rotation uses the same single account and route control", async ({ page }) => {
  await installTauriMock(page, { populated: true, usageActive: false, accountRouteKinds: ["codex", "siwc", "excel_bps"], accountRoutesGrouped: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Pool", exact: true }).click();
  const account = page.locator('.pool-member-card[data-member-kind="account"]');
  await expect(account).toHaveCount(1);
  await account.locator(".account-route-selector > button").click();
  await page.getByRole("option", { name: "Excel / BPS", exact: true }).click();
  await expect(account).toHaveCount(1);
  await expect(account.locator(".account-route-selector > button")).toHaveAttribute("data-value", "excel_bps");
});

for (const theme of ["light", "dark"] as const) {
  for (const plan of ["plus", "pro"] as const) {
    test(`plan and route badges share sizing on account cards: ${plan}, ${theme}`, async ({ page }) => {
      await installTauriMock(page, {
        populated: true, theme, usageActive: false, accountPlanType: plan,
        accountRouteKinds: ["codex", "siwc", "excel_bps"], accountRoutesGrouped: true,
      });
      await openAccounts(page);
      const initial = page.locator(".account-card .account-route-selector > button");
      await expect(initial).toHaveCSS("height", "24px");
      await initial.click();
      await page.getByRole("option", { name: "Excel / BPS", exact: true }).click();

      for (const surface of ["connections", "pool"] as const) {
        if (surface === "pool") await page.getByRole("button", { name: "Pool", exact: true }).click();
        const card = surface === "pool"
          ? page.locator('.pool-member-card[data-member-kind="account"]')
          : page.locator(".account-card");
        await expect(card).toHaveCount(1);
        await expect(card.locator(".account-plan-badge")).toHaveText(plan === "plus" ? "Plus" : "Pro 200");
        await expect(card.locator(".account-route-selector > button")).toHaveAttribute("data-value", "excel_bps");
        for (const width of [360, 800, 1152, 1160]) {
          await page.setViewportSize({ width, height: 900 });
          const badges = await card.locator(".account-badges").evaluate((element) => {
            const planBadge = element.querySelector(".account-plan-badge")!;
            const route = element.querySelector(".account-route-selector > button")!;
            const planBounds = planBadge.getBoundingClientRect();
            const routeBounds = route.getBoundingClientRect();
            const gap = Number.parseFloat(getComputedStyle(element).gap);
            const styles = [planBadge, route].map((badge) => {
              const bounds = badge.getBoundingClientRect();
              const style = getComputedStyle(badge);
              return { height: bounds.height, fontSize: style.fontSize,
                paddingLeft: style.paddingLeft, paddingRight: style.paddingRight, radius: style.borderRadius };
            });
            return { styles, sameLine: Math.abs(planBounds.top - routeBounds.top) < 1,
              requiredWidth: planBounds.width + routeBounds.width + gap, availableWidth: element.clientWidth,
              rowGap: routeBounds.top - planBounds.bottom, gap };
          });
          expect(badges.styles[0]).toEqual(badges.styles[1]);
          expect(badges.styles[0]!.height).toBe(24);
          if (width >= 800) expect(badges.sameLine, JSON.stringify({ surface, width, ...badges })).toBe(true);
          if (!badges.sameLine) {
            expect(badges.requiredWidth).toBeGreaterThan(badges.availableWidth);
            expect(badges.rowGap).toBeCloseTo(badges.gap, 0);
          }
          expect(await card.evaluate((element) => element.scrollWidth <= element.clientWidth + 1)).toBe(true);
        }
        await page.mouse.move(1, 1);
        await card.screenshot({ path: `output/playwright/account-badge-sizing-${plan}-${surface}-${theme}.png`, animations: "disabled" });
      }
    });
  }
}

for (const width of [800, 1160]) {
  test(`the longest route label does not overlap account actions at ${width}px`, async ({ page }) => {
    await page.setViewportSize({ width, height: 850 });
    await installTauriMock(page, { populated: true, usageActive: false, accountRouteKinds: ["excel_bps", "codex", "siwc"], accountRoutesGrouped: true });
    await openAccounts(page);
    const card = page.locator(".account-card").filter({ has: page.locator(".account-route-selector") });
    const positions = await card.evaluate((element) => ({
      route: element.querySelector(".account-route-selector > button")!.getBoundingClientRect().right,
      actions: element.querySelector(".account-card-header-actions")!.getBoundingClientRect().left,
      overflow: element.scrollWidth - element.clientWidth,
    }));
    expect(positions.route).toBeLessThanOrEqual(positions.actions + 1);
    expect(positions.overflow).toBeLessThanOrEqual(1);
    await card.locator(".account-route-selector > button").click();
    await expect(page.getByRole("option")).toHaveCount(3);
  });
}
