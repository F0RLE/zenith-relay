import { expect, test } from "../../bun-playwright";
import { installTauriMock } from "../tauri-mock";

for (const mode of ["local"] as const) {
  test(`${mode} account trash retains accounts across navigation and restores them outside the pool`, async ({ page }) => {
    await installTauriMock(page, { mode, locale: "en", populated: true, accountCount: 2, accountOAuthClientKind: "excel_bps" });
    await page.goto("/");
    await page.getByRole("button", { name: "Connections", exact: true }).click();
    await page.getByLabel("Select all accounts").check();
    await page.getByRole("button", { name: "Delete selected accounts" }).click();
    await expect(page.getByRole("dialog")).toHaveCount(0);
    await expect(page.getByText("No accounts", { exact: true })).toBeVisible();
    await page.getByRole("tab", { name: "Trash", exact: true }).click();
    await expect(page.locator(".account-trash-view [role=listitem]")).toHaveCount(2);
    await page.getByRole("tab", { name: "Accounts", exact: true }).click();
    await page.getByRole("tab", { name: "Trash", exact: true }).click();
    await expect(page.locator(".account-trash-view [role=listitem]")).toHaveCount(2);
    await page.getByLabel("Select all accounts in trash").check();
    await page.getByRole("button", { name: "Restore", exact: true }).first().click();
    await expect(page.getByText("Trash is empty", { exact: true })).toBeVisible();
    await page.getByRole("tab", { name: "Accounts", exact: true }).click();
    await expect(page.locator(".account-card")).toHaveCount(2);
    const accounts = await page.evaluate(() => {
      const snapshots = (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: Record<string, unknown> }> }).__TAURI_TEST_INVOKES__;
      return snapshots.filter((entry) => entry.command === "delete_local_accounts" || entry.command === "delete_local_account");
    });
    expect(accounts).toEqual([]);
    await expect(page.locator(".account-card").first().getByRole("button", { name: "Add to pool", exact: true })).toBeVisible();
  });
}

test("right-click account deletion confirms permanent deletion and cancel preserves the account", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  const card = page.locator(".account-card").first();
  await card.locator(".account-row-menu summary").click();
  await card.getByRole("menuitem", { name: "Delete", exact: true }).click({ button: "right" });
  await page.getByRole("menu", { name: "Context menu", exact: true }).getByRole("menuitem", { name: "Delete permanently", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "Delete permanently", exact: true });
  await expect(dialog).toContainText("They cannot be restored from trash.");
  await dialog.getByRole("button", { name: "Cancel", exact: true }).click();
  await expect(card).toBeVisible();
  await card.locator(".account-row-menu summary").click();
  await card.getByRole("menuitem", { name: "Delete", exact: true }).click({ button: "right" });
  await page.getByRole("menu", { name: "Context menu", exact: true }).getByRole("menuitem", { name: "Delete permanently", exact: true }).click();
  await dialog.getByRole("button", { name: "Delete permanently", exact: true }).click();
  await expect(page.getByText("No accounts", { exact: true })).toBeVisible();
  await page.getByRole("tab", { name: "Trash", exact: true }).click();
  await expect(page.getByText("Trash is empty", { exact: true })).toBeVisible();
});

test("permanent deletion from trash confirms before removing retained accounts", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await page.getByLabel("Select all accounts").check();
  await page.getByRole("button", { name: "Delete selected accounts" }).click();
  await page.getByRole("tab", { name: "Trash", exact: true }).click();
  const card = page.locator(".account-trash-view [role=listitem]").first();
  await card.getByRole("button", { name: "Delete permanently", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "Delete permanently", exact: true });
  await dialog.getByRole("button", { name: "Cancel", exact: true }).click();
  await expect(card).toBeVisible();
  await card.getByRole("button", { name: "Delete permanently", exact: true }).click();
  await dialog.getByRole("button", { name: "Delete permanently", exact: true }).click();
  await expect(page.getByText("Trash is empty", { exact: true })).toBeVisible();
});


test("right-click bulk deletion confirms before deleting permanently", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, accountCount: 2 });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await page.getByLabel("Select all accounts").check();
  await page.getByRole("button", { name: "Delete selected accounts" }).click({ button: "right" });
  await page.getByRole("menu", { name: "Context menu", exact: true }).getByRole("menuitem", { name: "Delete permanently", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "Delete permanently", exact: true });
  await expect(dialog).toContainText("(2)");
  await dialog.getByRole("button", { name: "Delete permanently", exact: true }).click();
  await expect(page.getByText("No accounts", { exact: true })).toBeVisible();
  await page.getByRole("tab", { name: "Trash", exact: true }).click();
  await expect(page.getByText("Trash is empty", { exact: true })).toBeVisible();
});

for (const width of [760, 1440]) {
  test(`Russian trash remains usable at ${width}px`, async ({ page }) => {
    await page.setViewportSize({ width, height: 900 });
    await installTauriMock(page, { mode: "local", locale: "ru", theme: "dark", populated: true, accountCount: 3 });
    await page.goto("/");
    await page.getByRole("button", { name: "Подключения", exact: true }).click();
    await page.getByLabel("Выбрать все учётные записи").check();
    await page.getByRole("button", { name: "Удалить выбранные записи", exact: true }).click();
    await page.getByRole("tab", { name: "Корзина", exact: true }).click();
    await expect(page.locator(".account-trash-view [role=listitem]")).toHaveCount(3);
    await page.getByLabel("Выбрать все учётные записи в корзине").check();
    await expect(page.getByRole("button", { name: "Восстановить", exact: true }).first()).toBeVisible();
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    await page.screenshot({ path: `output/playwright/account-trash-ru-dark-${width}.png` });
  });
}
