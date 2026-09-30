import { expect, test } from "../../bun-playwright";
import { installTauriMock } from "../tauri-mock";
import { chooseOption, openGatewayApi, openGatewayApplication } from "./helpers";

test("application chrome is not text-selectable", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true });
  await page.goto("/");
  await page.keyboard.press("Control+A");

  expect(await page.evaluate(() => window.getSelection()?.toString())).toBe("");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await expect(page.locator("input").first()).toHaveCSS("user-select", "text");
});

test("Gateway exposes an explicit Codex WebSocket switch", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true });
  await page.goto("/");
  await openGatewayApplication(page);

  const settings = page.locator(".gateway-settings-panel");
  await expect(settings).toBeVisible();
  await expect(settings.getByRole("heading", { name: "ChatGPT background tasks" })).toBeVisible();
  await expect(settings.getByRole("heading", { name: "WebSocket for ChatGPT" })).toBeVisible();

  const websocket = settings.getByRole("checkbox", { name: "Use WebSocket in ChatGPT" });
  await expect(websocket).toBeChecked();
  await websocket.uncheck();
  await expect(websocket).not.toBeChecked();
  await expect(settings.getByText("Disabled · HTTP is used", { exact: true })).toBeVisible();

  await websocket.click();
  const restartDialog = page.getByRole("dialog", { name: "Restart ChatGPT" });
  await expect(restartDialog).toBeVisible();
  await restartDialog.getByRole("button", { name: "Restart and enable" }).click();
  await expect(websocket).toBeChecked();
  await expect(settings.getByText("Enabled", { exact: true })).toBeVisible();

  const websocketCalls = await page.evaluate(() => (window as unknown as {
    __TAURI_TEST_INVOKES__: Array<{ command: string; args: { input?: { enabled?: boolean } } }>;
  }).__TAURI_TEST_INVOKES__
    .filter((call) => call.command === "set_local_codex_websockets")
    .map((call) => call.args.input?.enabled));
  expect(websocketCalls).toEqual([false, true]);
});

for (const mode of ["local", "remote"] as const) {
  test(`${mode} API route recovery switch saves and retains its state`, async ({ page }) => {
    await installTauriMock(page, { mode, locale: "en", populated: true, remoteFeatures: ["route_recovery_v1"] });
    await page.goto("/");
    await openGatewayApi(page);
    const recovery = page.getByRole("checkbox", { name: "Wait for route recovery", exact: true });
    await expect(recovery).not.toBeChecked();
    await recovery.click();
    await expect(recovery).toBeChecked();
    await page.getByRole("tab", { name: "ChatGPT", exact: true }).click();
    await expect(recovery).toHaveCount(0);
    await page.getByRole("tab", { name: "API", exact: true }).click();
    await expect(recovery).toBeChecked();
    await recovery.click();
    await expect(recovery).not.toBeChecked();
    const values = await page.evaluate(() => (window as unknown as {
      __TAURI_TEST_INVOKES__: Array<{ command: string; args: { input?: { enabled?: boolean; action?: { type: string }; payload?: { enabled: boolean } } } }>;
    }).__TAURI_TEST_INVOKES__.filter((call) => call.command === "set_local_chatgpt_retry_until_available"
      || call.command === "execute_remote_server_action" && call.args.input?.action?.type === "set_chatgpt_retry_until_available")
      .map((call) => call.args.input?.enabled ?? call.args.input?.payload?.enabled));
    expect(values).toEqual([true, false]);
  });
}

test("old remote server does not advertise cross-protocol route recovery", async ({ page }) => {
  await installTauriMock(page, { mode: "remote", locale: "en", populated: true, remoteFeatures: ["chatgpt_retry_until_available"] });
  await page.goto("/");
  await openGatewayApi(page);
  await expect(page.getByRole("checkbox", { name: "Wait for route recovery" })).toHaveCount(0);
});

