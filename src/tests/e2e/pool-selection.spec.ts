import { expect, test, type Page } from "../bun-playwright";
import { installTauriMock } from "./tauri-mock";

async function openPool(page: Page, russian = false) {
  await page.goto("/");
  await page.getByRole("button", { name: russian ? "Пул" : "Pool", exact: true }).click();
  await expect(page.locator("#splash-screen")).toHaveCount(0);
}

async function calls(page: Page, command: string) {
  return page.evaluate((name) => (window as unknown as {
    __TAURI_TEST_INVOKES__: Array<{ command: string; args: Record<string, unknown> }>;
  }).__TAURI_TEST_INVOKES__.filter((call) => call.command === name), command);
}

async function expectFailure(page: Page, message: string) {
  await page.getByRole("button", { name: "Show details", exact: true }).click();
  await expect(page.getByRole("dialog")).toContainText(message);
  await page.getByRole("dialog").getByRole("button", { name: "Close", exact: true }).last().click();
}

test("pool selection refreshes and toggles only selected accounts and API sources", async ({ page }) => {
  await installTauriMock(page, { populated: true, accountCount: 3, sourceCount: 2 });
  await openPool(page);
  const toolbar = page.locator(".pool-member-toolbar");
  const account = page.locator('.pool-member-card[data-member-kind="account"]').first();
  const source = page.locator('.pool-member-card[data-member-kind="source"]').first();
  await account.locator(".account-select-button").click();
  await source.locator(".account-select-button").click();
  await expect(toolbar).toContainText("Selected: 2");
  await expect(page.getByRole("dialog")).toHaveCount(0);
  await expect(toolbar.getByRole("checkbox")).toHaveJSProperty("indeterminate", true);
  await toolbar.getByRole("button", { name: "Refresh selected", exact: true }).click();
  await expect(toolbar.getByRole("button", { name: "Refresh selected", exact: true })).toBeEnabled();
  expect((await calls(page, "refresh_local_account_quota")).map((call) => call.args.accountId)).toEqual(["account_synthetic"]);
  expect((await calls(page, "refresh_local_source_data")).map((call) => call.args.sourceId)).toEqual(["source_synthetic"]);
  expect(await calls(page, "refresh_all_local_account_quotas")).toHaveLength(0);
  expect((await calls(page, "get_local_source_stats")).filter((call) => call.args.force).map((call) => call.args.sourceId)).toEqual(["source_synthetic"]);
  await expect(toolbar.getByRole("button", { name: "Enable", exact: true })).toBeDisabled();
  await toolbar.getByRole("button", { name: "Disable", exact: true }).click();
  await expect(toolbar.getByRole("button", { name: "Disable", exact: true })).toBeDisabled();
  await expect(toolbar.getByRole("button", { name: "Enable", exact: true })).toBeEnabled();
  expect((await calls(page, "set_local_account_enabled")).map((call) => call.args)).toEqual([{ accountId: "account_synthetic", enabled: false }]);
  expect((await calls(page, "set_local_source_enabled")).map((call) => call.args)).toEqual([{ sourceId: "source_synthetic", enabled: false }]);
  await toolbar.getByRole("button", { name: "Enable", exact: true }).click();
  await expect(toolbar.getByRole("button", { name: "Enable", exact: true })).toBeDisabled();
  await expect(page.locator('.pool-member-card[data-selected="true"]')).toHaveCount(2);
  await toolbar.getByRole("button", { name: "Clear selection", exact: true }).click();
  await expect(toolbar.getByRole("checkbox")).not.toBeChecked();
  await expect(toolbar.getByRole("checkbox")).toHaveJSProperty("indeterminate", false);
  await expect(page.locator('.pool-member-card[data-selected="true"]')).toHaveCount(0);
});

test("bulk pool removal confirms intent and keeps connections and credentials", async ({ page }) => {
  await installTauriMock(page, { populated: true, accountCount: 2, sourceCount: 2 });
  await openPool(page);
  const toolbar = page.locator(".pool-member-toolbar");
  await page.locator('.pool-member-card[data-member-kind="account"]').first().locator(".account-select-button").click();
  await page.locator('.pool-member-card[data-member-kind="source"]').first().locator(".account-select-button").click();
  await toolbar.getByRole("button", { name: "Remove from pool", exact: true }).click();
  await expect(page.getByRole("dialog")).toContainText("Accounts and API sources will be kept in Connections");
  await page.getByRole("dialog").getByRole("button", { name: "Cancel", exact: true }).click();
  expect(await calls(page, "set_local_pool_membership")).toHaveLength(0);
  await expect(page.locator('.pool-member-card[data-selected="true"]')).toHaveCount(2);
  await toolbar.getByRole("button", { name: "Remove from pool", exact: true }).click();
  await page.getByRole("dialog").getByRole("button", { name: "Remove from pool", exact: true }).click();
  await expect(page.locator(".pool-member-card")).toHaveCount(2);
  await expect(page.locator('.pool-member-card[data-selected="true"]')).toHaveCount(0);
  expect((await calls(page, "set_local_pool_membership")).map((call) => call.args.input)).toEqual([
    { accountIds: ["account_synthetic"], sourceIds: ["source_synthetic"], inPool: false },
  ]);
  for (const command of ["delete_local_account", "delete_local_accounts", "delete_local_source"]) expect(await calls(page, command)).toHaveLength(0);
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await expect(page.locator(".account-card")).toHaveCount(2);
  await page.getByRole("tab", { name: "Sources", exact: true }).click();
  await expect(page.locator(".source-card")).toHaveCount(2);
});

