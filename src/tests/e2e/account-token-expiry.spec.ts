import { expect, test } from "../bun-playwright";
import { installTauriMock } from "./tauri-mock";

const now = Date.UTC(2026, 9, 15, 12);

for (const mode of ["local"] as const) {
  for (const locale of ["en", "ru"] as const) {
    test(`${mode} ${locale} BPS cards show a compact token countdown without authorization banners`, async ({ page }, testInfo) => {
      await page.setViewportSize({ width: 640, height: 900 });
      await page.clock.install({ time: now });
      await page.clock.setFixedTime(now);
      await installTauriMock(page, {
        mode, locale, theme: "dark", populated: true, accountCount: 1,
        accountOAuthClientKind: "excel_bps",
        accountCredentialDiagnostics: {
          status: "not_expired", expiresAtMs: now + 10 * 86_400_000,
          hasRefreshToken: true, recovery: "none",
        },
      });
      await page.goto("/");
      await page.getByRole("button", { name: locale === "ru" ? "Подключения" : "Connections", exact: true }).click();
      const card = page.locator(".account-card").first();
      const expiry = card.locator(".account-token-expiry");
      await expect(expiry).toHaveText(locale === "ru" ? "10 дн. 0 ч 0 мин" : "10 d 0 h 0 min");
      await expect(expiry.locator("svg")).toHaveCount(1);
      await expect(expiry).toHaveAttribute("data-relay-tooltip", /2026/);
      await expect(card.locator(".account-value-strip")).toBeVisible();
      expect(await expiry.evaluate((element) => ({
        bottom: element.nextElementSibling?.className,
        height: element.getBoundingClientRect().height,
        overflow: element.scrollWidth > element.clientWidth,
      }))).toEqual({ bottom: "account-subscription-line", height: 24, overflow: false });
      await expect(page.locator(".account-credential-diagnostics")).toHaveCount(0);
      await expect(page.getByText(/Авторизация BPS|BPS authorization|Локальное состояние токена|Local token state/)).toHaveCount(0);
      await page.screenshot({ path: testInfo.outputPath("connections-token-expiry.png") });

      await page.getByRole("button", { name: locale === "ru" ? "Пул" : "Pool", exact: true }).click();
      const member = page.locator('.pool-member-card[data-member-label="Personal Plus"]');
      await expect(member.locator(".account-token-expiry")).toHaveText(locale === "ru" ? "10 дн. 0 ч 0 мин" : "10 d 0 h 0 min");
      await expect(member.locator(".account-value-strip")).toBeVisible();
      await page.screenshot({ path: testInfo.outputPath("pool-token-expiry.png") });
      await member.locator(".pool-member-actions button").last().click();
      await expect(page.getByRole("dialog")).toBeVisible();
      await expect(page.getByRole("dialog").locator(".account-credential-diagnostics, .account-token-expiry")).toHaveCount(0);
    });
  }
}

for (const surface of ["Connections", "Pool"] as const) {
  test(`${surface} token expiry only updates its label without refreshing or disabling accounts`, async ({ page }) => {
    await page.clock.install({ time: now });
    await installTauriMock(page, {
      mode: "local", locale: "en", populated: true, accountCount: 1,
      accountOAuthClientKind: "excel_bps",
      accountCredentialDiagnostics: {
        status: "not_expired", expiresAtMs: now + 10_000,
        hasRefreshToken: true, recovery: "none",
      },
    });
    await page.goto("/");
    await page.getByRole("button", { name: surface, exact: true }).click();
    const card = page.locator(surface === "Connections" ? ".account-card" : '.pool-member-card[data-member-kind="account"]').first();
    const expiry = card.locator(".account-token-expiry");
    await expect(expiry).toContainText("s");
    const status = card.locator(".relay-status-icon").first();
    const initialStatus = await status.getAttribute("data-status");
    const before = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: unknown[] }).__TAURI_TEST_INVOKES__.length);
    await page.clock.runFor(11_000);
    await expect(expiry).toHaveText("Expired");
    expect(await status.getAttribute("data-status")).toBe(initialStatus);
    await expect(card.getByRole("button", { name: /Remove from pool/ })).toBeEnabled();
    await page.clock.runFor(5_000);
    await expect(expiry).toHaveText("Expired");
    const mutations = await page.evaluate((offset) => (window as unknown as {
      __TAURI_TEST_INVOKES__: Array<{ command: string }>;
    }).__TAURI_TEST_INVOKES__.slice(offset).filter((call) => /refresh.*account|account.*(?:refresh|enabled)|update_.*account|membership|remote_action/.test(call.command)), before);
    expect(mutations).toEqual([]);
  });
}

test("unknown token expiry stays unknown without a reauthentication banner", async ({ page }) => {
  await installTauriMock(page, {
    mode: "local", locale: "en", populated: true,
    accountOAuthClientKind: "excel_bps",
    accountCredentialDiagnostics: { status: "unknown", expiresAtMs: null, hasRefreshToken: false, recovery: "sign_in" },
  });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await expect(page.locator(".account-token-expiry")).toHaveText("—");
  await expect(page.locator(".account-token-expiry")).toHaveAttribute("data-relay-tooltip", "Token expiry is unknown");
  await expect(page.getByText("Sign in to Excel / Basis Points again.", { exact: true })).toHaveCount(0);
});

for (const accountOAuthClientKind of ["codex", "excel_bps"] as const) {
  test(`${accountOAuthClientKind} accounts without diagnostics do not invent a token expiry`, async ({ page }) => {
    await installTauriMock(page, { mode: "local", locale: "en", populated: true, accountOAuthClientKind });
    await page.goto("/");
    await page.getByRole("button", { name: "Connections", exact: true }).click();
    await expect(page.locator(".account-card")).toBeVisible();
    await expect(page.locator(".account-token-expiry")).toHaveCount(0);
    await page.getByRole("button", { name: "Pool", exact: true }).click();
    await expect(page.locator('.pool-member-card[data-member-kind="account"]')).toBeVisible();
    await expect(page.locator(".account-token-expiry")).toHaveCount(0);
  });
}