test("local commands are reachable from the operational UI", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, codexBindings: false, importDescription: "# Seller package\n\n- Two Business accounts" });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await expect(page.getByRole("tab", { name: "Accounts" })).toBeVisible();
  await expect(page.getByRole("tab").allTextContents()).resolves.toEqual(["Accounts", "Sources", "Proxies", "Automations"]);
  await page.getByRole("tab", { name: "Sources" }).click();
  const sourceRow = page.getByRole("row").filter({ hasText: "Example compatible API" });
  await sourceRow.getByRole("button", { name: "Launch", exact: true }).click();
  await page.getByRole("dialog", { name: "Where do you want to launch this source?" }).getByRole("button", { name: "ChatGPT", exact: true }).click();
  await expect.poll(() => page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string }> }).__TAURI_TEST_INVOKES__.some((call) => call.command === "launch_codex_source"))).toBe(true);
  await page.getByRole("button", { name: "Edit" }).click();
  const sourceDialog = page.getByRole("dialog", { name: "Edit source" });
  await expect(sourceDialog.locator('[role="tablist"]').first().getByRole("tab")).toHaveText(["General", "Pricing"]);
  await expect(sourceDialog.locator(".source-routing-disclosure")).toHaveCount(0);
  await expect(sourceDialog.locator(".source-price-section")).toHaveCount(0);
  await expect(sourceDialog.locator(".source-protocol-availability")).toHaveCount(0);
  await expect(sourceDialog.getByRole("radiogroup", { name: "API source role" })).toHaveCount(0);
  await expect(sourceDialog.locator("[data-member-model-id]")).toHaveCount(0);
  await sourceDialog.getByRole("tab", { name: "Pricing" }).click();
  await sourceDialog.locator(".source-price-group > summary").filter({ hasText: "OpenAI" }).click();
  await sourceDialog.getByRole("textbox", { name: "Input token price for gpt-5.4", exact: true }).fill("1.25");
  await sourceDialog.getByRole("textbox", { name: "Output token price for gpt-5.4", exact: true }).fill("3.5");
  await sourceDialog.getByRole("button", { name: "Save" }).click();
  await expect.poll(() => page.evaluate(() => {
    const call = (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: { input?: { modelPriceOverrides?: Record<string, unknown> } } }> }).__TAURI_TEST_INVOKES__.findLast((item) => item.command === "update_local_source");
    return call?.args.input?.modelPriceOverrides?.["gpt-5.4"];
  })).toEqual({ inputMicroUsdPerMillion: 1_250_000, outputMicroUsdPerMillion: 3_500_000 });
  await page.getByRole("tab", { name: "Accounts" }).click();
  await page.getByRole("button", { name: "Sign in" }).first().click();
  const oauthDialog = page.getByRole("dialog", { name: "Sign in" });
  await expect(oauthDialog.getByText("Waiting for sign-in", { exact: true })).toBeVisible();
  await expect(oauthDialog.getByText("Time remaining", { exact: true })).toBeVisible();
  await expect(oauthDialog.getByRole("button", { name: "Copy sign-in link" })).toBeVisible();
  const open = oauthDialog.getByRole("button", { name: "Open sign-in window" });
  await expect(open).toBeEnabled();
  await open.click();
  const reopen = oauthDialog.getByRole("button", { name: /Open again in|Reopen sign-in window/ });
  await expect(reopen).toBeDisabled();
  await expect(reopen).toBeEnabled({ timeout: 4_000 });
  await reopen.click();
  await expect(reopen).toBeDisabled();
  await expect.poll(() => page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string }> }).__TAURI_TEST_INVOKES__.some((call) => call.command === "resume_codex_oauth"))).toBe(true);
  await expect(oauthDialog.getByText("Sign-in did not finish automatically", { exact: true })).toHaveCount(0);
  await page.getByRole("button", { name: "Cancel" }).click();

  await page.getByRole("button", { name: "Import", exact: true }).click();
  const importDialog = page.getByRole("dialog", { name: "Import accounts" });
  await importDialog.getByRole("button", { name: "Choose account files" }).click();
  await expect(importDialog.getByText("Package description")).toBeVisible();
  await expect(importDialog.getByRole("heading", { name: "Seller package" })).toBeVisible();
  const imported = importDialog.getByLabel("Select Imported account for import");
  const secondImported = importDialog.getByLabel("Select Second imported account for import");
  const existing = importDialog.getByLabel("Select Existing account for import");
  await expect(imported).toBeChecked();
  await expect(secondImported).toBeChecked();
  await expect(existing).not.toBeChecked();
  await importDialog.getByLabel("Add selected to pool after import").check();
  await expect(importDialog.getByLabel("Assign a stored proxy")).not.toBeChecked();
  await importDialog.getByRole("button", { name: "Import 2 account(s)" }).click();
  await expect(importDialog).toBeHidden();
  const importCalls = await page.evaluate(() => {
    const calls = (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: { input?: { selectedItemIds?: string[]; addToPool?: boolean; discoverModels?: boolean; probeQuota?: boolean } } }> }).__TAURI_TEST_INVOKES__;
    const confirmation = calls.findLast((call) => call.command === "confirm_local_account_import")?.args.input;
    return {
      filePreviewCalls: calls.filter((call) => call.command === "preview_local_account_import_files").length,
      selected: confirmation?.selectedItemIds,
      addToPool: confirmation?.addToPool,
      discoverModels: confirmation?.discoverModels,
      probeQuota: confirmation?.probeQuota,
      assignedFree: calls.filter((call) => call.command === "assign_free_local_account_proxies").length,
    };
  });
  expect(importCalls.filePreviewCalls).toBe(1);
  expect(importCalls.selected).toEqual([
    "import_0123456789abcdef",
    "import_1111222233334444",
  ]);
  expect(importCalls.addToPool).toBe(true);
  expect(importCalls.discoverModels).toBe(false);
  expect(importCalls.probeQuota).toBe(false);
  expect(importCalls.assignedFree).toBe(0);

  await page.getByRole("tab", { name: "Automations" }).click();
  await page.getByRole("button", { name: "Edit" }).click();
  const automation = page.getByRole("dialog", { name: "Edit automation" });
  await chooseOption(page, automation, "Accounts", "account_ids");
  await automation.getByLabel("Personal Plus").check();
  await chooseOption(page, automation, "Model", "gpt-5.4-mini");
  await automation.getByRole("button", { name: "Save" }).click();
  const automationRow = page.getByRole("listitem").filter({ hasText: "Start quota countdown" });
  await expect(automationRow).toContainText("Personal Plus");
  await expect(page.getByRole("columnheader", { name: "Quota" })).toHaveCount(0);
  await expect(automationRow).toContainText("After primary quota recovery");
  await expect(automationRow).not.toContainText("Secondary");
  await expect(automationRow).toContainText("gpt-5.4-mini");
  await expect(automationRow.getByRole("button", { name: "Test" })).toHaveCount(0);

  await page.getByRole("button", { name: "Pool", exact: true }).click();
  await page.getByRole("button", { name: "Pool member policy: Example compatible API" }).click();
  const sourcePolicy = page.getByRole("dialog", { name: "Pool member policy", exact: true });
  await sourcePolicy.getByRole("switch", { name: "Allow gpt-5.4", exact: true }).uncheck();
  await sourcePolicy.getByRole("tab", { name: "Settings", exact: true }).click();
  await chooseOption(page, sourcePolicy, "Recovery check", "60");
  await expect(sourcePolicy.getByLabel("Drain")).toHaveCount(0);
  await sourcePolicy.getByRole("button", { name: "Save policy" }).click();
  await page.getByRole("button", { name: "Pool member policy: Personal Plus" }).click();
  await page.getByRole("switch", { name: "Allow gpt-5.4", exact: true }).uncheck();
  await page.getByRole("dialog").getByRole("tab", { name: "Settings", exact: true }).click();
  await page.getByLabel("Drain").check();
  await page.getByLabel("Purchase cost, USD").fill("25.50");
  await page.getByRole("button", { name: "Save policy" }).click();
  await expect(page.getByText("Saved.")).toBeVisible();

  await openGatewayApi(page);
  await expect(page.locator(".gateway-api-connection-panel")).toBeVisible();
  await page.locator(".relay-page-actions .relay-action-menu summary").click();
  await page.getByRole("menuitem", { name: "Restart API" }).click();
  await page.getByRole("spinbutton", { name: "Port" }).fill("15001");
  await page.getByRole("spinbutton", { name: "Port" }).press("Enter");
  await expect(page.getByRole("spinbutton", { name: "Port" })).toHaveValue("15001");
  await expect(page.getByText("http://127.0.0.1:15001/v1")).toBeVisible();
  await page.getByRole("tab", { name: "ChatGPT", exact: true }).click();
  await expect(page.locator(".gateway-settings-panel")).toBeVisible();
  await expect(page.getByRole("button", { name: /^Account:/ })).toHaveAttribute("data-value", "auto");
  await expect(page.getByRole("heading", { name: "ChatGPT account" })).toBeVisible();
  const gatewayCalls = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: { port?: number } }> }).__TAURI_TEST_INVOKES__.filter((call) => call.command === "restart_local_gateway" || call.command === "update_local_gateway_port"));
  expect(gatewayCalls).toEqual([{ command: "restart_local_gateway", args: {} }, { command: "update_local_gateway_port", args: { port: 15001 } }]);
  const policyCalls = await page.evaluate(() => {
    const calls = (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: Record<string, unknown> }> }).__TAURI_TEST_INVOKES__;
    return Object.fromEntries(calls
      .filter((call) => ["update_local_source", "test_quota_wake_automation", "update_local_account"].includes(call.command))
      .map((call) => [call.command, call.args]));
  });
  expect(policyCalls.update_local_source).toMatchObject({ input: { wireApi: "responses", protocolBindings: [{ wireApi: "responses", modelIds: ["gpt-5.4", "gpt-5.4-mini"] }], models: ["gpt-5.4", "gpt-5.4-mini"], allowedModels: [], excludedModels: ["gpt-5.4"], priority: 10, weight: 100, recoveryDelaySeconds: 60 } });
  expect(policyCalls.test_quota_wake_automation).toBeUndefined();
  expect(policyCalls.update_local_account).toMatchObject({ input: { draining: true, allowedModels: [], excludedModels: ["gpt-5.4"], purchaseCostMicroUsd: 25_500_000 } });
});

