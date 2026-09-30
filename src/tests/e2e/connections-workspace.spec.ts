import { expect, test, type Page } from "../bun-playwright";
import { installTauriMock } from "./tauri-mock";

async function openConnections(page: Page) {
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await expect(page.locator("#splash-screen")).toHaveCount(0);
}

async function commands(page: Page) {
  return page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: Record<string, unknown> }> }).__TAURI_TEST_INVOKES__);
}

test("proxy action cards disable unavailable choices without applying changes", async ({ page }) => {
  await installTauriMock(page, { populated: true, accountCount: 3, proxyCount: 0, accountProxyRequired: true });
  await openConnections(page);
  await page.locator(".account-card").filter({ hasText: "Backup account" }).locator(".account-row-menu summary").click();
  await page.getByRole("menuitem", { name: "Proxy: No proxy", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "Account proxy" });
  const choices = dialog.getByRole("radiogroup", { name: "Account route" });
  await expect(choices.getByRole("radio")).toHaveCount(5);
  await expect(dialog.getByRole("button", { name: "Save", exact: true })).toBeDisabled();
  for (const name of ["No proxy", "Assign automatically", "Choose from storage"]) {
    await expect(choices.getByRole("radio", { name: new RegExp(`^${name}`) })).toBeDisabled();
  }
  const custom = choices.getByRole("radio", { name: /^Add a new proxy/ });
  await custom.focus();
  await page.keyboard.press("Enter");
  await expect(custom).toBeChecked();
  await expect(dialog.getByLabel("HTTP(S) proxy")).toBeVisible();
  await expect(dialog.getByRole("button", { name: "Save", exact: true })).toBeDisabled();
  expect((await commands(page)).some((call) => call.command === "set_local_account_proxy")).toBe(false);
});

test("proxy action cards retain a selected stored endpoint before saving", async ({ page }) => {
  await installTauriMock(page, { populated: true, proxyCount: 2 });
  await openConnections(page);
  await page.locator(".account-card").first().locator(".account-row-menu summary").click();
  await page.getByRole("menuitem", { name: "Proxy: Common", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "Account proxy" });
  const chooseAction = async (name: string) => {
    await dialog.getByRole("radio", { name: new RegExp(`^${name}`) }).click();
  };
  await chooseAction("Choose from storage");
  await dialog.getByRole("button", { name: /^Choose from storage:/ }).click();
  await page.getByRole("listbox").locator('[data-value="proxy_synthetic_2"]').click();
  await chooseAction("No proxy");
  await expect(dialog.getByRole("button", { name: /^Choose from storage:/ })).toHaveCount(0);
  await chooseAction("Choose from storage");
  await expect(dialog.getByRole("button", { name: /^Choose from storage:/ })).toHaveAttribute("data-value", "proxy_synthetic_2");
  expect((await commands(page)).some((call) => call.command === "assign_local_stored_proxy")).toBe(false);
  await dialog.getByRole("button", { name: "Save", exact: true }).click();
  await expect(dialog).toBeHidden();
  expect((await commands(page)).findLast((call) => call.command === "assign_local_stored_proxy")?.args).toEqual({ input: { accountId: "account_synthetic", proxyId: "proxy_synthetic_2" } });
});

test("proxy checks are explicit, show observed egress, and keep account assignments", async ({ page }) => {
  await installTauriMock(page, { populated: true, proxyCount: 2, proxyCheckDelayMs: 500 });
  await openConnections(page);
  await page.getByRole("tab", { name: "Proxies" }).click();
  const row = page.locator(".proxy-storage-row").first();
  await expect(row).toContainText("Declared: United States");
  await expect(row).toContainText("Not checked yet");
  expect((await commands(page)).some((call) => call.command === "check_local_stored_proxy")).toBe(false);
  await row.getByRole("button", { name: "Test proxy", exact: true }).hover();
  await expect(page.getByRole("tooltip")).toHaveText("Test proxy");
  await row.getByRole("button", { name: "Test proxy", exact: true }).click();
  await expect(row.getByRole("button", { name: "Test proxy", exact: true })).toBeDisabled();
  await expect(row).toContainText("Checking connection");
  await expect(row).toContainText("203.0.113.42");
  await expect(row).toContainText("Netherlands");
  await expect(row).toContainText("184 ms");
  const calls = await commands(page);
  expect(calls.filter((call) => call.command === "check_local_stored_proxy")).toEqual([{ command: "check_local_stored_proxy", args: { proxyId: "proxy_synthetic_1" } }]);
  expect(calls.some((call) => ["set_local_account_proxy", "set_local_stored_proxy_accounts", "start_local_gateway"].includes(call.command))).toBe(false);
  await page.getByRole("tab", { name: "Sources" }).click();
  await page.getByRole("tab", { name: "Proxies" }).click();
  await expect(page.locator(".proxy-storage-row").first()).toContainText("203.0.113.42");
});

