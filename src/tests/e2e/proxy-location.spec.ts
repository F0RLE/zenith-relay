import { expect, test } from "../bun-playwright";
import { installTauriMock } from "./tauri-mock";

for (const locale of ["en", "ru"] as const) {
  test(`exit country is automatic without databases even when city lookup fails (${locale})`, async ({ page }) => {
    await installTauriMock(page, { locale, populated: true, proxyCount: 1, proxyLocationError: "proxy_location_unavailable" });
    await page.goto("/");
    await page.getByRole("button", { name: locale === "ru" ? "Подключения" : "Connections", exact: true }).click();
    await page.getByRole("tab", { name: locale === "ru" ? "Прокси" : "Proxies", exact: true }).click();
    const row = page.locator(".proxy-storage-row").first();
    await expect(row.locator(".proxy-region")).toHaveText(locale === "ru" ? "Нидерланды" : "Netherlands");
    await expect(row).not.toContainText("203.0.113.42");
    await expect(row).toContainText(locale === "ru" ? "184 мс" : "184 ms");
    await expect(page.getByRole("button", { name: /Region database|База регионов/ })).toHaveCount(0);
    await expect(row).not.toContainText("MaxMind");
    await expect(row).not.toContainText(".mmdb");
    const source = await page.evaluate(async () => {
      const internals = (window as unknown as { __TAURI_INTERNALS__: { invoke: (command: string) => Promise<{ entries: Array<{ lastCheck: { locationSource: string } }> }> } }).__TAURI_INTERNALS__;
      return (await internals.invoke("get_local_proxy_pool")).entries[0]?.lastCheck.locationSource;
    });
    expect(source).toBe("cloudflare");
  });
}

test("city and timezone are automatic without database controls", async ({ page }) => {
  await installTauriMock(page, { locale: "en", populated: true, proxyCount: 1 });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await page.getByRole("tab", { name: "Proxies", exact: true }).click();
  const row = page.locator(".proxy-storage-row").first();
  await expect(row.locator(".proxy-region")).toHaveText("Netherlands · Amsterdam");
  await expect(row).not.toContainText("United States");
  await expect(row).not.toContainText("Europe/Amsterdam");
  await expect(page.getByRole("button", { name: /Region database|База регионов/ })).toHaveCount(0);
  await row.locator(".proxy-card-location").hover();
  await expect(page.getByRole("tooltip")).toContainText("Europe/Amsterdam");
});

test("unavailable geography does not invent the country declared in the proxy login", async ({ page }) => {
  await installTauriMock(page, { locale: "en", populated: true, proxyCount: 1, proxyCountryUnknown: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await page.getByRole("tab", { name: "Proxies", exact: true }).click();
  const row = page.locator(".proxy-storage-row").first();
  await expect(row.locator(".account-kind-icon")).toHaveAttribute("aria-label", "Connection works");
  await expect(row.locator(".proxy-region")).toHaveText("Region unknown");
  await expect(row).not.toContainText("United States");
});

for (const locale of ["en", "ru"] as const) {
  test(`conflicting IP families keep connectivity but omit an invented location (${locale})`, async ({ page }, testInfo) => {
    await installTauriMock(page, { locale, populated: true, proxyCount: 1, proxyLocationError: "proxy_location_conflict" });
    await page.goto("/");
    await page.getByRole("button", { name: locale === "ru" ? "Подключения" : "Connections", exact: true }).click();
    await page.getByRole("tab", { name: locale === "ru" ? "Прокси" : "Proxies", exact: true }).click();
    const row = page.locator(".proxy-storage-row").first();
    await expect(row.locator(".proxy-card-location")).toHaveAttribute("data-state", "success");
    await expect(row.locator(".proxy-region")).toHaveText(locale === "ru" ? "Регион неизвестен" : "Region unknown");
    await row.locator(".proxy-card-location").hover();
    const tooltip = page.getByRole("tooltip");
    await expect(tooltip).toContainText("IPv4: 203.0.113.42");
    await expect(tooltip).toContainText("IPv6: 2001:db8::42");
    await expect(tooltip).toContainText(locale === "ru" ? "IPv4 и IPv6" : "IPv4 and IPv6");
    await expect(tooltip).not.toContainText("Europe/Amsterdam");
    await page.screenshot({ path: testInfo.outputPath(`proxy-families-${locale}.png`) });
  });
}