test("failed bulk disable keeps selection and displays the successful partial state", async ({ page }) => {
  await installTauriMock(page, { populated: true, accountCount: 2, sourceCount: 0 });
  await openPool(page);
  await page.evaluate(() => {
    const scope = window as unknown as { __TAURI_INTERNALS__: { invoke: (command: string, args?: Record<string, unknown>, options?: unknown) => Promise<unknown> } };
    const invoke = scope.__TAURI_INTERNALS__.invoke.bind(scope.__TAURI_INTERNALS__);
    let failed = false;
    scope.__TAURI_INTERNALS__.invoke = async (command, args, options) => {
      if (command === "set_local_account_enabled" && args?.accountId === "account_synthetic_2" && !failed) {
        failed = true;
        throw { code: "conflict", message: "Synthetic bulk failure" };
      }
      return invoke(command, args, options);
    };
  });
  const toolbar = page.locator(".pool-member-toolbar");
  for (const select of await page.locator('.pool-member-card[data-member-kind="account"] .account-select-button').all()) await select.click();
  await toolbar.getByRole("button", { name: "Disable", exact: true }).click();
  await expectFailure(page, "Synthetic bulk failure");
  await expect(toolbar).toContainText("Selected: 2");
  await expect(toolbar.getByRole("button", { name: "Enable", exact: true })).toBeEnabled();
  await expect(toolbar.getByRole("button", { name: "Disable", exact: true })).toBeEnabled();
  await toolbar.getByRole("button", { name: "Disable", exact: true }).click();
  await expect(toolbar.getByRole("button", { name: "Disable", exact: true })).toBeDisabled();
  expect((await calls(page, "set_local_account_enabled")).map((call) => call.args.accountId)).toEqual(["account_synthetic", "account_synthetic_2"]);
});

test("refresh failure retains selection and still refreshes selected accounts", async ({ page }) => {
  await installTauriMock(page, { populated: true, sourceRefreshError: true });
  await openPool(page);
  const toolbar = page.locator(".pool-member-toolbar");
  await toolbar.getByRole("checkbox").check();
  await toolbar.getByRole("button", { name: "Refresh selected", exact: true }).click();
  await expectFailure(page, "Source models unavailable");
  await expect(toolbar.getByRole("checkbox")).toBeChecked();
  expect(await calls(page, "refresh_local_account_quota")).toHaveLength(1);
  await expect(toolbar.getByRole("button", { name: "Refresh selected", exact: true })).toBeEnabled();
});

test("selected SIWC refresh uses the linked quota connection", async ({ page }) => {
  await installTauriMock(page, { populated: true, sourceCount: 0, accountRouteKinds: ["siwc", "excel_bps"], accountRoutesGrouped: true });
  await openPool(page);
  await expect(page.locator('.pool-member-card[data-member-kind="account"]')).toHaveCount(1);
  const toolbar = page.locator(".pool-member-toolbar");
  await page.locator('.pool-member-card[data-member-kind="account"] .account-select-button').click();
  await toolbar.getByRole("button", { name: "Refresh selected", exact: true }).click();
  await expect(toolbar.getByRole("button", { name: "Refresh selected", exact: true })).toBeEnabled();
  expect((await calls(page, "refresh_local_account_quota")).map((call) => call.args.accountId)).toEqual(["account_synthetic_2"]);
});

