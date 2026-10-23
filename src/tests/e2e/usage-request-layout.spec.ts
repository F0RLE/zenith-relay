import { expect, test } from "../bun-playwright";
import { installTauriMock } from "./tauri-mock";

for (const scenario of [
  { width: 2451, locale: "en", theme: "light" },
  { width: 1660, locale: "ru", theme: "dark" },
  { width: 1160, locale: "ru", theme: "dark" },
  { width: 840, locale: "en", theme: "light" },
  { width: 390, locale: "ru", theme: "dark" },
] as const) {
  test(`request table keeps identity readable and status compact: ${scenario.width} ${scenario.locale}`, async ({ page }) => {
    const accountLabel = "long.synthetic.member.0123456789@example.test";
    await page.setViewportSize({ width: scenario.width, height: 900 });
    await installTauriMock(page, {
      ...scenario, mode: "local", populated: true, accountLabel,
      usageRequestedModel: "synthetic-model-with-a-long-name-2026",
      usageResolvedModel: "synthetic-model-with-a-long-name-2026",
      usageRepeatedRequests: true,
    });
    await page.goto("/");
    await page.getByRole("button", { name: scenario.locale === "ru" ? "Использование" : "Usage", exact: true }).click();
    const table = page.locator(".usage-request-table");
    const row = table.locator("tbody tr").first();
    const connection = row.locator(".usage-request-connection");
    await expect(connection).toHaveText(accountLabel);
    await expect(row.locator('[data-column="connection"]')).toHaveAttribute("data-relay-tooltip", accountLabel);
    if (scenario.width >= 840) {
      expect(await table.locator("th, td").evaluateAll((elements) => elements
        .filter((element) => element.getBoundingClientRect().width > 0)
        .every((element) => getComputedStyle(element).textAlign === "center"))).toBe(true);
      expect(await row.locator(".usage-model-value").evaluate((element) => getComputedStyle(element).alignItems)).toBe("center");
      expect(await row.locator(".usage-model-value small").evaluate((element) => {
        const label = element.getBoundingClientRect();
        const cell = element.closest("td")!.getBoundingClientRect();
        return Math.abs(label.left + label.width / 2 - cell.left - cell.width / 2);
      })).toBeLessThanOrEqual(1);
      expect(await connection.evaluate((element) => ({
        whiteSpace: getComputedStyle(element).whiteSpace,
        textOverflow: getComputedStyle(element).textOverflow,
        height: element.getBoundingClientRect().height,
        lineHeight: parseFloat(getComputedStyle(element).lineHeight),
      }))).toMatchObject({ whiteSpace: "nowrap", textOverflow: "ellipsis", height: 18, lineHeight: 18 });
    } else {
      expect(await row.locator('[data-column="model"]').evaluate((element) => getComputedStyle(element).textAlign)).toBe("left");
      expect(await row.locator(".usage-model-value").evaluate((element) => getComputedStyle(element).alignItems)).toBe("flex-start");
    }
    if (scenario.width >= 1160) {
      const statusWidth = await table.locator('th[data-column="status"]').evaluate((element) => element.getBoundingClientRect().width);
      expect(statusWidth).toBeLessThanOrEqual(80);
      for (const column of ["model", "connection"]) {
        expect(await table.locator(`th[data-column="${column}"]`).evaluate((element) => element.getBoundingClientRect().width)).toBeGreaterThan(statusWidth * 2);
      }
      for (const column of ["tokens", "request"]) {
        const heading = table.locator(`th[data-column="${column}"] .usage-column-heading > span`);
        expect(await heading.evaluate((element) => element.getBoundingClientRect().height)).toBeLessThanOrEqual(17);
      }
    }
    expect(await table.locator("td").evaluateAll((elements) => elements.filter((element) => element.getBoundingClientRect().width > 0)
      .every((element) => element.scrollWidth <= element.clientWidth + 1))).toBe(true);
    await page.mouse.move(0, 0);
    await (scenario.width < 840 ? row : table).screenshot({ path: `output/playwright/usage-requests-centered-${scenario.locale}-${scenario.width}.png` });
    await row.getByRole("button", { name: /req|relay-synthetic/ }).click();
    await expect(page.getByRole("dialog")).toBeVisible();
  });
}
