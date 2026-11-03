import { expect, test } from "../bun-playwright";
import { installTauriMock } from "./tauri-mock";

for (const scenario of [
  { locale: "en", theme: "dark", width: 1160 },
  { locale: "ru", theme: "light", width: 840 },
  { locale: "ru", theme: "dark", width: 390 },
] as const) {
  test(`explicit image-only membership from account and picker: ${scenario.locale} ${scenario.width}`, async ({ page }) => {
    await page.setViewportSize({ width: scenario.width, height: 800 });
    await installTauriMock(page, { ...scenario, mode: "local", populated: true, poolMembers: false, accountOAuthClientKind: "codex", usageActive: false });
    const ru = scenario.locale === "ru";
    const badge = ru ? "Только изображения" : "Images only";
    await page.goto("/");
    const navigate = async (name: string) => {
      const menu = page.getByRole("button", { name: ru ? "Меню" : "Menu", exact: true });
      if (await menu.isVisible()) await menu.click();
      await page.getByRole("button", { name, exact: true }).click();
    };
    await navigate(ru ? "Подключения" : "Connections");
    const card = page.locator(".account-card").first();
    await expect(card.getByText(badge, { exact: true })).toHaveCount(0);
    const add = card.getByRole("button", { name: ru ? "Добавить в пул для изображений" : "Add to pool for images", exact: true });
    await add.click();
    const remove = card.getByRole("button", { name: ru ? "Убрать из пула" : "Remove from pool", exact: true });
    await expect(remove).toBeVisible();
    await expect(card.getByText(badge, { exact: true })).toHaveCount(0);
    await expect.poll(() => card.evaluate((element) => element.scrollWidth <= element.clientWidth + 1)).toBe(true);
    await card.screenshot({ path: `output/playwright/image-pool-account-${scenario.locale}-${scenario.width}.png` });
    await remove.click();
    await expect(add).toBeVisible();

    await navigate(ru ? "Пул" : "Pool");
    await page.getByRole("button", { name: ru ? "Добавить участника" : "Add member", exact: true }).first().click();
    const dialog = page.getByRole("dialog", { name: ru ? "Добавить подключения в пул" : "Add connections to pool" });
    const row = dialog.locator(".pool-picker-option").filter({ hasText: "Personal Plus" });
    await expect(row.getByText(badge, { exact: true })).toBeVisible();
    await row.getByRole("checkbox").check();
    await expect.poll(() => dialog.evaluate((element) => element.scrollWidth <= element.clientWidth + 1)).toBe(true);
    await dialog.screenshot({ path: `output/playwright/image-pool-picker-${scenario.locale}-${scenario.width}.png` });
    await dialog.getByRole("button", { name: ru ? "Добавить выбранные (1)" : "Add selected (1)", exact: true }).click();
    await expect(dialog).toHaveCount(0);
    const member = page.locator(".pool-member-card").filter({ hasText: "Personal Plus" });
    await expect(member.getByText(badge, { exact: true })).toBeVisible();
    await member.screenshot({ path: `output/playwright/image-pool-member-${scenario.locale}-${scenario.width}.png` });
    await expect.poll(() => member.evaluate((element) => element.scrollWidth <= element.clientWidth + 1)).toBe(true);
    const calls = await page.evaluate(() => (window as unknown as {
      __TAURI_TEST_INVOKES__: Array<{ command: string; args: { input?: { accountIds: string[]; inPool: boolean } } }>;
    }).__TAURI_TEST_INVOKES__.filter((call) => call.command === "set_local_pool_membership"));
    expect(calls.map((call) => call.args.input?.inPool)).toEqual([true, false, true]);
    expect(calls.every((call) => call.args.input?.accountIds[0] === "account_synthetic")).toBe(true);
  });
}