test("reset credits are visible and require explicit account confirmation", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();

  const reset = page.getByRole("button", { name: "Reset available: 1 · Reset weekly quota", exact: true });
  await expect(reset).toBeVisible();
  await reset.click();
  const dialog = page.getByRole("dialog", { name: "Reset weekly quota" });
  await expect(dialog).toContainText("Reset the weekly quota for this account?");
  await dialog.getByRole("button", { name: "No", exact: true }).click();
  await expect(dialog).toBeHidden();
  await expect(reset).toBeVisible();

  await reset.click();
  await page.getByRole("dialog", { name: "Reset weekly quota" }).getByRole("button", { name: "Yes, reset", exact: true }).click();
  await expect(dialog).toBeHidden();
  await expect(page.getByRole("button", { name: /Reset available: 0/ })).toHaveCount(0);

  const calls = await page.evaluate(() => (window as unknown as {
    __TAURI_TEST_INVOKES__: Array<{ command: string; args: { accountId?: string } }>;
  }).__TAURI_TEST_INVOKES__);
  expect(calls.filter((call) => call.command === "get_local_reset_credits")).toHaveLength(0);
  expect(calls.filter((call) => call.command === "consume_local_reset_credit")).toHaveLength(1);
  expect(calls.filter((call) => call.command === "consume_local_reset_credit").at(-1)?.args.accountId).toBe("account_synthetic");
});

