import { expect, test, type Locator } from "../bun-playwright";
import { installTauriMock } from "./tauri-mock";

async function cardStyles(card: Locator) {
  return card.evaluate((element) => {
    const read = (selector: string, properties: string[]) => {
      const node = element.querySelector(selector);
      if (!node) throw new Error(`Missing card element: ${selector}`);
      const style = getComputedStyle(node);
      return Object.fromEntries(properties.map((property) => [property, style.getPropertyValue(property)]));
    };
    const text = ["font-family", "font-size", "font-weight", "line-height"];
    return {
      identity: read(".account-identity, .pool-member-identity", ["padding", "row-gap"]),
      name: read(".account-identity > strong, .pool-member-name", text),
      quota: read(".compact-quota-layout", ["min-height", "padding"]),
      stack: read(".quota-stack", ["row-gap", "column-gap"]),
      meter: read(".quota-meter", ["padding", "row-gap"]),
      label: read(".quota-meter-heading > span", text),
      value: read(".quota-meter-heading > strong", text),
      reset: read(".quota-meter-heading small", text),
      track: read(".quota-track", ["height", "margin-top"]),
      expiry: read(".account-token-expiry", [...text, "padding", "margin"]),
      subscription: read(".account-subscription-line", [...text, "padding", "margin"]),
      economicsLabel: read(".account-value-strip dt", text),
      economicsValue: read(".account-value-strip dd", text),
      footer: read(".account-card-footer, .pool-member-card-footer", ["min-height", "padding"]),
    };
  });
}

for (const locale of ["ru", "en"] as const) {
  for (const theme of ["dark", "light"] as const) {
    for (const width of [1160, 390]) {
      test(`connection account cards match pool typography and spacing: ${locale} ${theme} ${width}`, async ({ page }, testInfo) => {
        const now = Date.UTC(2026, 9, 25, 12);
        await page.setViewportSize({ width, height: 900 });
        await page.clock.setFixedTime(now);
        await installTauriMock(page, {
          locale, theme, populated: true, accountCount: 1,
          accountOAuthClientKind: "excel_bps", subscriptionExpiresInMs: 28 * 86_400_000,
          accountCredentialDiagnostics: {
            status: "not_expired", expiresAtMs: now + 10 * 86_400_000,
            hasRefreshToken: true, recovery: "none",
          },
        });
        await page.goto("/");
        await page.getByRole("button", { name: locale === "ru" ? "Подключения" : "Connections", exact: true }).click();
        const connection = page.locator(".account-card").first();
        await expect(connection.locator(".account-value-strip")).toBeVisible();
        const styles = await cardStyles(connection);
        expect(await connection.evaluate((element) => element.scrollWidth <= element.clientWidth + 1)).toBe(true);
        await page.screenshot({ path: testInfo.outputPath("connections.png"), animations: "disabled" });
        await page.getByRole("button", { name: locale === "ru" ? "Пул" : "Pool", exact: true }).click();
        const pool = page.locator('.pool-member-card[data-member-kind="account"]').first();
        await expect(pool.locator(".account-value-strip")).toBeVisible();
        expect(await pool.evaluate((element) => element.scrollWidth <= element.clientWidth + 1)).toBe(true);
        await page.screenshot({ path: testInfo.outputPath("pool.png"), animations: "disabled" });
        expect(styles).toEqual(await cardStyles(pool));
      });
    }
  }
}