test("import checks only new proxies and retains them after a failed check", async ({ page }) => {
  await installTauriMock(page, { populated: true, proxyCount: 0, proxyCheckError: "proxy_check_timeout" });
  await openConnections(page);
  await page.getByRole("tab", { name: "Proxies" }).click();
  await page.getByRole("button", { name: "Import", exact: true }).first().click();
  const dialog = page.getByRole("dialog", { name: "Import proxies" });
  await dialog.getByLabel("Proxy list").fill("proxy-new.example.test:1234:user:synthetic-password\nproxy-new.example.test:1234:user:synthetic-password");
  await expect(dialog.getByLabel("Check after adding")).toBeChecked();
  await dialog.getByRole("button", { name: "Import 2", exact: true }).click();
  await expect(dialog).toContainText("Added 1; skipped 1 duplicate(s).");
  await expect(dialog).toContainText("Check failed");
  await expect(dialog).toContainText("No response within 12 seconds");
  await expect(dialog).not.toContainText("synthetic-password");
  await dialog.getByRole("button", { name: "Done", exact: true }).click();
  await expect(page.locator(".proxy-storage-row")).toHaveCount(1);
  await expect(page.locator(".proxy-storage-row")).toContainText("Check failed");
  expect((await commands(page)).filter((call) => call.command === "check_local_stored_proxy")).toHaveLength(1);
});

test("proxy import can skip checks and complete without diagnostic requests", async ({ page }) => {
  await installTauriMock(page, { populated: true, proxyCount: 0 });
  await openConnections(page);
  await page.getByRole("tab", { name: "Proxies" }).click();
  await page.getByRole("button", { name: "Import", exact: true }).first().click();
  const dialog = page.getByRole("dialog", { name: "Import proxies" });
  await dialog.getByLabel("Proxy list").fill("proxy-new.example.test:1234:user:synthetic-password");
  await dialog.getByLabel("Check after adding").uncheck();
  await dialog.getByRole("button", { name: "Import 1", exact: true }).click();
  await expect(dialog).toContainText("Not checked yet");
  expect((await commands(page)).some((call) => call.command === "check_local_stored_proxy")).toBe(false);
});

async function expectDialogFits(page: Page) {
  const dialog = page.getByRole("dialog");
  expect(await dialog.evaluate((element) => {
    const rect = element.getBoundingClientRect();
    const body = element.querySelector(".relay-dialog-body")!;
    const footer = element.querySelector("footer")!.getBoundingClientRect();
    return rect.left >= 0 && rect.right <= innerWidth && rect.top >= 34 && rect.bottom <= innerHeight
      && body.scrollWidth <= body.clientWidth && footer.bottom <= innerHeight;
  })).toBe(true);
}

