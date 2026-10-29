import { expect, test } from "../bun-playwright";
import { emitTauriEvent, installTauriMock } from "./tauri-mock";

for (const locale of ["en", "ru"] as const) {
  test(`timezone is automatic without an account picker or renderer probes (${locale})`, async ({ page }) => {
    await installTauriMock(page, { locale, populated: true, proxyCount: 1 });
    await page.goto("/");
    await page.getByRole("button", { name: locale === "ru" ? "Подключения" : "Connections", exact: true }).click();
    await page.locator(".account-card").first().locator(".account-row-menu summary").click();
    await expect(page.getByRole("menuitem", { name: /Account timezone|Часовой пояс аккаунта/ })).toHaveCount(0);
    await page.keyboard.press("Escape");
    await page.getByRole("button", { name: locale === "ru" ? "Доступ" : "Access", exact: true }).click();
    await page.getByRole("tab", { name: locale === "ru" ? "Аккаунты" : "Accounts", exact: true }).click();
    await page.getByRole("tab", { name: "ChatGPT", exact: true }).click();
    await expect(page.locator(".request-timezone-policy")).toHaveCount(0);
    await page.getByRole("button", { name: locale === "ru" ? "Подключения" : "Connections", exact: true }).click();
    await page.getByRole("tab", { name: locale === "ru" ? "Прокси" : "Proxies", exact: true }).click();
    await expect(page.locator(".proxy-region").first()).toHaveText(locale === "ru" ? "Нидерланды · Amsterdam" : "Netherlands · Amsterdam");
    const checks = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string }> }).__TAURI_TEST_INVOKES__.filter(({ command }) => command === "check_local_stored_proxy"));
    expect(checks).toHaveLength(0);
  });
}

test("proxy observation events refresh only the redacted pool and coalesce a burst", async ({ page }) => {
  await installTauriMock(page, { populated: true, proxyCount: 1 });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await page.getByRole("tab", { name: "Proxies", exact: true }).click();
  await expect(page.locator(".proxy-region").first()).toHaveText("Netherlands · Amsterdam");
  const poolReads = () => page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string }> }).__TAURI_TEST_INVOKES__.filter(({ command }) => command === "get_local_proxy_pool").length);
  const before = await poolReads();
  for (let index = 0; index < 3; index++) await emitTauriEvent(page, "zenith-proxy-pool-changed", null);
  await expect.poll(poolReads).toBe(before + 1);
  await expect(page.locator(".proxy-region").first()).toHaveText("Netherlands · Amsterdam");
});
