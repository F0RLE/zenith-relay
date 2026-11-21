import { expect, test, type Page } from "../bun-playwright";
import { emitTauriEvent, installTauriMock } from "./tauri-mock";
import type { AccountSummary } from "../../src/features/relay/api/types";

async function openExport(page: Page) {
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await page.locator(".account-card .account-row-menu summary").click();
  await page.getByRole("menuitem", { name: "Export", exact: true }).click();
  return page.getByRole("dialog", { name: "Export accounts" });
}

async function exports(page: Page) {
  return page.evaluate(() => (window as unknown as {
    __TAURI_TEST_INVOKES__: Array<{ command: string; args: { input: unknown } }>;
  }).__TAURI_TEST_INVOKES__.filter((call) => call.command === "export_local_accounts").map((call) => call.args.input));
}

test("export includes only the chosen sign-in methods and adapts formats", async ({ page }) => {
  await installTauriMock(page, { populated: true, usageActive: false,
    accountRouteKinds: ["codex", "excel_bps", "siwc"], accountRoutesGrouped: true });
  const dialog = await openExport(page);
  const methods = dialog.getByRole("group", { name: "Sign-in methods" });
  await expect(methods.getByRole("checkbox", { name: "Codex", exact: true })).toBeChecked();
  await expect(methods.getByRole("checkbox", { name: "Excel / BPS" })).not.toBeChecked();
  await expect(methods.getByRole("checkbox", { name: "SIWC" })).not.toBeChecked();
  await expect(dialog.getByRole("radio")).toHaveCount(8);
  await methods.getByRole("checkbox", { name: "Codex", exact: true }).uncheck();
  await expect(dialog.getByRole("button", { name: "Download JSON" })).toBeDisabled();
  await expect(dialog.getByRole("button", { name: "Copy JSON" })).toBeDisabled();
  await methods.getByRole("checkbox", { name: "Excel / BPS" }).check();
  await expect(dialog.getByRole("radio")).toHaveCount(2);
  await methods.getByRole("checkbox", { name: "SIWC" }).check();
  await expect(dialog.getByRole("radio")).toHaveCount(1);
  await expect(dialog.getByRole("alert")).toContainText("Sign in to SIWC again");
  await dialog.screenshot({ path: "output/playwright/export-sign-in-methods.png" });
  await dialog.getByRole("button", { name: "Download JSON" }).click();
  expect(await exports(page)).toEqual([{ accountIds: ["account_synthetic_2", "account_synthetic_3"], format: "zenith", destination: "download" }]);
});

test("refresh cannot silently include a newly linked method", async ({ page }) => {
  await installTauriMock(page, { populated: true, usageActive: false,
    accountRouteKinds: ["codex", "excel_bps"], accountRoutesGrouped: true });
  const dialog = await openExport(page);
  await page.evaluate(() => {
    const host = window as unknown as { __TAURI_INTERNALS__: { invoke: (command: string, args?: unknown, options?: unknown) => Promise<unknown> } };
    const invoke = host.__TAURI_INTERNALS__.invoke.bind(host.__TAURI_INTERNALS__);
    host.__TAURI_INTERNALS__.invoke = async (command, args, options) => {
      const result = await invoke(command, args, options);
      if (command === "get_local_runtime_state") {
        const state = result as { accounts: AccountSummary[] };
        state.accounts.push({ ...structuredClone(state.accounts[1]!), id: "account_new_siwc", oauthClientKind: "siwc" });
      }
      return result;
    };
  });
  await emitTauriEvent(page, "zenith-state-changed");
  await expect(dialog.getByRole("checkbox", { name: "SIWC" })).not.toBeChecked();
  await dialog.getByRole("button", { name: "Download JSON" }).click();
  expect(await exports(page)).toEqual([{ accountIds: ["account_synthetic_1"], format: "zenith", destination: "download" }]);
});

test("an unavailable selected method does not prevent exporting another saved method", async ({ page }) => {
  await installTauriMock(page, { populated: true, usageActive: false,
    accountRouteKinds: ["codex", "excel_bps"], accountRoutesGrouped: true });
  await page.addInitScript(() => {
    const host = window as unknown as { __TAURI_INTERNALS__: { invoke: (command: string, args?: unknown, options?: unknown) => Promise<unknown> } };
    const invoke = host.__TAURI_INTERNALS__.invoke.bind(host.__TAURI_INTERNALS__);
    host.__TAURI_INTERNALS__.invoke = async (command, args, options) => {
      const result = await invoke(command, args, options);
      if (command === "get_local_runtime_state") (result as { accounts: AccountSummary[] }).accounts[0]!.secretAvailable = false;
      return result;
    };
  });
  const dialog = await openExport(page);
  await expect(dialog.getByRole("checkbox", { name: "Codex", exact: true })).toBeDisabled();
  await expect(dialog.getByRole("button", { name: "Download JSON" })).toBeDisabled();
  await dialog.getByRole("checkbox", { name: "Excel / BPS" }).check();
  await dialog.getByRole("button", { name: "Download JSON" }).click();
  expect(await exports(page)).toEqual([{ accountIds: ["account_synthetic_2"], format: "zenith", destination: "download" }]);
});

test("changing sign-in methods clears permission to export proxy credentials", async ({ page }) => {
  await installTauriMock(page, { populated: true, usageActive: false,
    accountRouteKinds: ["codex", "excel_bps", "siwc"], accountRoutesGrouped: true });
  const dialog = await openExport(page);
  const proxy = dialog.getByRole("checkbox", { name: /Include account proxies/ });
  await proxy.check();
  await dialog.getByRole("checkbox", { name: "Excel / BPS", exact: true }).check();
  await expect(proxy).not.toBeChecked();
  await dialog.getByRole("button", { name: "Download JSON" }).click();
  expect(await exports(page)).toEqual([{ accountIds: ["account_synthetic_1", "account_synthetic_2"], format: "zenith", destination: "download" }]);
});