for (const theme of ["light", "dark"] as const) {
  for (const viewport of [{ width: 1160, height: 760 }, { width: 840, height: 560 }, { width: 390, height: 844 }]) {
    test(`connections workspace ${theme} ${viewport.width}`, async ({ page }) => {
      await installTauriMock(page, { locale: "ru", theme, populated: true, sourceCount: 2, proxyCount: 3, accountCount: 3 });
      await page.setViewportSize(viewport);
      await page.goto("/");
      await page.getByRole("button", { name: "Подключения", exact: true }).click();
      await expect(page.locator("#splash-screen")).toHaveCount(0);
      const capture = async (name: string) => {
        await page.mouse.move(1, 1);
        await page.screenshot({ path: `output/playwright/connections-${name}-${theme}-${viewport.width}.png`, animations: "disabled" });
      };
      await page.locator(".account-card").first().locator(".account-row-menu summary").click();
      await page.getByRole("menuitem", { name: /^Прокси:/ }).click();
      await expectDialogFits(page);
      await expect(page.getByRole("radiogroup", { name: "Маршрут аккаунта" })).toBeVisible();
      await capture("proxy-account");
      await page.getByRole("dialog").getByRole("button", { name: "Отмена", exact: true }).click();
      await page.getByRole("tab", { name: "Источники API", exact: true }).click();
      expect(await page.locator(".connection-list-wrap").evaluate((element) => element.scrollWidth <= element.clientWidth)).toBe(true);
      await capture("sources");
      await page.locator(".source-table tbody tr").first().getByRole("button", { name: "Изменить", exact: true }).click();
      await expectDialogFits(page);
      await capture("source-editor");
      await page.getByRole("dialog").getByRole("tab").nth(1).click();
      await page.locator(".source-price-group > summary").first().click();
      await expectDialogFits(page);
      await capture("source-prices");
      await page.getByRole("dialog").getByRole("button", { name: "Отмена", exact: true }).click();
      await page.getByRole("tab", { name: "Прокси", exact: true }).click();
      await page.locator(".proxy-storage-row").first().getByRole("button", { name: "Проверить прокси", exact: true }).click();
      await expect(page.locator(".proxy-storage-row").first()).toContainText("203.0.113.42");
      expect(await page.locator(".proxy-storage-row").evaluateAll((rows) => rows.every((row) => row.scrollWidth <= row.clientWidth))).toBe(true);
      await capture("proxies");
      await page.getByRole("button", { name: "Управлять привязанными аккаунтами" }).first().click();
      await expectDialogFits(page);
      await capture("proxy-accounts");
      await page.getByRole("dialog").getByRole("button", { name: "Отмена", exact: true }).click();
      await page.getByRole("button", { name: "Импортировать", exact: true }).click();
      await expectDialogFits(page);
      await capture("proxy-import");
      await page.getByRole("dialog").getByLabel("Список прокси").fill("http://proxy-new.example.test:1234");
      await page.getByRole("dialog").getByRole("button", { name: /Импортировать.*1/ }).click();
      await expect(page.getByRole("dialog")).toContainText("203.0.113.42");
      await expectDialogFits(page);
      await capture("proxy-result");
      await page.getByRole("dialog").getByRole("button", { name: "Готово", exact: true }).click();
      await page.getByRole("tab", { name: "Автоматизация", exact: true }).click();
      await expect(page.locator(".connections-toolbar")).toHaveCount(0);
      expect(await page.locator(".connection-list-wrap").evaluate((element) => element.scrollWidth <= element.clientWidth)).toBe(true);
      await expect(page.getByRole("button", { name: "Запустить готовые проверки" })).toHaveCount(0);
      await expect(page.locator(".automation-list").getByRole("button", { name: "Проверить" })).toHaveCount(0);
      await capture("automations");
      await page.getByRole("button", { name: "Добавить автоматизацию", exact: true }).click();
      await expectDialogFits(page);
      await expect(page.getByRole("dialog").getByRole("button", { name: /^(Автоматически|Вручную)$/ })).toHaveCount(0);
      await capture("automation-editor");
      await page.getByRole("dialog").screenshot({ path: `output/playwright/automation-type-dialog-${theme}-${viewport.width}.png`, animations: "disabled" });
      await page.getByRole("dialog").getByRole("button", { name: /^Тип автоматизации:/ }).click();
      await page.getByRole("option", { name: "Сбросить недельную квоту", exact: true }).click();
      await expectDialogFits(page);
      await expect(page.getByRole("dialog").getByRole("button", { name: /^Модель:/ })).toHaveCount(0);
      await capture("automation-weekly-editor");
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    });
  }
}
