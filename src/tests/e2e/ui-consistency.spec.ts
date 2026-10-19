import { expect, test, type Page } from "../bun-playwright";
import { installTauriMock } from "./tauri-mock";

async function expectAlignedQuotas(page: Page, selector: string) {
  const cards = page.locator(selector);
  await expect(cards).toHaveCount(5);
  const positions = await cards.evaluateAll((elements) => elements.map((card) => ({
    row: Math.round(card.getBoundingClientRect().top),
    quota: card.querySelector(".quota-stack, .account-quota-refresh-state")!.getBoundingClientRect().top,
    reset: card.querySelector(".reset-credits-control")?.getBoundingClientRect().top,
  })));
  const rows = new Map<number, typeof positions>();
  for (const position of positions) rows.set(position.row, [...(rows.get(position.row) ?? []), position]);
  expect([...rows.values()].some((row) => row.length > 1)).toBe(true);
  for (const row of rows.values()) {
    const quotaTops = row.map((position) => position.quota);
    expect(Math.max(...quotaTops) - Math.min(...quotaTops)).toBeLessThanOrEqual(1);
    const resetTops = row.flatMap((position) => position.reset == null ? [] : [position.reset]);
    if (resetTops.length > 1) expect(Math.max(...resetTops) - Math.min(...resetTops)).toBeLessThanOrEqual(1);
  }
}

for (const theme of ["light", "dark"] as const) {
  for (const mode of ["local", "remote"] as const) {
    test(`account quotas align across optional dates and pending states ${mode} ${theme}`, async ({ page }) => {
      await page.setViewportSize({ width: 1160, height: 900 });
      await installTauriMock(page, { locale: "ru", theme, mode, populated: true, accountCount: 5, poolMembers: true });
      await page.goto("/");
      await page.getByRole("button", { name: "Подключения", exact: true }).click();
      await expectAlignedQuotas(page, ".account-card");
      await page.screenshot({ path: `output/playwright/ui-account-alignment-${mode}-${theme}.png` });
      await page.getByRole("button", { name: "Пул", exact: true }).click();
      await expectAlignedQuotas(page, '.pool-member-card[data-member-kind="account"]');
      await page.screenshot({ path: `output/playwright/ui-pool-alignment-${mode}-${theme}.png` });
      await page.getByRole("button", { name: "Подключения", exact: true }).click();
      await page.getByRole("tab", { name: "Источники API", exact: true }).click();
      const host = page.locator(".source-card-host").first();
      await expect(host).toBeVisible();
      const address = await host.getAttribute("data-relay-tooltip");
      expect(address).toMatch(/^https?:\/\//);
      await expect(host).toHaveText(address!);
    });
  }

  test(`responsive sidebar restores automatic layout and preserves manual choices ${theme}`, async ({ page }) => {
    await page.setViewportSize({ width: 1160, height: 760 });
    await installTauriMock(page, { locale: "en", theme, mode: "local", populated: true });
    await page.goto("/");
    const shell = page.locator(".relay-shell");
    await expect(shell).not.toHaveClass(/sidebar-collapsed/);
    await page.setViewportSize({ width: 840, height: 760 });
    await expect(shell).toHaveClass(/sidebar-collapsed/);
    await page.setViewportSize({ width: 1160, height: 760 });
    await expect(shell).not.toHaveClass(/sidebar-collapsed/);
    await page.getByRole("button", { name: "Collapse sidebar", exact: true }).click();
    await page.setViewportSize({ width: 840, height: 760 });
    await page.setViewportSize({ width: 1160, height: 760 });
    await expect(shell).toHaveClass(/sidebar-collapsed/);
    await page.getByRole("button", { name: "Expand sidebar", exact: true }).click();
    await page.setViewportSize({ width: 840, height: 760 });
    await expect(shell).not.toHaveClass(/sidebar-collapsed/);
  });

  test(`local API address uses the shared monospace font ${theme}`, async ({ page }) => {
    await installTauriMock(page, { locale: "en", theme, mode: "local", populated: true });
    await page.goto("/");
    await page.getByRole("button", { name: "Access", exact: true }).click();
    const address = page.locator(".gateway-api-address").first();
    await expect(address).toBeVisible();
    expect(await address.evaluate((element) => getComputedStyle(element).fontFamily)).toContain("Cascadia Code");
  });
}