test("pool summary shows the total provider credits across pooled accounts", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, accountCount: 2, providerCredits: 2.5 });
  await page.goto("/");
  await page.getByRole("button", { name: "Pool", exact: true }).click();

  const summary = page.locator(".pool-summary");
  const credits = summary.locator('[data-summary="provider-credits"]');
  await expect(summary).toHaveAttribute("data-has-provider-credits", "true");
  await expect(credits).toContainText("Total credits");
  await expect(credits.locator("strong")).toHaveText("5");
});

test("reset action is absent when no reset credit is available", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, resetCreditsAvailable: 0 });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await expect(page.locator(".reset-credits-control")).toHaveCount(0);
});

test("source launch picker starts the selected source in OpenCode", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await page.getByRole("tab", { name: "Sources", exact: true }).click();

  const sourceRow = page.getByRole("row").filter({ hasText: "Example compatible API" });
  await sourceRow.getByRole("button", { name: "Launch", exact: true }).click();
  const picker = page.getByRole("dialog", { name: "Where do you want to launch this source?" });
  await picker.getByRole("button", { name: "OpenCode", exact: true }).click();
  await expect(page.getByText("Client launched.", { exact: true })).toBeVisible();

  const calls = await page.evaluate(() => (window as unknown as {
    __TAURI_TEST_INVOKES__: Array<{ command: string; args: Record<string, unknown> }>;
  }).__TAURI_TEST_INVOKES__);
  expect(calls.find((call) => call.command === "launch_opencode_source")?.args).toEqual({ sourceId: "source_synthetic" });
  expect(calls.map((call) => call.command)).not.toContain("launch_codex_source");
  await expect.poll(() => page.evaluate(() => localStorage.getItem("relay.directSourceId"))).toBe("source_synthetic");
});

