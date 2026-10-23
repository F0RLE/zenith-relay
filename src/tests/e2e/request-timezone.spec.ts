import { expect, test, type Page } from "../bun-playwright";
import { installTauriMock } from "./tauri-mock";
import { chooseOption, openGatewayApi } from "./operations/helpers";

async function openAccountSettings(page: Page, russian = false) {
  await page.getByRole("button", { name: russian ? "Доступ" : "Access", exact: true }).click();
  await page.getByRole("tab", { name: russian ? "Аккаунты" : "Accounts", exact: true }).click();
  await page.getByRole("tab", { name: "ChatGPT", exact: true }).click();
}

for (const mode of ["local"] as const) {
  test(`${mode} changing the default timezone source preserves account overrides`, async ({ page }, testInfo) => {
    await installTauriMock(page, { mode, locale: "en", populated: true, accountCount: 2 });
    await page.goto("/");
    await openAccountSettings(page);
    const source = page.getByRole("button", { name: /^Default request timezone:/ });
    await expect(source).toContainText("Assigned proxy");
    await chooseOption(page, page, "Default request timezone", "account");
    await expect(source).toContainText("Account setting");
    await openGatewayApi(page);
    await expect(source).toHaveCount(0);
    await page.getByRole("button", { name: "Connections", exact: true }).click();
    const card = page.locator(".account-card").filter({ hasText: "Personal Plus" });
    await card.locator(".account-row-menu summary").click();
    await page.getByRole("menuitem", { name: "Account timezone", exact: true }).click();
    const dialog = page.getByRole("dialog", { name: "Account timezone", exact: true });
    await expect(dialog.getByRole("radio", { name: "Auto", exact: true })).toBeChecked();
    await expect(dialog).toContainText("The default is the account setting.");
    await dialog.getByRole("radio", { name: "Set", exact: true }).click();
    await dialog.locator("label.account-timezone-option", { hasText: "Etc/GMT-3" }).click();
    await dialog.getByRole("button", { name: "Save", exact: true }).click();
    await expect(dialog).not.toBeVisible();
    await openAccountSettings(page);
    await chooseOption(page, page, "Default request timezone", "proxy");
    await expect(source).toContainText("Assigned proxy");
    await page.getByRole("button", { name: "Connections", exact: true }).click();
    await card.locator(".account-row-menu summary").click();
    await page.getByRole("menuitem", { name: "Account timezone", exact: true }).click();
    await expect(dialog.getByRole("radio", { name: /Etc\/GMT-3/ })).toBeChecked();
    await expect(dialog.getByRole("radio", { name: "Set", exact: true })).toBeChecked();
    await dialog.getByRole("button", { name: "Cancel", exact: true }).click();
    await page.locator(".account-card").filter({ hasText: "Business Workspace" }).locator(".account-row-menu summary").click();
    await page.getByRole("menuitem", { name: "Account timezone", exact: true }).click();
    await expect(dialog.getByRole("radio", { name: "Auto", exact: true })).toBeChecked();
    await expect(dialog).toContainText("The default is the assigned proxy's timezone.");
    await dialog.getByRole("radio", { name: "Set", exact: true }).click();
    await expect(dialog.getByRole("radio", { name: /America\/New_York/ })).toBeChecked();
    await page.screenshot({ path: testInfo.outputPath("timezone-dialog.png") });
  });
}


test("Russian timezone settings fit a narrow dark window", async ({ page }, testInfo) => {
  await page.setViewportSize({ width: 640, height: 900 });
  await installTauriMock(page, { mode: "local", locale: "ru", theme: "dark", populated: true, accountCount: 2 });
  await page.goto("/");
  await openAccountSettings(page, true);
  const source = page.getByRole("button", { name: /^Часовой пояс по умолчанию:/ });
  await expect(source).toContainText("Назначенный прокси");
  await expect(source).toBeInViewport();
  await page.getByRole("button", { name: "Подключения", exact: true }).click();
  await page.locator(".account-card").filter({ hasText: "Personal Plus" }).locator(".account-row-menu summary").click();
  await page.getByRole("menuitem", { name: "Часовой пояс аккаунта", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "Часовой пояс аккаунта", exact: true });
  await dialog.getByRole("radio", { name: "Задать", exact: true }).click();
  await dialog.locator("label.account-timezone-option", { hasText: "Etc/GMT-3" }).click();
  await expect(dialog.getByRole("button", { name: "Сохранить", exact: true })).toBeEnabled();
  const width = await dialog.evaluate((element) => ({
    scroll: element.scrollWidth, client: element.clientWidth,
  }));
  expect(width.scroll).toBeLessThanOrEqual(width.client + 1);
  await page.screenshot({ path: testInfo.outputPath("timezone-dialog-ru-narrow.png") });
});