test("bulk removal locks selection while saving and retains it on failure", async ({ page }) => {
  await installTauriMock(page, { populated: true });
  await openPool(page);
  await page.evaluate(() => {
    const scope = window as unknown as {
      __TAURI_INTERNALS__: { invoke: (command: string, args?: Record<string, unknown>, options?: unknown) => Promise<unknown> };
      __FAIL_POOL_REMOVE__: () => void;
    };
    const invoke = scope.__TAURI_INTERNALS__.invoke.bind(scope.__TAURI_INTERNALS__);
    let failed = false;
    scope.__TAURI_INTERNALS__.invoke = async (command, args, options) => {
      if (command === "set_local_pool_membership" && !failed) {
        failed = true;
        await new Promise<void>((resolve) => { scope.__FAIL_POOL_REMOVE__ = resolve; });
        throw { code: "conflict", message: "Synthetic membership failure" };
      }
      return invoke(command, args, options);
    };
  });
  const toolbar = page.locator(".pool-member-toolbar");
  await toolbar.getByRole("checkbox").check();
  await toolbar.getByRole("button", { name: "Remove from pool", exact: true }).click();
  await page.getByRole("dialog").getByRole("button", { name: "Remove from pool", exact: true }).click();
  await expect(toolbar.getByRole("checkbox")).toBeDisabled();
  await expect(toolbar.getByRole("button", { name: "Clear selection", exact: true })).toBeDisabled();
  for (const button of await page.locator(".pool-member-card .account-select-button").all()) await expect(button).toBeDisabled();
  await page.evaluate(() => (window as unknown as { __FAIL_POOL_REMOVE__: () => void }).__FAIL_POOL_REMOVE__());
  await expectFailure(page, "Synthetic membership failure");
  await expect(page.locator('.pool-member-card[data-selected="true"]')).toHaveCount(2);
  await toolbar.getByRole("button", { name: "Remove from pool", exact: true }).click();
  await page.getByRole("dialog").getByRole("button", { name: "Remove from pool", exact: true }).click();
  await expect(page.locator(".pool-member-card")).toHaveCount(0);
  await expect(toolbar).toHaveCount(0);
  expect(await calls(page, "set_local_pool_membership")).toHaveLength(1);
});

test("selection cannot refresh accounts needing sign-in or sources without credentials", async ({ page }) => {
  await installTauriMock(page, { populated: true, quotaRefreshStatus: "requires_reauth", sourceSecretAvailable: false });
  await openPool(page);
  const toolbar = page.locator(".pool-member-toolbar");
  await toolbar.getByRole("checkbox").check();
  await expect(toolbar.getByRole("button", { name: "Refresh selected", exact: true })).toBeDisabled();
  expect(await calls(page, "refresh_local_account_quota")).toHaveLength(0);
  expect(await calls(page, "refresh_local_source_data")).toHaveLength(0);
  await expect(toolbar.getByRole("button", { name: "Remove from pool", exact: true })).toBeEnabled();
});

for (const locale of ["ru", "en"] as const) {
  for (const theme of ["dark", "light"] as const) {
    for (const width of [1160, 840, 390, 360]) {
      test(`pool selection stays compact ${locale} ${theme} ${width}`, async ({ page }, testInfo) => {
        const ru = locale === "ru";
        await installTauriMock(page, { populated: true, accountRouteKinds: Array.from({ length: 16 }, () => "excel_bps" as const), sourceCount: 3, locale, theme });
        await page.setViewportSize({ width, height: 844 });
        await openPool(page, ru);
        const controls = page.locator(".pool-controls");
        const toolbar = page.locator(".pool-member-toolbar");
        const before = (await controls.boundingBox())!.height;
        const selection = page.locator(".pool-member-card .account-select-button").first();
        await selection.focus();
        await page.keyboard.press("Space");
        await expect(selection).toHaveAttribute("aria-pressed", "true");
        await expect(toolbar.getByRole("checkbox")).toHaveJSProperty("indeterminate", true);
        await toolbar.getByRole("checkbox").check();
        await expect(page.locator('.pool-member-card[data-selected="true"]')).toHaveCount(19);
        await expect(toolbar).toContainText(ru ? "Выбрано: 19" : "Selected: 19");
        expect((await controls.boundingBox())!.height).toBeLessThanOrEqual(before + 1);
        expect(await toolbar.evaluate((element) => {
          const bounds = element.getBoundingClientRect();
          const boxes = [...element.querySelectorAll<HTMLElement>("button, input, .pool-selected-count")].map((item) => item.getBoundingClientRect());
          return bounds.left >= 0 && bounds.right <= innerWidth && element.scrollWidth <= element.clientWidth + 1
            && boxes.every((box) => box.left >= bounds.left && box.right <= bounds.right && box.top >= bounds.top && box.bottom <= bounds.bottom)
            && boxes.every((box, i) => boxes.slice(i + 1).every((other) => box.right <= other.left || other.right <= box.left));
        })).toBe(true);
        expect(await page.locator(".pool-member-card").evaluateAll((cards) => cards.every((card) => card.scrollWidth <= card.clientWidth + 1))).toBe(true);
        await page.mouse.move(1, 1);
        await page.screenshot({ path: testInfo.outputPath("pool-selection.png"), animations: "disabled" });
        await toolbar.getByRole("checkbox").uncheck();
        await expect(page.locator('.pool-member-card[data-selected="true"]')).toHaveCount(0);
        await expect(toolbar.getByRole("button", { name: ru ? "Настройки ротации пула" : "Pool rotation settings", exact: true })).toBeVisible();
      });
    }
  }
}