for (const nav of ["Connections", "Pool"] as const) {
  test(`${nav} keeps reset refresh failures visible after the last credit is consumed`, async ({ page }) => {
    await installTauriMock(page, {
      mode: "local", locale: "en", populated: true,
      resetCreditsRefreshError: "Quota unavailable: token=synthetic-reset-secret",
    });
    await page.goto("/");
    await page.getByRole("button", { name: nav, exact: true }).click();
    await page.locator(".reset-credits-control").click();
    await page.getByRole("dialog", { name: "Reset weekly quota" }).getByRole("button", { name: "Yes, reset", exact: true }).click();
    const error = page.locator(".reset-credits-inline-error");
    await expect(error).toContainText("Reset was applied, but quota refresh failed");
    await expect(error).toContainText("Quota unavailable");
    await expect(error).not.toContainText("synthetic-reset-secret");
    await expect(page.locator(".reset-credits-control")).toHaveCount(0);
  });
}

test("reset rejection uses redacted diagnostics and leaves the credit available", async ({ page }) => {
  await installTauriMock(page, {
    mode: "local", locale: "en", populated: true,
    resetCreditsError: 'Reset rejected: {"token":"synthetic-reset-secret"}',
  });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await page.locator(".reset-credits-control").click();
  await page.getByRole("dialog", { name: "Reset weekly quota" }).getByRole("button", { name: "Yes, reset", exact: true }).click();
  const error = page.locator(".reset-credits-inline-error");
  await expect(error).toContainText("Reset failed: Reset rejected");
  await expect(error).not.toContainText("synthetic-reset-secret");
  await expect(page.getByRole("button", { name: "Reset available: 1 · Reset weekly quota", exact: true })).toBeEnabled();
});

test("secret fields expose only the themed reveal control", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await page.getByRole("tab", { name: "Sources", exact: true }).click();
  await page.getByRole("button", { name: "Add source", exact: true }).click();
  await page.getByRole("radio", { name: /^OpenAI/ }).click();
  await expect(page.locator(".secret-field")).toHaveCount(1);
  await expect(page.locator(".secret-field > button")).toHaveCount(1);
});

