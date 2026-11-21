import { expect, test, type Page } from "../bun-playwright";
import { installTauriMock } from "./tauri-mock";
import { chooseOption, openGatewayApi, openGatewayApplication } from "./operations/helpers";

type PendingSettingWindow = Window & {
  __TAURI_INTERNALS__: { invoke: (command: string, args?: unknown, options?: unknown) => Promise<unknown> };
  pendingSettingCalls: number;
  finishSettingSave?: () => void;
};

async function holdSettingSave(page: Page, command: string, fails: boolean) {
  await page.evaluate(({ target, fails }) => {
    const mock = window as PendingSettingWindow;
    const original = mock.__TAURI_INTERNALS__.invoke.bind(mock.__TAURI_INTERNALS__);
    mock.pendingSettingCalls = 0;
    mock.__TAURI_INTERNALS__.invoke = async (command, args, options) => {
      if (command === target) {
        mock.pendingSettingCalls += 1;
        await new Promise<void>((resolve) => { mock.finishSettingSave = resolve; });
        if (fails) throw { code: "storage_error", message: "Synthetic setting save failure" };
      }
      return original(command, args, options);
    };
  }, { target: command, fails });
}

const settings = [
  { label: "Allow background tasks", command: "set_local_codex_background_tasks", open: openGatewayApplication },
  { label: "Wait for route recovery", command: "set_local_chatgpt_retry_until_available", open: openGatewayApi },
  {
    label: "Debug mode", command: "set_diagnostic_debug_mode",
    open: async (page: Page) => { await page.getByRole("button", { name: "Settings", exact: true }).click(); },
  },
  {
    label: "Enabled", command: "set_quota_wake_automation_enabled",
    open: async (page: Page) => {
      await page.getByRole("button", { name: "Connections", exact: true }).click();
      await page.getByRole("tab", { name: "Automations", exact: true }).click();
    },
  },
  {
    label: "Keep 1% reserved", command: "update_chatgpt_interface_quota_reserve",
    open: async (page: Page) => {
      await openGatewayApplication(page);
      await chooseOption(page, page.locator(".client-setup"), "Account", "account_synthetic");
    },
  },
];

for (const setting of settings) {
  for (const fails of [false, true]) {
    test(`${setting.label} prevents overlapping saves and restores acknowledged state fail=${fails}`, async ({ page }) => {
      await installTauriMock(page, { mode: "local", locale: "en", populated: true });
      await page.goto("/");
      await setting.open(page);
      const toggle = page.getByRole("checkbox", { name: setting.label, exact: true });
      await expect(toggle).toBeEnabled();
      const original = await toggle.isChecked();
      await holdSettingSave(page, setting.command, fails);
      await toggle.click();
      await expect.poll(() => page.evaluate(() => Boolean((window as PendingSettingWindow).finishSettingSave))).toBe(true);
      await expect(toggle).toBeDisabled();
      await expect(toggle).toBeChecked({ checked: !original });
      await toggle.evaluate((element: HTMLInputElement) => element.click());
      expect(await page.evaluate(() => (window as PendingSettingWindow).pendingSettingCalls)).toBe(1);
      await page.evaluate(() => (window as PendingSettingWindow).finishSettingSave!());
      await expect(toggle).toBeEnabled();
      await expect(toggle).toBeChecked({ checked: fails ? original : !original });
      await page.getByRole("button", { name: "Usage", exact: true }).click();
      await setting.open(page);
      await expect(toggle).toBeChecked({ checked: fails ? original : !original });
    });
  }
}

test("account availability action stays locked until its own save settles", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  const menu = page.locator(".account-card .account-row-menu summary").first();
  await holdSettingSave(page, "set_local_account_enabled", false);
  await menu.click();
  await page.getByRole("menuitem", { name: "Disable", exact: true }).click();
  await expect.poll(() => page.evaluate(() => Boolean((window as PendingSettingWindow).finishSettingSave))).toBe(true);
  await menu.click();
  const enable = page.getByRole("menuitem", { name: "Enable", exact: true });
  await expect(enable).toBeDisabled();
  expect(await page.evaluate(() => (window as PendingSettingWindow).pendingSettingCalls)).toBe(1);
  await page.evaluate(() => (window as PendingSettingWindow).finishSettingSave!());
  await expect(enable).toBeEnabled();
});
