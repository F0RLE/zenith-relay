import { expect, test } from "../bun-playwright";
import { installTauriMock } from "./tauri-mock";

for (const theme of ["light", "dark"] as const) {
  test(`compact dialogs remain usable in ${theme} theme`, async ({ page }, testInfo) => {
    await page.setViewportSize({ width: 390, height: 844 });
    await installTauriMock(page, { locale: "ru", mode: "local", theme, populated: true });
    await page.goto("/");
    await page.getByRole("button", { name: "Подключения", exact: true }).click();
    const checkDialog = async (label: string) => {
      const dialog = page.getByRole("dialog");
      await expect(dialog).toBeVisible();
      const bounds = await dialog.evaluate((element) => {
        const rect = element.getBoundingClientRect();
        return {
          fits: rect.left >= 0 && rect.right <= innerWidth && rect.top >= 36 && rect.bottom <= innerHeight,
          bodyFits: element.querySelector(".relay-dialog-body")!.scrollWidth <= element.querySelector(".relay-dialog-body")!.clientWidth + 1,
        };
      });
      expect(bounds).toEqual({ fits: true, bodyFits: true });
      await page.screenshot({ path: testInfo.outputPath(`${label}.png`) });
      await page.locator(".relay-modal-backdrop").click({ position: { x: 2, y: 2 } });
      await expect(dialog).toHaveCount(0);
    };

    await page.getByRole("tab", { name: "Источники API" }).click();
    await page.getByRole("button", { name: "Добавить источник" }).click();
    await checkDialog("source-add");

    await page.getByRole("tab", { name: "Учётные записи", exact: true }).click();
    await page.locator(".account-card").first().locator(".account-row-menu summary").click();
    await page.getByRole("menuitem", { name: /^Прокси:/ }).click();
    await checkDialog("proxy-choice");

    await page.locator(".account-bulk-menu summary").click();
    await page.getByRole("menuitem", { name: /Экспортировать все/ }).click();
    await checkDialog("export");
  });
}

test("transient menus close when the user clicks outside", async ({ page }) => {
  await installTauriMock(page, { locale: "en", mode: "local", theme: "light", populated: true });
  await page.goto("/");

  const modeTrigger = page.locator(".mode-picker > button");
  await modeTrigger.click();
  const modeMenu = page.getByRole("menu");
  await expect(modeMenu).toBeVisible();
  await page.mouse.click(420, 120);
  await expect(modeMenu).toBeHidden();

  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await page.getByRole("tab", { name: "Sources", exact: true }).click();
  const actionMenu = page.locator(".relay-action-menu").first();
  await actionMenu.locator("summary").click();
  await expect(actionMenu).toHaveAttribute("open", "");
  await page.mouse.click(420, 120);
  await expect(actionMenu).not.toHaveAttribute("open", "");

  await page.getByRole("tab", { name: "Automations", exact: true }).click();
  await page.getByRole("button", { name: "Edit", exact: true }).click();
  const automationDialog = page.getByRole("dialog", { name: "Edit automation" });
  await automationDialog.getByRole("button", { name: /^Accounts:/ }).click();
  const accountList = page.getByRole("listbox", { name: "Accounts" });
  await expect(accountList).toBeVisible();
  await page.mouse.click(420, 120);
  await expect(accountList).toBeHidden();
  await page.keyboard.press("Escape");
  await expect(automationDialog).toBeHidden();

  await page.getByRole("button", { name: "Pool", exact: true }).click();
  await page.locator(".mode-picker > button").click();
  await page.keyboard.press("Escape");
  await expect(page.locator(".mode-picker > button")).toBeFocused();
  await expect(page.getByRole("menu")).toBeHidden();
});

test("context menu closes outside and keeps its keyboard contract", async ({ page }) => {
  await installTauriMock(page, { locale: "en", mode: "local", theme: "dark", populated: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  const search = page.getByPlaceholder("Search").first();
  await search.fill("Business");
  await search.evaluate((element) => {
    const input = element as HTMLInputElement;
    input.setSelectionRange(0, 4);
    const rect = input.getBoundingClientRect();
    input.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true, clientX: rect.left + 20, clientY: rect.bottom }));
  });
  const menu = page.getByRole("menu", { name: "Context menu" });
  await expect(menu).toBeVisible();
  await page.mouse.click(420, 120);
  await expect(menu).toBeHidden();

  await search.evaluate((element) => {
    const input = element as HTMLInputElement;
    const rect = input.getBoundingClientRect();
    input.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true, clientX: rect.left + 20, clientY: rect.bottom }));
  });
  await expect(menu).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(menu).toBeHidden();
  await expect(search).toBeFocused();
});

test("mobile help contents and usage hint close outside", async ({ page }) => {
  await installTauriMock(page, { locale: "en", mode: "local", theme: "light", populated: true });
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto("/");
  await page.getByRole("button", { name: "Help", exact: true }).click();
  const contentsToggle = page.locator(".help-contents-toggle");
  await contentsToggle.click();
  await expect(contentsToggle).toHaveAttribute("aria-expanded", "true");
  await page.mouse.click(16, 720);
  await expect(contentsToggle).toHaveAttribute("aria-expanded", "false");

  await page.getByRole("button", { name: "Usage", exact: true }).click();
  await page.locator(".usage-account-menu").getByRole("button").click();
  await page.getByRole("option", { name: "Personal Plus", exact: true }).click();
  const accountSummary = page.locator(".usage-account-value");
  await accountSummary.locator("summary").click();
  await expect(accountSummary.locator("details")).toHaveAttribute("open", "");
  await page.mouse.click(16, 720);
  await expect(accountSummary.locator("details")).not.toHaveAttribute("open", "");
});