for (const mode of ["local", "remote"] as const) {
  test(`${mode} mixed rotation reorders accounts and API providers and retains weights`, async ({ page }) => {
    await installTauriMock(page, { mode, locale: "en", populated: true, sourceCount: 2 });
    await page.goto("/");
    await page.getByRole("button", { name: "Pool", exact: true }).click();
    await page.getByRole("button", { name: "Pool rotation settings", exact: true }).click();
    const dialog = page.getByRole("dialog", { name: "Pool rotation", exact: true });
    await dialog.getByRole("radio", { name: "In order", exact: true }).click();
    const order = dialog.getByRole("list", { name: "Member order" });
    await expect(order.getByRole("listitem")).toHaveCount(3);
    const api = order.locator('[data-member-id="source:source_synthetic"]');
    const account = order.locator('[data-member-id="account:account_synthetic"]');
    await api.getByRole("button", { name: "Reorder Example compatible API" }).dragTo(account);
    await expect(order.getByRole("listitem").first()).toContainText("Example compatible API");
    await dialog.getByRole("radio", { name: "Round robin", exact: true }).click();
    await dialog.getByLabel("Request share: Example compatible API", { exact: true }).fill("3");
    await dialog.getByLabel("Concurrent requests: Example compatible API", { exact: true }).fill("2");
    await dialog.getByRole("button", { name: "Close", exact: true }).last().click();
    await expect(dialog).toBeHidden();
    await page.getByRole("button", { name: "Pool rotation settings", exact: true }).click();
    await expect(dialog.getByRole("radio", { name: "Round robin", exact: true })).toHaveAttribute("aria-checked", "true");
    await expect(dialog.getByLabel("Request share: Example compatible API", { exact: true })).toHaveValue("3");
    await expect(dialog.getByLabel("Concurrent requests: Example compatible API", { exact: true })).toHaveValue("2");
    await dialog.getByRole("radio", { name: "In order", exact: true }).click();
    await expect(order.getByRole("listitem").first()).toContainText("Example compatible API");
    await expect(dialog.getByLabel("Request share: Example compatible API", { exact: true })).toHaveCount(0);
    await dialog.getByRole("button", { name: "Close", exact: true }).last().click();
  });
}

test("dialogs keep editable focus and close a nested option list before the dialog", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await page.getByRole("tab", { name: "Sources" }).click();

  const sourceRow = page.getByRole("row").filter({ hasText: "Example compatible API" });
  const sourceEdit = sourceRow.getByRole("button", { name: "Edit" });
  await sourceEdit.click();
  const sourceDialog = page.getByRole("dialog", { name: "Edit source" });
  const name = sourceDialog.getByRole("textbox", { name: "Name" });
  await name.focus();
  await page.keyboard.type("x");
  await page.keyboard.press("Backspace");
  await expect(name).toHaveValue("Example compatible API");
  await expect(name).toBeFocused();

  const save = sourceDialog.getByRole("button", { name: "Save" });
  await save.focus();
  await page.keyboard.press("Tab");
  await expect(sourceDialog.getByRole("button", { name: "Close" })).toBeFocused();
  await page.keyboard.press("Shift+Tab");
  await expect(save).toBeFocused();
  await page.keyboard.press("Escape");
  await expect(sourceDialog).toBeHidden();
  await expect(sourceEdit).toBeFocused();

  await page.getByRole("tab", { name: "Automations" }).click();
  const automationEdit = page.getByRole("button", { name: "Edit", exact: true });
  await automationEdit.click();
  const automationDialog = page.getByRole("dialog", { name: "Edit automation" });
  const accounts = automationDialog.getByRole("button", { name: /^Accounts:/ });
  await accounts.click();
  const list = page.getByRole("listbox", { name: "Accounts" });
  await expect(list).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(list).toBeHidden();
  await expect(automationDialog).toBeVisible();
  await expect(accounts).toBeFocused();
  await page.keyboard.press("Escape");
  await expect(automationDialog).toBeHidden();
  await expect(automationEdit).toBeFocused();
});
