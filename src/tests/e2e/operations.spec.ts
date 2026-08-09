import { expect, test, type Locator, type Page } from "../bun-playwright";
import { emitTauriEvent, installTauriMock } from "./tauri-mock";

async function chooseOption(page: Page, scope: Page | Locator, label: string, value: string) {
  await scope.getByRole("button", { name: new RegExp(`^${label.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}:`) }).click();
  await page.locator(`[role="option"][data-value="${value}"]`).click();
}

async function settleConfirmation(page: Page, accept = true) {
  const dialog = page.getByRole("dialog", { name: "Confirm action" });
  await expect(dialog).toBeVisible();
  await dialog.getByRole("button", { name: accept ? "Confirm" : "Cancel" }).click();
}

test("application chrome is not text-selectable", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true });
  await page.goto("/");
  await page.keyboard.press("Control+A");

  expect(await page.evaluate(() => window.getSelection()?.toString())).toBe("");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await expect(page.locator("input").first()).toHaveCSS("user-select", "text");
});

test("local commands are reachable from the operational UI", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, codexBindings: false, importDescription: "# Seller package\n\n- Two Business accounts" });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await expect(page.getByRole("tab", { name: "Accounts" })).toBeVisible();
  await expect(page.getByRole("tab").allTextContents()).resolves.toEqual(["Accounts", "Sources", "Proxies", "Automations"]);
  await page.getByRole("tab", { name: "Sources" }).click();
  const sourceRow = page.getByRole("row").filter({ hasText: "Example compatible API" });
  await sourceRow.getByRole("button", { name: "Launch in ChatGPT" }).click();
  await expect.poll(() => page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string }> }).__TAURI_TEST_INVOKES__.some((call) => call.command === "launch_codex_source"))).toBe(true);
  await page.getByRole("button", { name: "Edit" }).click();
  const sourceDialog = page.getByRole("dialog", { name: "Edit source" });
  await sourceDialog.locator(".source-routing-details > summary").click();
  await sourceDialog.getByRole("checkbox", { name: "Chat Completions is available from this source", exact: true }).check();
  await sourceDialog.getByRole("checkbox", { name: "Responses is available from this source", exact: true }).uncheck();
  await expect(sourceDialog.getByRole("radiogroup", { name: "API source role" })).toHaveCount(0);
  await expect(sourceDialog.locator("[data-member-model-id]")).toHaveCount(0);
  await sourceDialog.locator(".source-price-section > summary").click();
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
  const open = oauthDialog.getByRole("button", { name: "Open in browser" });
  await expect(open).toBeEnabled();
  await open.click();
  const reopen = oauthDialog.getByRole("button", { name: /Open again in|Reopen sign-in page/ });
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
  const automationRow = page.getByRole("row").filter({ hasText: "Start quota countdown" });
  await expect(automationRow).toContainText("Personal Plus");
  await expect(page.getByRole("columnheader", { name: "Quota" })).toHaveCount(0);
  await expect(automationRow).not.toContainText("Primary");
  await expect(automationRow).not.toContainText("Secondary");
  await expect(automationRow).toContainText("gpt-5.4-mini");
  await automationRow.getByRole("button", { name: "Test" }).click();

  await page.getByRole("button", { name: "Pool", exact: true }).click();
  await page.getByRole("button", { name: "Pool member policy: Example compatible API" }).click();
  const sourcePolicy = page.getByRole("dialog", { name: /Pool member policy.*Example compatible API/ });
  const sourceRoles = sourcePolicy.getByRole("radiogroup", { name: "API source role" });
  await expect(sourceRoles.getByRole("radio")).toHaveCount(3);
  await expect(sourcePolicy.getByLabel("Fallback order")).toContainText("API first");
  await expect(sourcePolicy.getByLabel("Fallback order")).toContainText("Accounts");
  await sourceRoles.getByRole("radio", { name: /API first/ }).click();
  await expect(sourcePolicy.locator('.source-route-stage[data-current="true"]')).toContainText("API first");
  await expect(sourcePolicy.getByRole("spinbutton", { name: "Traffic share" })).toHaveCount(0);
  await expect(sourcePolicy.getByRole("list", { name: "API order in this role" }).getByRole("listitem")).toHaveCount(1);
  await chooseOption(page, sourcePolicy, "Recovery check", "60");
  await expect(sourcePolicy.locator(".member-model-rules")).toHaveCount(0);
  await expect(sourcePolicy.locator(".source-model-configuration > summary")).toContainText("Models and cost");
  await sourcePolicy.locator(".source-model-configuration > summary").click();
  await sourcePolicy.locator(".source-price-group > summary").filter({ hasText: "OpenAI" }).click();
  await sourcePolicy.locator('[data-member-model-id="gpt-5.4"]').getByRole("button", { name: "Disable gpt-5.4" }).click();
  await expect(sourcePolicy.getByLabel("Drain")).toHaveCount(0);
  await sourcePolicy.getByRole("button", { name: "Save policy" }).click();
  await page.getByRole("button", { name: "Pool member policy: Personal Plus" }).click();
  await page.getByLabel("Drain").check();
  await page.getByLabel("Purchase cost, USD").fill("25.50");
  await page.locator(".member-model-rules > summary").click();
  await page.locator('[data-member-model-id="gpt-5.4"]').getByRole("button", { name: "Disable gpt-5.4" }).click();
  await page.getByRole("button", { name: "Save policy" }).click();
  await expect(page.getByText("Saved.")).toBeVisible();

  await page.getByRole("button", { name: "API & ChatGPT", exact: true }).click();
  await expect(page.locator(".gateway-settings-panel")).toBeVisible();
  await page.locator(".relay-page-actions .relay-action-menu summary").click();
  await page.getByRole("menuitem", { name: "Restart API" }).click();
  await page.getByRole("spinbutton", { name: "Port" }).fill("15001");
  await page.getByRole("spinbutton", { name: "Port" }).press("Enter");
  await expect(page.getByRole("spinbutton", { name: "Port" })).toHaveValue("15001");
  await expect(page.getByText("http://127.0.0.1:15001/v1")).toHaveCount(0);
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
  expect(policyCalls.update_local_source).toMatchObject({ input: { wireApi: "chat_completions", protocolBindings: [{ wireApi: "chat_completions", modelIds: [] }], models: ["gpt-5.4", "gpt-5.4-mini"], allowedModels: ["gpt-5.4-mini"], excludedModels: ["gpt-5.4"], priority: 1_000_001, sourcePriorities: { source_synthetic: 1_000_001 }, weight: 1, recoveryDelaySeconds: 60 } });
  expect(policyCalls.test_quota_wake_automation).toEqual({ taskId: "wake_synthetic" });
  expect(policyCalls.update_local_account).toMatchObject({ input: { draining: true, allowedModels: ["gpt-5.4-mini"], excludedModels: ["gpt-5.4"], purchaseCostMicroUsd: 25_500_000 } });
});

test("API sources use an explicit fallback order instead of traffic weights", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, sourceCount: 2 });
  await page.goto("/");
  await page.getByRole("button", { name: "Pool", exact: true }).click();
  await page.getByRole("button", { name: "Pool member policy: Example compatible API" }).click();

  const dialog = page.getByRole("dialog", { name: /Pool member policy.*Example compatible API/ });
  const order = dialog.getByRole("list", { name: "API order in this role" });
  await expect(order.getByRole("listitem")).toHaveCount(2);
  await expect(order.getByRole("listitem").nth(0)).toContainText("Example compatible API");
  await dialog.getByRole("button", { name: "Move Example compatible API down" }).click();
  await expect(order.getByRole("listitem").nth(0)).toContainText("Backup API 1");
  await dialog.getByRole("button", { name: "Save policy" }).click();

  const input = await page.evaluate(() => {
    const calls = (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: { input?: Record<string, unknown> } }> }).__TAURI_TEST_INVOKES__;
    return calls.findLast((call) => call.command === "update_local_source")?.args.input;
  });
  expect(input).toMatchObject({ priority: 1, sourcePriorities: { source_synthetic: 1, source_synthetic_2: 2 }, weight: 1 });
  const routingOrder = await page.evaluate(async () => {
    const internals = (window as unknown as { __TAURI_INTERNALS__: { invoke: (command: string) => Promise<Array<{ candidateId: string; kind: string }>> } }).__TAURI_INTERNALS__;
    return (await internals.invoke("get_local_runtime_order"))
      .filter((candidate) => candidate.kind === "api_source")
      .map((candidate) => candidate.candidateId);
  });
  expect(routingOrder).toEqual(["source_synthetic_2", "source_synthetic"]);
});

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

test("automation editor only saves executable local configurations", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, accountCount: 3, codexBindings: false });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await page.getByRole("tab", { name: "Automations" }).click();
  await page.getByRole("button", { name: "Edit" }).click();

  const dialog = page.getByRole("dialog", { name: "Edit automation" });
  const save = dialog.getByRole("button", { name: "Save" });
  await dialog.getByRole("button", { name: /^Accounts:/ }).click();
  await expect(page.locator('[role="option"][data-value="tags"]')).toHaveCount(0);
  await page.locator('[role="option"][data-value="account_ids"]').click();
  await expect(save).toBeDisabled();

  await dialog.getByLabel("Personal Plus").check();
  await dialog.getByLabel("Backup account").check();
  await dialog.getByRole("button", { name: /^Model:/ }).click();
  await expect(page.locator('[role="option"][data-value="gpt-5.4-mini"]')).toHaveCount(1);
  await expect(page.locator('[role="option"][data-value="gpt-5.4"]')).toHaveCount(0);
  await expect(page.locator('[role="option"][data-value="o3"]')).toHaveCount(0);
  await page.locator('[role="option"][data-value="gpt-5.4-mini"]').click();

  await dialog.getByRole("button", { name: "Manual" }).click();
  await expect(dialog.getByRole("button", { name: "Manual" })).toHaveAttribute("aria-pressed", "true");
  await save.click();

  const call = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: { input?: Record<string, unknown> } }> }).__TAURI_TEST_INVOKES__.findLast((item) => item.command === "update_quota_wake_automation"));
  expect(call?.args.input).toMatchObject({
    accountSelector: { kind: "account_ids", values: ["account_synthetic", "account_synthetic_3"] },
    windowKinds: ["primary"],
    modelPolicy: { kind: "explicit", value: "gpt-5.4-mini" },
    executionPolicy: "require_confirmation",
  });
});

test("remote automation editor exposes only automatic execution", async ({ page }) => {
  await installTauriMock(page, { mode: "remote", locale: "en", populated: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await page.getByRole("tab", { name: "Automations" }).click();
  await page.getByRole("button", { name: "Edit" }).click();

  const dialog = page.getByRole("dialog", { name: "Edit automation" });
  await expect(dialog.getByRole("group", { name: "Run" })).toHaveCount(0);
  await expect(dialog.getByText("Automatic", { exact: true })).toBeVisible();
  await expect(dialog.getByRole("button", { name: /^Model: gpt-5\.4/ })).toBeVisible();
  await dialog.getByRole("button", { name: /^Accounts:/ }).click();
  await expect(page.locator('[role="option"][data-value="tags"]')).toHaveCount(0);
  await page.keyboard.press("Escape");
  await dialog.getByRole("button", { name: "Save" }).click();

  const call = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: { input?: { action?: Record<string, unknown>; payload?: Record<string, unknown> } } }> }).__TAURI_TEST_INVOKES__.findLast((item) => item.command === "execute_remote_server_action"));
  expect(call?.args.input).toMatchObject({ action: { type: "update_wake_task", id: "wake_synthetic" }, payload: { executionPolicy: "automatic", modelPolicy: { kind: "explicit", value: "gpt-5.4" } } });
});

for (const mode of ["local", "remote"] as const) {
  test(`API source usage keeps source diagnostics in ${mode} mode`, async ({ page }) => {
    await installTauriMock(page, { mode, locale: "en", populated: true, usageCandidateKind: "source" });
    await page.goto("/");
    await page.getByRole("button", { name: "Usage", exact: true }).click();
    await page.getByRole("button", { name: new RegExp(`Request details: req_synthetic_${mode}`) }).click();
    const dialog = page.getByRole("dialog", { name: "Request details" });
    await expect(dialog).toContainText("Weighted rotation");
    await expect(dialog).toContainText("Example compatible API");
    await expect(dialog.getByText("Quota at selection", { exact: true })).toHaveCount(0);
  });
}

test("API pricing groups expose cache-write TTLs only for Claude", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, mixedModels: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await page.getByRole("tab", { name: "Sources" }).click();
  await page.getByRole("row").filter({ hasText: "Example compatible API" }).getByRole("button", { name: "Edit" }).click();
  const dialog = page.getByRole("dialog", { name: "Edit source" });
  await dialog.locator(".source-price-section > summary").click();
  await expect(dialog.locator(".source-price-group > summary")).toHaveText(["OpenAIModels: 1", "ClaudeModels: 1", "Google GeminiModels: 1", "Zhipu GLMModels: 1", "xAI GrokModels: 1", "OtherModels: 1"]);

  await dialog.locator(".source-price-group > summary").filter({ hasText: "OpenAI" }).click();
  await expect(dialog.getByRole("textbox", { name: /cache write price for gpt-5.4/i })).toHaveCount(0);
  await dialog.locator(".source-price-group > summary").filter({ hasText: "Claude" }).click();
  await dialog.getByRole("textbox", { name: "Input token price for claude-opus-4-8", exact: true }).fill("1.4");
  await dialog.getByRole("textbox", { name: "Output token price for claude-opus-4-8", exact: true }).fill("7");
  await dialog.getByRole("textbox", { name: "Cached input token price for claude-opus-4-8", exact: true }).fill("1.6");
  await dialog.getByRole("textbox", { name: "5-minute cache write price for claude-opus-4-8" }).fill("2.1");
  await dialog.getByRole("textbox", { name: "1-hour cache write price for claude-opus-4-8" }).fill("4.2");
  await dialog.getByRole("button", { name: "Save" }).click();

  await expect.poll(() => page.evaluate(() => {
    const call = (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: { input?: { modelPriceOverrides?: Record<string, unknown> } } }> }).__TAURI_TEST_INVOKES__.findLast((item) => item.command === "update_local_source");
    return call?.args.input?.modelPriceOverrides?.["claude-opus-4-8"];
  })).toEqual({ inputMicroUsdPerMillion: 1_400_000, outputMicroUsdPerMillion: 7_000_000, cachedInputMicroUsdPerMillion: 1_600_000, cacheWrite5mMicroUsdPerMillion: 2_100_000, cacheWrite1hMicroUsdPerMillion: 4_200_000 });
});

test("pool API source edits model availability and cost in one list", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Pool", exact: true }).click();
  await page.getByRole("button", { name: "Pool member policy: Example compatible API" }).click();

  const dialog = page.getByRole("dialog", { name: /Pool member policy.*Example compatible API/ });
  const configuration = dialog.locator(".source-model-configuration");
  await expect(configuration).toHaveCount(1);
  await expect(configuration.locator("> summary")).toContainText("Models and cost");
  await expect(configuration.locator("> summary")).toContainText("Enabled: 2/2");
  await expect(dialog.locator(".member-model-rules")).toHaveCount(0);

  await configuration.locator("> summary").click();
  await configuration.locator(".source-price-group > summary").filter({ hasText: "OpenAI" }).click();
  const model = configuration.locator('[data-member-model-id="gpt-5.4"]');
  await model.getByRole("button", { name: "Disable gpt-5.4" }).click();
  await model.getByRole("textbox", { name: "Input token price for gpt-5.4", exact: true }).fill("1.75");
  await model.getByRole("textbox", { name: "Output token price for gpt-5.4", exact: true }).fill("4.5");
  await expect(configuration.locator("> summary")).toContainText("Enabled: 1/2");
  await dialog.getByRole("button", { name: "Save policy" }).click();

  const update = await page.evaluate(() => {
    const calls = (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: { input?: Record<string, unknown> } }> }).__TAURI_TEST_INVOKES__;
    return calls.findLast((call) => call.command === "update_local_source")?.args.input;
  });
  expect(update).toMatchObject({
    allowedModels: ["gpt-5.4-mini"],
    excludedModels: ["gpt-5.4"],
    modelPriceOverrides: {
      "gpt-5.4": {
        inputMicroUsdPerMillion: 1_750_000,
        outputMicroUsdPerMillion: 4_500_000,
      },
    },
  });
});

test("background account updates refresh the visible runtime", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true });
  await page.goto("/");
  await page.waitForTimeout(300);
  const countRuntimeReads = () => page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string }> }).__TAURI_TEST_INVOKES__.filter((call) => call.command === "get_local_runtime_state").length);
  const before = await countRuntimeReads();

  await emitTauriEvent(page, "zenith-state-changed", null);

  await expect.poll(countRuntimeReads).toBeGreaterThan(before);
});

test("Pool and Connections refresh the visible account quota after a background state event", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, quotaAvailable: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Pool", exact: true }).click();

  const poolAccount = page.locator('[data-member-label="Personal Plus"]');
  const poolPrimaryQuota = poolAccount.locator(".quota-meter-heading > strong").first();
  await expect(poolPrimaryQuota).toHaveText("72%");
  await page.evaluate(() => {
    const internals = (window as unknown as { __TAURI_INTERNALS__: { invoke: (command: string, args?: unknown, options?: unknown) => Promise<unknown> } }).__TAURI_INTERNALS__;
    const invoke = internals.invoke.bind(internals);
    const quotaValues = [4300, 2100];
    internals.invoke = async (command, args, options) => {
      const result = await invoke(command, args, options);
      if (command !== "get_local_runtime_state") return result;
      const snapshot = structuredClone(result) as { accounts: Array<{ quota: { primary: { availableBasisPoints: number } | null } }> };
      const primary = snapshot.accounts[0]?.quota.primary;
      if (primary) primary.availableBasisPoints = quotaValues.shift() ?? primary.availableBasisPoints;
      return snapshot;
    };
  });
  const stateReads = () => page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string }> }).__TAURI_TEST_INVOKES__.filter((call) => call.command === "get_local_runtime_state").length);
  const before = await stateReads();

  await emitTauriEvent(page, "zenith-state-changed", null);

  await expect.poll(stateReads).toBeGreaterThan(before);
  await expect(poolPrimaryQuota).toHaveText("43%");

  await page.getByRole("button", { name: "Connections", exact: true }).click();
  const connectionAccount = page.locator(".account-card").filter({ hasText: "Personal Plus" });
  const connectionPrimaryQuota = connectionAccount.locator(".quota-meter-heading > strong").first();
  await expect(connectionPrimaryQuota).toHaveText("43%");
  const connectionReadsBefore = await stateReads();

  await emitTauriEvent(page, "zenith-state-changed", null);

  await expect.poll(stateReads).toBeGreaterThan(connectionReadsBefore);
  await expect(connectionPrimaryQuota).toHaveText("21%");
});

test("OAuth callback offers pool and stored proxy setup for the added account", async ({ page }) => {
  await installTauriMock(page, { locale: "en", mode: "local", populated: false, proxyCount: 1 });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await page.getByRole("button", { name: "Sign in", exact: true }).first().click();
  const dialog = page.getByRole("dialog", { name: "Sign in" });
  await expect(dialog.getByText("Waiting for sign-in", { exact: true })).toBeVisible();

  await emitTauriEvent(page, "relay-oauth-status", { loginId: "oauth_synthetic", status: "callback_received" });

  await expect(dialog).toHaveCount(0);
  await expect(page.locator(".global-feedback.success")).toHaveText("Account added.");
  const setup = page.getByRole("dialog", { name: "Account added" });
  await expect(setup.getByLabel("Add account to pool")).toBeChecked();
  await expect(setup.getByLabel("Assign a stored proxy")).not.toBeChecked();
  await setup.getByRole("button", { name: "Done" }).click();
  await expect(setup).toHaveCount(0);
  const calls = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string }> }).__TAURI_TEST_INVOKES__);
  expect(calls.map((call) => call.command)).toEqual(expect.arrayContaining(["complete_codex_oauth", "set_local_pool_membership"]));
  expect(calls.some((call) => call.command === "assign_free_local_account_proxies")).toBe(false);
});

test("local proxy storage warns, detaches accounts, and deletes selected endpoints", async ({ page }) => {
  await installTauriMock(page, { locale: "en", mode: "local", populated: true, accountCount: 3, proxyCount: 3 });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await page.getByRole("tab", { name: "Proxies" }).click();

  const summary = page.locator(".proxy-storage-counts");
  await expect(summary).toContainText("Total3");
  await expect(summary).toContainText("Free2");
  await expect(summary).toContainText("Assigned1");
  await expect(page.locator(".proxy-storage-list")).not.toContainText("secret");
  await expect(page.locator(".proxy-storage-row").first()).toContainText("United States");

  await page.getByRole("button", { name: "Manage assigned accounts" }).first().click();
  let manager = page.getByRole("dialog", { name: "Proxy accounts" });
  await expect(manager.getByText("Business Workspace", { exact: true })).toBeVisible();
  await manager.getByText("Personal Plus", { exact: true }).click();
  await manager.getByRole("button", { name: "Save" }).click();
  await expect(page.locator(".proxy-storage-account-count").first()).toHaveText("Business Workspace+1");
  await expect(page.locator(".proxy-storage-account-count").first()).toHaveAttribute("title", "Business Workspace, Personal Plus");

  await page.getByRole("button", { name: "Import", exact: true }).click();
  const importDialog = page.getByRole("dialog", { name: "Import proxies" });
  await importDialog.getByLabel("Proxy list").fill("new-proxy.example.test:12000:user:secret\nsecond-proxy.example.test:12001:user:secret");
  await importDialog.getByRole("button", { name: "Import 2" }).click();
  await expect(importDialog.getByText("Added 2; skipped 0 duplicate(s).", { exact: true })).toBeVisible();
  await importDialog.getByRole("button", { name: "Done" }).click();
  await expect(summary).toContainText("Total5");

  await page.getByLabel("Select all visible proxies").check();
  await page.locator(".proxy-storage-toolbar").getByRole("button", { name: "Delete", exact: true }).click();
  const confirmation = page.getByRole("dialog", { name: "Confirm action" });
  await expect(confirmation).toContainText("1 selected proxy endpoint(s) are used by 2 account(s). Delete all 5 selected proxies and return those accounts to their inherited route?");
  await confirmation.getByRole("button", { name: "Detach and delete" }).click();
  await expect(page.getByText("Proxy storage is empty", { exact: true })).toBeVisible();
  const detachCalls = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: { input?: { accountIds?: string[] } } }> }).__TAURI_TEST_INVOKES__.filter((call) => call.command === "set_local_stored_proxy_accounts" && call.args.input?.accountIds?.length === 0));
  expect(detachCalls).toHaveLength(1);
  expect(detachCalls[0].args.input?.accountIds).toEqual([]);
});

test("account import can reuse an already assigned stored proxy", async ({ page }) => {
  await installTauriMock(page, { locale: "en", mode: "local", populated: true, accountCount: 2, proxyCount: 1 });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await page.getByRole("button", { name: "Import", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "Import accounts" });
  await dialog.getByRole("button", { name: "Choose account files" }).click();
  const assignProxy = dialog.getByLabel("Assign a stored proxy");
  await expect(assignProxy).toBeEnabled();
  await expect(dialog).toContainText("1 stored proxy endpoint(s) available");
  await assignProxy.check();
  await dialog.getByRole("button", { name: "Import 2 account(s)" }).click();
  await expect(dialog).toBeHidden();

  const assignment = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: Record<string, unknown> }> }).__TAURI_TEST_INVOKES__.findLast((call) => call.command === "assign_free_local_account_proxies"));
  expect(assignment?.args).toEqual({ input: { accountIds: ["account_imported_1", "account_imported_2"] } });
});

test("OAuth callback is not lost when the browser redirects before start returns", async ({ page }) => {
  await installTauriMock(page, { locale: "en", mode: "local", populated: false, oauthCallbackBeforeStartReturns: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await page.getByRole("button", { name: "Sign in", exact: true }).first().click();

  await expect(page.getByRole("dialog", { name: "Sign in" })).toHaveCount(0);
  await expect(page.locator(".global-feedback.success")).toHaveText("Account added.");
  const calls = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string }> }).__TAURI_TEST_INVOKES__);
  expect(calls.map((call) => call.command)).toContain("complete_codex_oauth");
});

test("pasted Cockpit arrays reach the Rust batch preview unchanged", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await page.getByRole("button", { name: "Import", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "Import accounts" });
  await expect(dialog).not.toContainText("Cockpit");
  await expect(dialog.getByLabel("Account data or tokens")).toHaveAttribute("placeholder", /JWT/);
  const payload = JSON.stringify([
    { type: "codex", access_token: "synthetic-access-one", account_id: "synthetic-one", email: "one@example.test" },
    { type: "codex", access_token: "synthetic-access-two", account_id: "synthetic-two", email: "two@example.test" },
    { auth_mode: "apikey", OPENAI_API_KEY: "synthetic-api-key", api_base_url: "https://api.example.test/v1", api_provider_name: "Example API" },
  ]);
  await dialog.getByLabel("Account data or tokens").fill(payload);
  await dialog.getByRole("button", { name: "Preview import" }).click();
  await expect(dialog.getByLabel("Select Imported account for import")).toBeChecked();
  await expect(dialog.getByLabel("Select Second imported account for import")).toBeChecked();

  const importedContent = await page.evaluate(() => {
    const calls = (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: { input?: { content?: string } } }> }).__TAURI_TEST_INVOKES__;
    return calls.findLast((call) => call.command === "start_local_account_import")?.args.input?.content;
  });
  expect(importedContent).toBe(payload);
  await dialog.getByRole("button", { name: "Cancel" }).click();
});

test("dropping account files shows progress before the shared import preview", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, importPreviewDelayMs: 500 });
  await page.goto("/");
  await page.getByRole("button", { name: "Usage", exact: true }).click();
  await expect.poll(() => page.evaluate(() => {
    const calls = (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string }> }).__TAURI_TEST_INVOKES__;
    return calls.filter((call) => call.command === "plugin:event|listen").length;
  })).toBeGreaterThanOrEqual(4);
  const paths = ["C:\\Temp\\cockpit-one.json", "C:\\Temp\\sub2api-two.json"];
  await page.evaluate((droppedPaths) => {
    const emit = (window as unknown as { __TAURI_TEST_EMIT__: (event: string, payload: unknown) => void }).__TAURI_TEST_EMIT__;
    emit("tauri://drag-enter", { paths: droppedPaths, position: { x: 200, y: 160 } });
  }, paths);
  await expect(page.getByText("Drop JSON or TXT files to preview accounts")).toBeVisible();
  await page.evaluate((droppedPaths) => {
    const emit = (window as unknown as { __TAURI_TEST_EMIT__: (event: string, payload: unknown) => void }).__TAURI_TEST_EMIT__;
    emit("tauri://drag-drop", { paths: droppedPaths, position: { x: 200, y: 160 } });
  }, paths);

  const dialog = page.getByRole("dialog", { name: "Import accounts" });
  await expect(dialog.getByText("Preparing import", { exact: true })).toBeVisible();
  await expect(dialog.locator(".import-file-loading .spin")).toBeVisible();
  await expect(dialog.getByLabel("Select Imported account for import")).toBeChecked();
  await expect(dialog.getByLabel("Select Second imported account for import")).toBeChecked();
  await expect(dialog.locator('.account-plan-badge[data-plan="k12"]')).toHaveCount(3);
  await expect(page.getByText("Drop JSON or TXT files to preview accounts")).toBeHidden();
  const call = await page.evaluate(() => {
    const calls = (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: { paths?: string[] } }> }).__TAURI_TEST_INVOKES__;
    return calls.findLast((item) => item.command === "preview_local_account_import_files");
  });
  expect(call?.args.paths).toEqual(paths);
});

test("local account import reports live per-account progress", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, importConfirmDelayMs: 1_000 });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await page.getByRole("button", { name: "Import", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "Import accounts" });
  await dialog.getByRole("button", { name: "Choose account files" }).click();
  await dialog.getByRole("button", { name: "Import 2 account(s)" }).click();

  const progress = dialog.locator(".import-progress");
  await expect(progress).toBeVisible();
  await expect(progress).toContainText("Current: Imported account");
  await expect(progress).toContainText("Importing 1 of 2");
  await expect(dialog).toBeHidden();
});

test("failed-only retry sends only the failed account", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, importResult: "item_failure" });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await page.getByRole("button", { name: "Import", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "Import accounts" });
  await dialog.getByRole("button", { name: "Choose account files" }).click();
  await dialog.getByRole("button", { name: "Import 2 account(s)" }).click();
  await dialog.getByRole("button", { name: "Retry failed" }).click();
  await expect(dialog.getByRole("alert")).toBeVisible();

  const selections = await page.evaluate(() => {
    const calls = (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: { input?: { selectedItemIds?: string[] } } }> }).__TAURI_TEST_INVOKES__;
    return calls.filter((call) => call.command === "confirm_local_account_import").map((call) => call.args.input?.selectedItemIds);
  });
  expect(selections).toEqual([
    ["import_0123456789abcdef", "import_1111222233334444"],
    ["import_0123456789abcdef"],
  ]);
});

for (const scenario of [
  { mode: "local", locale: "en", code: "provider_account_id_missing", nav: "Connections", action: "Import", title: "Import accounts", input: "Account data or tokens", preview: "Preview import", confirm: "Import 2 account(s)", heading: "Some accounts were not imported", reason: "The imported record and its token claims do not contain a ChatGPT account ID.", close: "Close" },
  { mode: "local", locale: "ru", code: "models_http_status", nav: "Подключения", action: "Импорт", title: "Импортировать учётные записи", input: "Данные аккаунтов или токены", preview: "Проверить импорт", confirm: "Импортировать: 2", heading: "Часть учётных записей не импортирована", reason: "При проверке доступных моделей провайдер вернул неожиданный ответ.", close: "Закрыть" },
  { mode: "remote", locale: "en", code: "models_forbidden", nav: "Connections", action: "Import", title: "Import accounts", input: "Account data or tokens", preview: "Preview import", confirm: "Import 2 account(s)", heading: "Some accounts were not imported", reason: "The provider denied access to the model list. Check this account's access and proxy region.", close: "Close" },
  { mode: "remote", locale: "ru", code: "item_not_found", nav: "Подключения", action: "Импорт", title: "Импортировать учётные записи", input: "Данные аккаунтов или токены", preview: "Проверить импорт", confirm: "Импортировать: 2", heading: "Часть учётных записей не импортирована", reason: "Не пройдена финальная проверка аккаунта. Обновите его данные или прокси и повторите импорт.", close: "Закрыть" },
] as const) {
  test(`${scenario.mode} ${scenario.locale} import failures identify the safe account and explain the cause`, async ({ page }) => {
    await installTauriMock(page, { mode: scenario.mode, locale: scenario.locale, populated: true, importResult: "item_failure", importFailureCode: scenario.code });
    await page.goto("/");
    await page.getByRole("button", { name: scenario.nav, exact: true }).click();
    await page.getByRole("button", { name: scenario.action, exact: true }).click();
    const dialog = page.getByRole("dialog", { name: scenario.title });
    await dialog.getByLabel(scenario.input).fill('{"accounts":[]}');
    await dialog.getByRole("button", { name: scenario.preview }).click();
    await dialog.getByRole("button", { name: scenario.confirm }).click();
    const alert = dialog.getByRole("alert");
    await expect(alert).toContainText(scenario.heading);
    await expect(alert.getByText("Imported account", { exact: true })).toBeVisible();
    await expect(alert.getByText("im••••ed", { exact: true })).toBeVisible();
    await expect(alert.getByText(scenario.code, { exact: true })).toBeVisible();
    await expect(alert.getByText(scenario.reason, { exact: true })).toBeVisible();
    await expect(alert).not.toContainText("synthetic-access-token");
    await expect(alert).not.toContainText("raw-provider-id");
    await expect(alert).not.toContainText("import_0123456789abcdef");
    await dialog.getByRole("button", { name: scenario.close }).last().click();
    if (scenario.mode === "local") {
      const canceled = await page.evaluate(() => {
        const calls = (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string }> }).__TAURI_TEST_INVOKES__;
        return calls.some((call) => call.command === "cancel_local_account_import");
      });
      expect(canceled).toBe(true);
    }
  });
}

test("missing import session keeps the dialog open with recovery guidance", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, importResult: "not_found" });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await page.getByRole("button", { name: "Import", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "Import accounts" });
  await dialog.getByLabel("Account data or tokens").fill('{"accounts":[]}');
  await dialog.getByRole("button", { name: "Preview import" }).click();
  await dialog.getByRole("button", { name: "Import 2 account(s)" }).click();
  await expect(dialog.getByRole("alert")).toContainText("The operation did not finish");
  await expect(dialog.getByRole("button", { name: "Import 2 account(s)" })).toBeVisible();
  await expect(dialog.getByLabel("Resume import session ID")).toHaveCount(0);
  await expect(page.locator(".global-feedback.error")).toBeVisible();
  const layers = await page.evaluate(() => ({
    feedback: Number.parseInt(getComputedStyle(document.querySelector(".global-feedback")!).zIndex, 10),
    modal: Number.parseInt(getComputedStyle(document.querySelector(".relay-modal-backdrop")!).zIndex, 10),
  }));
  expect(layers.feedback).toBeGreaterThan(layers.modal);
});

test("empty Choose API mode opens the shared source picker", async ({ page }) => {
  await installTauriMock(page, { mode: "zenith", locale: "en", populated: false, readyConnected: false });
  await page.goto("/");
  const zenithTestKey = "test-source-key";
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await expect(page.getByRole("tab", { name: "Sources", exact: true })).toBeVisible();
  await expect(page.getByText("No API sources", { exact: true })).toBeVisible();
  await expect(page.getByRole("button", { name: "Add source", exact: true })).toHaveCount(1);
  await expect(page.getByText("Zenith API", { exact: true })).toHaveCount(0);

  await page.getByRole("button", { name: "Add source", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "Add source" });
  await expect(dialog.locator(".api-provider-title strong")).toHaveText(["OpenAI", "OpenRouter", "Zenith API", "Custom API"]);
  expect(await dialog.getByRole("radio").evaluateAll((items) => items.map((item) => item.getAttribute("aria-checked")))).toEqual(["false", "false", "false", "false"]);
  await expect(dialog.getByText("Recommended", { exact: true })).toHaveCount(0);
  await expect(dialog.getByRole("button", { name: "Get API key", exact: true })).toHaveCount(0);
  await expect(dialog.getByRole("button", { name: "Save", exact: true })).toHaveCount(0);

  await dialog.getByRole("radio", { name: /OpenAI/ }).click();
  await expect(dialog.getByText("Model routing", { exact: true })).toBeVisible();
  const routingDetails = dialog.locator(".source-routing-details");
  await expect(routingDetails).not.toHaveAttribute("open", "");
  await expect(dialog.locator(".source-route-matrix")).toBeHidden();
  await expect(dialog.getByRole("checkbox", { name: "Responses is available from this source", exact: true })).toBeHidden();
  await routingDetails.locator("summary").click();
  await expect(dialog.getByRole("checkbox", { name: "Responses is available from this source", exact: true })).toBeChecked();
  await expect(dialog.getByRole("checkbox", { name: "Chat Completions is available from this source", exact: true })).toHaveCount(1);
  await expect(dialog.getByRole("checkbox", { name: "Messages is available from this source", exact: true })).toHaveCount(1);
  await expect(dialog.locator(".source-route-format-heading")).toHaveCount(3);

  await dialog.getByRole("button", { name: "Edit", exact: true }).click();
  await dialog.getByRole("radio", { name: /OpenRouter/ }).click();
  await dialog.locator(".source-routing-details > summary").click();
  const responses = dialog.getByRole("checkbox", { name: "Responses is available from this source", exact: true });
  const messages = dialog.getByRole("checkbox", { name: "Messages is available from this source", exact: true });
  await expect(responses).toBeChecked();
  await messages.check();
  await expect(messages).toBeChecked();
  await messages.uncheck();
  await expect(responses).toBeChecked();
  await dialog.getByRole("button", { name: "Get API key", exact: true }).click();
  await expect.poll(() => page.evaluate(() => (window as unknown as {
    __TAURI_TEST_INVOKES__: Array<{ command: string; args: Record<string, unknown> }>;
  }).__TAURI_TEST_INVOKES__.some((call) => (
    call.command === "open_api_key_page"
    && call.args.provider === "openrouter"
  )))).toBe(true);
  const key = dialog.getByLabel("Upstream API key");
  await key.focus();
  expect(await key.evaluate((input) => {
    const field = input.closest<HTMLElement>(".secret-field")!;
    return { inputOutline: getComputedStyle(input).outlineStyle, fieldOutline: getComputedStyle(field).outlineWidth };
  })).toEqual({ inputOutline: "none", fieldOutline: "2px" });

  await dialog.getByRole("button", { name: "Edit", exact: true }).click();
  await dialog.getByRole("radio", { name: /Zenith API/ }).click();
  await dialog.getByLabel("Upstream API key").fill(zenithTestKey);
  await dialog.getByRole("button", { name: "Save", exact: true }).click();
  await expect(page.getByText("Zenith API", { exact: true })).toBeVisible();
  await expect(page.getByRole("button", { name: "Mode: Choose API", exact: true })).toBeVisible();
  await page.getByLabel("Launch in ChatGPT").click();
  await expect.poll(() => page.evaluate(() => localStorage.getItem("relay.directSourceId"))).toBe("source_created_1");
  await page.getByRole("button", { name: "Overview", exact: true }).click();
  await expect(page.getByRole("button", { name: /Selected API: Zenith API/ })).toBeVisible();
  await expect(page.locator(".direct-api-metrics")).toContainText("$42.50");
  await expect(page.locator(".direct-api-metrics")).toContainText("987,654");
  await expect(page.locator(".direct-api-models code")).toHaveText(["gpt-5.4", "gpt-5.4-mini", "o3"]);
  const calls = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: Record<string, unknown> }> }).__TAURI_TEST_INVOKES__);
  expect(calls.find((call) => call.command === "create_local_source")?.args.input).toMatchObject({
    name: "Zenith API",
    baseUrl: "https://api.zenithmarket.dev/v1",
    wireApi: "responses",
    protocolBindings: [{
      wireApi: "responses",
      adapter: "native",
      reasoningMode: "disabled",
      modelIds: [],
    }],
    apiKey: zenithTestKey,
  });
  expect(calls.find((call) => call.command === "get_local_source_stats")?.args).toEqual({ sourceId: "source_created_1" });
  expect(calls.map((call) => call.command)).not.toContain("save_key");
  expect(calls.map((call) => call.command)).not.toContain("set_local_pool_membership");
});

test("provider presets leave source protocol verification to the connector", async ({ page }) => {
  await installTauriMock(page, { mode: "zenith", locale: "en", populated: false, readyConnected: false });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await page.getByRole("button", { name: "Add source", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "Add source" });

  await dialog.getByRole("radio", { name: /OpenAI/ }).click();
  const routingDetails = dialog.locator(".source-routing-details");
  await expect(routingDetails).not.toHaveAttribute("open", "");
  await expect(dialog.locator(".source-route-matrix")).toBeHidden();
  await routingDetails.locator("summary").click();
  const responses = dialog.getByRole("checkbox", { name: "Responses is available from this source", exact: true });
  await expect(responses).toBeChecked();
  await expect(dialog.getByRole("checkbox", { name: "Messages is available from this source", exact: true })).not.toBeChecked();
  await expect(dialog.locator(".source-route-format-heading")).toHaveCount(3);

  await dialog.getByLabel("Upstream API key").fill("sk-synthetic-ready-key");
  await dialog.getByRole("button", { name: "Save", exact: true }).click();
  await expect(page.getByText("OpenAI", { exact: true })).toBeVisible();
  const calls = await page.evaluate(() => (window as unknown as {
    __TAURI_TEST_INVOKES__: Array<{ command: string; args: Record<string, unknown> }>;
  }).__TAURI_TEST_INVOKES__);
  expect(calls.find((call) => call.command === "create_local_source")?.args.input).toMatchObject({
    name: "OpenAI",
    baseUrl: "https://api.openai.com/v1",
    wireApi: "responses",
    protocolBindings: [
      {
        wireApi: "responses",
        adapter: "native",
        reasoningMode: "disabled",
        modelIds: [],
      },
    ],
    apiKey: "sk-synthetic-ready-key",
  });
});

test("source editor keeps bridge model ownership explicit", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await page.getByRole("tab", { name: "Sources", exact: true }).click();
  await page.getByRole("button", { name: "Edit", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "Edit source" });

  await dialog.locator(".source-routing-details > summary").click();
  const messages = dialog.getByRole("checkbox", { name: "Messages is available from this source", exact: true });
  await messages.check();
  const nativeMini = dialog.getByRole("checkbox", { name: "Responses for gpt-5.4-mini", exact: true });
  const messageMini = dialog.getByRole("checkbox", { name: "Messages for gpt-5.4-mini", exact: true });
  const bridge = dialog.getByRole("checkbox", { name: "Through Relay for gpt-5.4-mini", exact: true });
  await expect(bridge).not.toBeChecked();
  await expect(dialog.locator(".source-route-model-row").filter({ hasText: "gpt-5.4-mini" })).toHaveCount(1);
  await expect(dialog.getByRole("group", { name: "Messages reasoning" })).toHaveCount(0);
  await nativeMini.uncheck();
  await messageMini.check();
  await expect(bridge).toBeEnabled();
  await bridge.check();
  await expect(bridge).toBeChecked();
  await expect(messageMini).toBeChecked();
  await expect(messageMini).toBeDisabled();
  await page.screenshot({ path: "output/playwright/source-bridge-routes-1160x760.png" });

  await page.setViewportSize({ width: 840, height: 560 });
  expect(await dialog.evaluate((element) => {
    const rect = element.getBoundingClientRect();
    return rect.left >= 0 && rect.right <= innerWidth && rect.top >= 0 && rect.bottom <= innerHeight;
  })).toBe(true);
  expect(await dialog.locator(".source-route-matrix").evaluate((element) => {
    const rect = element.getBoundingClientRect();
    return rect.left >= 0 && rect.right <= innerWidth;
  })).toBe(true);
  await page.screenshot({ path: "output/playwright/source-bridge-routes-840x560.png" });

  await dialog.getByRole("button", { name: "Save", exact: true }).click();
  await expect.poll(() => page.evaluate(() => {
    const calls = (window as unknown as {
      __TAURI_TEST_INVOKES__: Array<{
        command: string;
        args: {
          input?: {
            protocolBindings?: Array<{
              wireApi: string;
              adapter: string;
              reasoningMode: string;
              modelIds: string[];
            }>;
          };
        };
      }>;
    }).__TAURI_TEST_INVOKES__;
    return calls.findLast((call) => call.command === "update_local_source")
      ?.args.input?.protocolBindings;
  })).toEqual([
    {
      wireApi: "responses",
      adapter: "native",
      reasoningMode: "disabled",
      modelIds: ["gpt-5.4"],
    },
    {
      wireApi: "messages",
      adapter: "native",
      reasoningMode: "disabled",
      modelIds: ["gpt-5.4-mini"],
    },
    {
      wireApi: "responses",
      adapter: "responses_to_messages",
      reasoningMode: "disabled",
      modelIds: ["gpt-5.4-mini"],
    },
  ]);
});

test("bridge-only sources stay pool-compatible but cannot launch ChatGPT directly", async ({ page }) => {
  await installTauriMock(page, {
    mode: "local",
    locale: "en",
    populated: true,
    serverModelOrder: ["claude-bridge"],
    sourceProtocolBindings: [{
      wireApi: "responses",
      adapter: "responses_to_messages",
      reasoningMode: "adaptive",
      modelIds: ["claude-bridge"],
    }],
  });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await page.getByRole("tab", { name: "Sources", exact: true }).click();

  const sourceRow = page.getByRole("row").filter({ hasText: "Example compatible API" });
  const launch = sourceRow.getByRole("button", { name: "Launch in ChatGPT", exact: true });
  await expect(launch).toBeDisabled();
  await expect(launch).toHaveAttribute("title", /native Responses API binding/);
});

test("Choose API mode manages and launches saved sources without balance controls", async ({ page }) => {
  await installTauriMock(page, { mode: "zenith", locale: "en", populated: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await expect(page.getByText("Example compatible API", { exact: true })).toBeVisible();
  await expect(page.getByText("Balance", { exact: true })).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Top up", exact: true })).toHaveCount(0);
  await page.getByLabel("Launch in ChatGPT").click();
  await expect.poll(() => page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string }> }).__TAURI_TEST_INVOKES__.some((call) => call.command === "launch_codex_source"))).toBe(true);
  const calls = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: Record<string, unknown> }> }).__TAURI_TEST_INVOKES__);
  expect(calls.find((call) => call.command === "launch_codex_source")?.args).toEqual({ sourceId: "source_synthetic" });
});

test("Choose API overview shows provider statistics and models", async ({ page }) => {
  await installTauriMock(page, { mode: "zenith", locale: "en", populated: true });
  await page.goto("/");
  const metrics = page.locator(".direct-api-metrics");
  await expect(metrics).toContainText("$42.50");
  await expect(metrics).toContainText("$7.50");
  await expect(metrics).toContainText("128");
  await expect(metrics).toContainText("987,654");
  await expect(page.locator(".direct-api-models code")).toHaveText(["gpt-5.4", "gpt-5.4-mini"]);
  await expect(page.getByText("Usage over time", { exact: true })).toHaveCount(0);
});

test("remote pool refreshes provider statistics through the server", async ({ page }) => {
  await installTauriMock(page, { mode: "remote", locale: "en", populated: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Pool", exact: true }).click();

  const source = page.locator('.pool-member-card[data-member-kind="source"]');
  const refresh = source.getByRole("button", { name: "Refresh balance" });
  await expect(refresh).toBeEnabled();
  await refresh.click();
  await expect.poll(() => page.evaluate(() => {
    const calls = (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: Record<string, unknown> }> }).__TAURI_TEST_INVOKES__;
    return calls.findLast((call) => call.command === "get_remote_source_stats")?.args;
  })).toEqual({ sourceId: "source_synthetic" });
});

test("recovery and export controls call the Rust-owned operations", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true });
  await page.goto("/");

  await page.getByRole("button", { name: "Usage", exact: true }).click();
  await page.getByLabel("Actions").click();
  await page.getByRole("menuitem", { name: "Export", exact: true }).click();
  await expect(page.getByText("Redacted export created.")).toBeVisible();
  const usageExport = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: { rows?: Array<{ reasoningTokens?: number }> } }> }).__TAURI_TEST_INVOKES__.findLast((call) => call.command === "export_usage"));
  expect(usageExport?.args.rows?.[0]?.reasoningTokens).toBe(5);

  await page.getByRole("button", { name: "Recovery", exact: true }).click();
  await page.getByRole("button", { name: "Open backups folder" }).click();

  await page.getByRole("button", { name: "Settings", exact: true }).click();
  await page.getByRole("button", { name: "Open data folder" }).click();
  await page.locator(".settings-group").filter({ hasText: "Pool data" }).getByRole("button", { name: "Reset" }).click();
  await settleConfirmation(page, false);

  const commands = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string }> }).__TAURI_TEST_INVOKES__.map((call) => call.command));
  expect(commands).toEqual(expect.arrayContaining(["export_usage", "open_relay_folder"]));
  expect(commands).not.toContain("reset_local_pool_data");
});

test("profile switch reminder can cancel a switch and be disabled", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, profileSwitchBackupPrompt: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();

  const launch = page.getByRole("button", { name: "Launch in ChatGPT" });
  await launch.click();
  const reminder = page.getByRole("dialog", { name: "Before switching ChatGPT" });
  await expect(reminder).toContainText("protected automatic backup");
  await reminder.getByRole("button", { name: "Cancel" }).click();
  expect(await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string }> }).__TAURI_TEST_INVOKES__.some((call) => call.command === "launch_codex_account"))).toBe(false);

  await launch.click();
  await reminder.getByRole("button", { name: "Save and continue" }).click();
  await expect(page.getByText("Client launched.")).toBeVisible();

  await page.getByRole("button", { name: "Settings", exact: true }).click();
  const toggle = page.getByLabel("Remind me about the restore point");
  await expect(toggle).toBeChecked();
  await toggle.uncheck();
  await expect.poll(() => page.evaluate(() => localStorage.getItem("relay.profileSwitchBackupPrompt"))).toBe("0");

  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await page.getByRole("button", { name: "Launch in ChatGPT" }).click();
  await expect(reminder).toHaveCount(0);

  await page.reload();
  await page.getByRole("button", { name: "Settings", exact: true }).click();
  await expect(page.getByLabel("Remind me about the restore point")).not.toBeChecked();
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await page.getByRole("button", { name: "Launch in ChatGPT" }).click();
  await expect(reminder).toHaveCount(0);
  const launches = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string }> }).__TAURI_TEST_INVOKES__.filter((call) => call.command === "launch_codex_account").length);
  expect(launches).toBe(1);
});

test("profile snapshot restore preference defaults to saving and persists when disabled", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Settings", exact: true }).click();

  const toggle = page.getByLabel("Save before restoring");
  await expect(toggle).toBeChecked();
  await toggle.uncheck();
  await expect.poll(() => page.evaluate(() => localStorage.getItem("relay.profileSnapshotBackupBeforeRestore"))).toBe("0");

  await page.getByRole("button", { name: "Recovery", exact: true }).click();
  await page.getByRole("button", { name: "Restore Original profile" }).click();
  const restoreDialog = page.getByRole("dialog", { name: "Restore snapshot" });
  await expect(restoreDialog.getByRole("checkbox", { name: "Save the current profile first" })).not.toBeChecked();
  await restoreDialog.getByRole("button", { name: "Cancel" }).click();

  await page.reload();
  await page.getByRole("button", { name: "Settings", exact: true }).click();
  await expect(page.getByLabel("Save before restoring")).not.toBeChecked();
});

test("confirmed local reset delegates protected restoration to Rust", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Settings", exact: true }).click();
  await page.locator(".settings-group").filter({ hasText: "Pool data" }).getByRole("button", { name: "Reset" }).click();
  await settleConfirmation(page);

  await expect.poll(() => page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string }> }).__TAURI_TEST_INVOKES__.some((call) => call.command === "reset_local_pool_data"))).toBe(true);
  const commands = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string }> }).__TAURI_TEST_INVOKES__.map((call) => call.command));
  expect(commands).not.toContain("create_codex_profile_snapshot");
});

test("local pool reset is hidden outside Computer mode", async ({ page }) => {
  await installTauriMock(page, { mode: "remote", locale: "en", populated: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Settings", exact: true }).click();
  await expect(page.getByText("Reset local pool data", { exact: true })).toHaveCount(0);
});

test("connection search and request ID filters change visible rows", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true });
  await page.goto("/");

  await page.getByRole("button", { name: "Connections", exact: true }).click();
  const connectionSearch = page.getByPlaceholder("Search");
  await connectionSearch.fill("no such account");
  await expect(page.getByText("No matching results")).toBeVisible();
  await connectionSearch.fill("Personal Plus");
  await expect(page.getByText("Personal Plus", { exact: true })).toBeVisible();

  await page.getByRole("button", { name: "Usage", exact: true }).click();
  await page.getByRole("button", { name: "More filters" }).click();
  const requestFilter = page.getByRole("textbox", { name: "Request ID" });
  await requestFilter.fill("missing-request");
  await expect(page.getByText("No matching results")).toBeVisible();
  await requestFilter.fill("req_synthetic_local");
  await expect(page.getByText("req_synthetic_local")).toBeVisible();
});

test("usage pagination follows the errors table", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, usageFailure: true, usageTotalPages: 3 });
  await page.goto("/");
  await page.getByRole("button", { name: "Usage", exact: true }).click();
  await page.getByRole("tab", { name: "Errors", exact: true }).click();

  const pagination = page.getByRole("navigation", { name: "Usage pages" });
  await expect(pagination).toContainText("Page 1 of 3");
  await expect(page.locator(".relay-table-wrap + .usage-pagination")).toBeVisible();
});

test("dense status rows use accessible icons without repeated labels", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true });
  await page.goto("/");

  const activityStatus = page.locator(".activity-section li").first().locator(".relay-status-icon");
  await expect(activityStatus).toHaveAttribute("aria-label", "Success");

  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await page.getByRole("tab", { name: "Sources" }).click();
  await expect(page.locator(".relay-table tbody tr").first().locator(".relay-status-icon")).toHaveAttribute("aria-label", "In rotation");

  await page.getByRole("button", { name: "Pool", exact: true }).click();
  await page.getByRole("tab", { name: "Model Rules" }).click();
  await expect(page.locator(".model-rules tbody tr[data-model-id]").first().locator(".relay-status-icon")).toHaveAttribute("aria-label", "Available");

  await page.getByRole("button", { name: "Usage", exact: true }).click();
  const requestStatus = page.locator(".usage-request-table tbody tr td").nth(1);
  await expect(requestStatus.locator(".relay-status-icon")).toHaveAttribute("aria-label", "Success");
  await expect(requestStatus).toHaveText("");

  await page.getByRole("button", { name: "Recovery", exact: true }).click();
  await expect(page.locator(".profile-snapshot-table tbody tr").first().locator(".relay-status-icon")).toHaveAttribute("aria-label", "Config and sign-in");
});

test("account export supports bulk copy and per-account download", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();

  await page.locator(".account-bulk-menu summary").click();
  await page.getByRole("menuitem", { name: "Export all" }).click();
  let dialog = page.getByRole("dialog", { name: "Export accounts" });
  await expect(dialog.getByRole("radio")).toHaveCount(8);
  await expect(dialog.getByRole("radio", { name: "Zenith" })).toHaveAttribute("aria-checked", "true");
  await expect(dialog.getByRole("button", { name: "Copy JSON" })).toBeEnabled();
  await expect(dialog).not.toContainText("Reusable credentials");
  await expect(dialog.getByRole("checkbox")).toHaveCount(0);
  const markdownDescription = "# Seller package\n\n- Two Business accounts";
  await dialog.locator('input[type="file"][accept*=".md"]').setInputFiles({ name: "offer.md", mimeType: "text/markdown", buffer: Buffer.from(markdownDescription) });
  await expect(dialog.getByRole("heading", { name: "Seller package" })).toBeVisible();
  await dialog.getByRole("button", { name: "Copy JSON" }).click();
  await expect(page.getByText("Account export copied.")).toBeVisible();
  await expect.poll(() => page.evaluate(() => navigator.clipboard.readText())).toContain("synthetic-export-token");
  await expect.poll(() => page.evaluate(() => navigator.clipboard.readText())).toContain("# Seller package");

  await page.locator(".account-card .account-row-menu summary").click();
  await page.getByRole("menuitem", { name: "Export" }).click();
  dialog = page.getByRole("dialog", { name: "Export accounts" });
  await dialog.getByRole("radio", { name: "9router" }).click();
  await dialog.getByRole("button", { name: "Download JSON" }).click();
  await expect(page.getByText("Account export saved.")).toBeVisible();

  const calls = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: Record<string, unknown> }> }).__TAURI_TEST_INVOKES__.filter((call) => call.command === "export_local_accounts"));
  expect(calls.map((call) => call.args.input)).toEqual([
    { accountIds: ["account_synthetic"], format: "zenith", destination: "copy", description: markdownDescription },
    { accountIds: ["account_synthetic"], format: "9router", destination: "download" },
  ]);
});

test("Zenith package descriptions render Markdown without active content", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await page.locator(".account-bulk-menu summary").click();
  await page.getByRole("menuitem", { name: "Export all" }).click();

  const dialog = page.getByRole("dialog", { name: "Export accounts" });
  await dialog.getByLabel("Markdown description").fill([
    "## Safe package",
    "",
    "[Seller page](https://example.invalid)",
    "![Remote image](https://example.invalid/tracker.png)",
    '<img src="invalid" onerror="window.__markdownExecuted = true">',
  ].join("\n"));
  await dialog.getByRole("button", { name: "Preview", exact: true }).click();

  await expect(dialog.getByRole("heading", { name: "Safe package" })).toBeVisible();
  await expect(dialog.getByText("Seller page", { exact: true })).toHaveAttribute("title", "https://example.invalid");
  await expect(dialog.locator(".markdown-description a, .markdown-description img")).toHaveCount(0);
  await expect.poll(() => page.evaluate(() => Boolean((window as unknown as { __markdownExecuted?: boolean }).__markdownExecuted))).toBe(false);
});

test("bulk account export only offers formats that support one JSON document", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, accountCount: 3 });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await page.locator(".account-bulk-menu summary").click();
  await page.getByRole("menuitem", { name: "Export all" }).click();
  const dialog = page.getByRole("dialog", { name: "Export accounts" });
  await expect(dialog.getByRole("radio")).toHaveCount(5);
  await expect(dialog.locator('[role="radio"][data-value="zenith"]')).toBeVisible();
  await expect(dialog.locator('[role="radio"][data-value="sub2api"]')).toBeVisible();
  await expect(dialog.locator('[role="radio"][data-value="cockpit"]')).toBeVisible();
  await expect(dialog.locator('[role="radio"][data-value="9router"]')).toBeVisible();
  await expect(dialog.locator('[role="radio"][data-value="codex_manager"]')).toBeVisible();
  await expect(dialog.locator('[role="radio"][data-value="cpa"]')).toHaveCount(0);
  await expect(dialog.locator('[role="radio"][data-value="codex"]')).toHaveCount(0);
  await expect(dialog.locator('[role="radio"][data-value="axon_hub"]')).toHaveCount(0);
});

test("frequent account actions use full-width zones and secondary actions stay in the menu", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await expect(page.locator(".account-card").first()).toBeVisible();
  const actions = page.locator(".account-card").first().locator(".account-card-actions");
  expect(await actions.locator(":scope > *").evaluateAll((items) => items.map((item) => item.getAttribute("aria-label")))).toEqual([
    "In pool",
    "Refresh quota",
    "Proxy: Common",
    "Launch in ChatGPT",
  ]);
  await page.locator(".account-card .account-row-menu summary").click();
  const menu = page.getByRole("menu");
  await expect(menu.getByRole("menuitem", { name: /^Proxy:/ })).toHaveCount(0);
  await expect(menu.getByRole("menuitem", { name: "Export" })).toBeVisible();
  await expect(menu.getByRole("menuitem")).toHaveCount(3);
  await expect(menu.getByRole("menuitem", { name: "Disable" })).toBeVisible();
  await expect(menu.getByRole("menuitem", { name: "Delete" })).toBeVisible();
});

test("plan filters keep unavailable accounts visible with typed errors", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, accountCount: 3, quotaAvailable: true, accountAuthReason: "invalid_grant" });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();

  const filters = page.locator(".account-filter-stack");
  await expect(filters.locator(".account-filter-menu")).toHaveCount(2);
  await chooseOption(page, filters, "Filter by plan", "business");
  await expect(page.locator(".account-card")).toHaveCount(1);
  await expect(page.locator(".account-card")).toContainText("Business Workspace");
  await expect(page.locator(".account-filter-summary")).toContainText("Showing 1 of 3 accounts");

  await chooseOption(page, filters, "Filter by plan", "errors");
  await expect(page.locator(".account-card")).toHaveCount(2);
  const invalidGrantAccount = page.locator(".account-card").filter({ hasText: "Personal Plus" });
  await expect(invalidGrantAccount.locator(".account-status-button")).toHaveAttribute("aria-label", "Signed out or account changed");
  await expect(invalidGrantAccount).not.toContainText("quota_transport");
  await invalidGrantAccount.locator(".account-status-button").click();
  const errorDialog = page.getByRole("dialog", { name: "Technical error details" });
  const errorJson = JSON.parse(await errorDialog.locator("pre").innerText()) as Record<string, unknown>;
  expect(errorJson).toMatchObject({ code: "auth_invalid_grant", message: "Signed out or account changed", account: "Personal Plus", health: "healthy", auth_state: "requires_reauth", subscription_status: "active", observed_at: null });
  await expect(errorDialog).not.toContainText("test_zenith_source_key");
  await errorDialog.locator("footer").getByRole("button", { name: "Close" }).click();
  await page.getByRole("button", { name: "Clear filters" }).click();
  await expect(page.locator(".account-card")).toHaveCount(3);
  await expect(page.getByText("Showing 2 of 3 accounts")).toHaveCount(0);
});

test("connections and pool share the current account status", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, quotaAvailable: true, staleAccountError: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();

  const account = page.locator(".account-card").filter({ hasText: "Personal Plus" });
  await expect(account.locator(".account-status-button")).toHaveCount(0);
  await expect(account.locator('.relay-status-icon[aria-label="In rotation"]')).toBeVisible();

  await page.getByRole("button", { name: "Pool", exact: true }).click();
  const member = page.locator('[data-member-label="Personal Plus"]');
  await expect(member.locator('.relay-status-icon[aria-label="In rotation"]')).toBeVisible();
});

test("connections follow the live router order", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, accountCount: 6, usageAccountIndex: 3 });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  const labels = page.locator(".account-card .account-identity > strong");

  await expect(labels).toHaveText(["Pro account", "Business Workspace", "Personal Plus", "Backup account", "Quota pending", "Free reserve"]);
  await expect(page.getByRole("button", { name: /Sort accounts/ })).toHaveCount(0);
  await expect(page.getByRole("button", { name: "List view" })).toHaveCount(0);
  await expect(page.locator(".account-priority")).toHaveCount(0);
});

test("connections and pool show the same live account state", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, accountHealth: "degraded", quotaAvailable: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();

  const connection = page.locator(".account-card").filter({ hasText: "Personal Plus" });
  await expect(connection.locator(".relay-status-icon")).toHaveAttribute("aria-label", "In rotation");
  await connection.locator(".relay-status-icon").hover();
  await expect(page.getByRole("tooltip")).toHaveText("In rotation");
  await expect(connection.getByText("Limited", { exact: true })).toHaveCount(0);

  await page.getByRole("button", { name: "Pool", exact: true }).click();
  const member = page.locator('[data-member-label="Personal Plus"]');
  await expect(member.locator('.relay-status-icon[aria-label="In rotation"]')).toBeVisible();
  await expect(member.getByText("Limited", { exact: true })).toHaveCount(0);
});

test("connections and pool show the same exhausted quota state", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, quotaAvailable: false });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();

  const connection = page.locator(".account-card").filter({ hasText: "Personal Plus" });
  await expect(connection.locator('.relay-status-icon[aria-label="Waiting for quota"]')).toBeVisible();
  await expect(connection.locator(".account-status-button")).toHaveCount(0);

  await page.getByRole("button", { name: "Pool", exact: true }).click();
  const member = page.locator('[data-member-label="Personal Plus"]');
  await expect(member.locator('.relay-status-icon[aria-label="Waiting for quota"]')).toBeVisible();
});

test("terminal authentication overrides an exhausted quota state", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, quotaAvailable: false, accountAuthReason: "invalidated_refresh_token" });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();

  const connection = page.locator(".account-card").filter({ hasText: "Personal Plus" });
  await expect(connection.locator(".account-status-button")).toHaveAttribute("aria-label", "Sign-in revoked");
  await expect(connection.getByText("Waiting for quota", { exact: true })).toHaveCount(0);

  await page.getByRole("button", { name: "Pool", exact: true }).click();
  const member = page.locator('[data-member-label="Personal Plus"]');
  await expect(member.locator('.pool-member-kind-icon[data-status="error"]')).toHaveAttribute("aria-label", "Sign-in revoked");
  await expect(member.getByText("Waiting for quota", { exact: true })).toHaveCount(0);
});

test("connections and pool ignore legacy account cooldown state", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, accountHealth: "degraded", quotaAvailable: true, accountCooldown: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();

  const connection = page.locator(".account-card").filter({ hasText: "Personal Plus" });
  await expect(connection.locator(".relay-status-icon")).toHaveAttribute("aria-label", "In rotation");
  await expect(connection.getByText("Waiting for quota", { exact: true })).toHaveCount(0);

  await page.getByRole("button", { name: "Pool", exact: true }).click();
  const member = page.locator('[data-member-label="Personal Plus"]');
  await expect(member.locator('.relay-status-icon[aria-label="In rotation"]')).toBeVisible();
  await expect(member.getByText("Waiting for quota", { exact: true })).toHaveCount(0);
  await expect(member.locator(".pool-member-kind-icon")).not.toHaveAttribute("title", /^Retry after /);
});

test("accounts use cards as the only layout", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, accountCount: 4 });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  const accounts = page.locator(".account-list");

  await expect(accounts).toHaveCSS("display", "grid");
  expect(await accounts.evaluate((list) => getComputedStyle(list).gridTemplateColumns.split(" ").length)).toBe(3);
  await expect(accounts).toContainText("Pro account");
  await expect(page.getByRole("button", { name: "List view" })).toHaveCount(0);
});

test("quota refresh is visible without a destructive bulk cleanup action", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();

  await expect(page.getByRole("button", { name: "Refresh all quotas" })).toBeVisible();
  await page.getByRole("button", { name: "Refresh all quotas" }).click();
  await expect(page.getByText("Updated: 1 · Errors: 0", { exact: true })).toBeVisible();
  await expect(page.getByRole("button", { name: "Refresh and delete non-working accounts" })).toHaveCount(0);
  await page.locator(".account-bulk-menu summary").click();
  await expect(page.getByRole("menuitem", { name: "Refresh all quotas" })).toHaveCount(0);
  await expect(page.getByRole("menuitem", { name: "Refresh and delete non-working accounts" })).toHaveCount(0);
});

test("accounts without quota show the automatic refresh state", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, accountCount: 5 });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  const pending = page.locator(".account-card").filter({ hasText: "Quota pending" });
  await expect(pending.getByText("Waiting for check", { exact: true })).toBeVisible();
});

test("account cards show the subscription end date or an explicit unavailable state", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, accountCount: 3 });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  const cards = page.locator(".account-card");
  const personal = cards.filter({ hasText: "Personal Plus" });
  const business = cards.filter({ hasText: "Business Workspace" });
  await expect(personal.locator('.account-identity .account-plan-badge[data-plan="plus"]')).toHaveText("Plus");
  await expect(business.locator('.account-identity .account-plan-badge[data-plan="business"]')).toHaveText("Business");
  await expect(personal.locator(".account-fact-plan")).toHaveCount(0);
  await expect(personal.locator(".account-subscription-line")).toContainText(/\d{2}\/\d{2}\/\d{4}/);
  await expect(personal.locator(".account-subscription-countdown")).toHaveText(/^\d+ d \d+ h \d+ min$/);
  await expect(business.locator(".account-subscription-line")).toContainText(/\d{2}\/\d{2}\/\d{4}/);
  await expect(business.locator(".account-subscription-countdown")).toHaveText(/^\d+ d \d+ h \d+ min$/);
  await expect(cards.filter({ hasText: "Backup account" }).locator(".account-subscription-line")).toHaveText("Subscription end date unavailable");
});

test("subscription countdown uses live short units in the final minute", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "ru", populated: true, subscriptionExpiresInMs: 70_000 });
  await page.goto("/");
  await page.getByRole("button", { name: "Подключения", exact: true }).click();
  const countdown = page.locator(".account-subscription-countdown");
  await expect(countdown).toHaveText(/^1 мин \d{1,2} с$/);
  const initial = await countdown.textContent();
  await expect.poll(() => countdown.textContent()).not.toBe(initial);
});

test("plan filters and pool controls exclude a selected account without deleting it", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, accountCount: 3 });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  const filters = page.locator(".account-filter-stack");
  await chooseOption(page, filters, "Filter by plan", "free");
  await expect(page.locator(".account-card")).toHaveCount(1);
  await page.getByLabel("Select all accounts").check();
  await page.getByRole("button", { name: "Remove selected from pool", exact: true }).click();

  await chooseOption(page, filters, "Filter by pool participation", "excluded");
  const card = page.locator(".account-card").filter({ hasText: "Backup account" });
  await expect(card).toBeVisible();
  await page.getByLabel("Select all accounts").check();
  await expect(page.getByRole("button", { name: "Add selected to pool", exact: true })).toBeVisible();
  await expect(page.getByRole("button", { name: "Remove selected from pool", exact: true })).toHaveCount(0);
  await page.getByRole("button", { name: "Add selected to pool", exact: true }).click();
  await expect(card).toBeHidden();

  await expect.poll(() => page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string }> }).__TAURI_TEST_INVOKES__.filter((call) => call.command === "set_local_pool_membership").length)).toBe(2);
  const calls = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: Record<string, unknown> }> }).__TAURI_TEST_INVOKES__);
  expect(calls.filter((call) => call.command === "set_local_pool_membership").map((call) => call.args)).toEqual([
    { input: { accountIds: ["account_synthetic_3"], sourceIds: [], inPool: false } },
    { input: { accountIds: ["account_synthetic_3"], sourceIds: [], inPool: true } },
  ]);
  expect(calls.some((call) => call.command === "delete_local_account")).toBe(false);
});

test("bulk account actions stay compact and delete the selected records", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, accountCount: 3 });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await page.getByLabel("Select all accounts").check();

  const actions = page.locator(".account-command-bar > div:last-child");
  await expect(actions.locator(".relay-button")).toHaveCount(0);
  await expect(actions.locator(".relay-icon-button")).toHaveCount(5);
  await expect(actions.getByRole("button", { name: "Add selected to pool" })).toHaveCount(0);
  await expect(actions.getByRole("button", { name: "Remove selected from pool" })).toBeVisible();
  await expect(actions.getByRole("button", { name: "Export selected (3)" })).toBeVisible();

  await actions.getByRole("button", { name: "Delete selected accounts" }).click();
  await settleConfirmation(page);
  await expect(page.getByText("No accounts", { exact: true })).toBeVisible();

  const deleted = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: { accountIds?: string[] } }> }).__TAURI_TEST_INVOKES__.findLast((call) => call.command === "delete_local_accounts")?.args.accountIds);
  expect([...(deleted ?? [])].sort()).toEqual(["account_synthetic", "account_synthetic_2", "account_synthetic_3"].sort());
});

test("selected local accounts move to the server and remain as inactive local records", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, accountCount: 2, codexBoundOauthAccountId: "account_synthetic" });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await page.getByLabel("Select all accounts").check();
  await page.getByRole("button", { name: "Move to server", exact: true }).click();

  const dialog = page.getByRole("dialog", { name: "Move to server" });
  await expect(dialog).toContainText("They will join its pool and stop participating in local routing.");
  await dialog.getByRole("button", { name: "Move", exact: true }).click();

  const profileDialog = page.getByRole("dialog", { name: "Switch ChatGPT to the server" });
  await expect(profileDialog).toContainText("currently uses one of the selected accounts directly");
  await profileDialog.getByRole("button", { name: "Switch and continue", exact: true }).click();

  const progress = page.locator(".account-transfer-progress");
  await expect(progress).toBeVisible();
  await expect(progress.locator("li")).toHaveCount(2);
  await expect(progress).toContainText("Personal Plus");
  await expect(progress).toContainText("Business Workspace");
  await expect(page.locator(".account-card")).toHaveCount(2);
  await expect(page.locator('.account-card input[role="switch"]:checked')).toHaveCount(0);
  const serverAccountIndicator = page.getByRole("button", { name: "This account runs on the user-managed server and does not participate in the local pool.", exact: true });
  await expect(serverAccountIndicator).toHaveCount(2);
  await expect(progress).toHaveCount(0);
  await page.getByLabel("Select all accounts").check();
  await expect(page.getByRole("button", { name: "Move to server", exact: true })).toBeDisabled();
  const call = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: { input?: { accountIds?: string[] } } }> }).__TAURI_TEST_INVOKES__.findLast((item) => item.command === "move_local_accounts_to_remote"));
  expect([...(call?.args.input?.accountIds ?? [])].sort()).toEqual(["account_synthetic", "account_synthetic_2"].sort());
  await expect.poll(() => page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string }> }).__TAURI_TEST_INVOKES__.filter((item) => item.command === "attach_codex_to_remote_gateway").length)).toBe(1);

  await page.locator(".account-card").filter({ hasText: "Personal Plus" }).locator(".account-row-menu summary").click();
  await page.getByRole("menuitem", { name: "Return to this computer" }).click();
  const returnDialog = page.getByRole("dialog", { name: "Return to this computer" });
  await expect(returnDialog).toContainText("validate the latest server session");
  await returnDialog.getByRole("button", { name: "Return", exact: true }).click();
  await expect(serverAccountIndicator).toHaveCount(1);
  const returned = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: { input?: { localAccountId?: string } } }> }).__TAURI_TEST_INVOKES__.findLast((item) => item.command === "return_remote_account_to_local"));
  expect(returned?.args.input?.localAccountId).toBe("account_synthetic");

  await page.locator(".account-card").filter({ hasText: "Business Workspace" }).locator(".account-row-menu summary").click();
  await page.getByRole("menuitem", { name: "Use local recovery copy" }).click();
  const recoveryDialog = page.getByRole("dialog", { name: "Use local recovery copy" });
  await expect(recoveryDialog).toContainText("two copies may briefly use the same session");
  await recoveryDialog.getByRole("button", { name: "Activate locally", exact: true }).click();
  await expect(serverAccountIndicator).toHaveCount(0);
  const recovered = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: { input?: { localAccountId?: string; confirmRemoteMayStillBeRunning?: boolean } } }> }).__TAURI_TEST_INVOKES__.findLast((item) => item.command === "force_activate_remote_account_locally"));
  expect(recovered?.args.input).toEqual({ localAccountId: "account_synthetic_2", confirmRemoteMayStillBeRunning: true });
});

test("failed server move restores the direct ChatGPT profile", async ({ page }) => {
  await installTauriMock(page, {
    mode: "local",
    locale: "en",
    populated: true,
    codexBoundOauthAccountId: "account_synthetic",
    moveAccountsError: true,
  });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await page.getByLabel("Select all accounts").check();
  await page.getByRole("button", { name: "Move to server", exact: true }).click();
  await page.getByRole("dialog", { name: "Move to server" }).getByRole("button", { name: "Move", exact: true }).click();
  await page.getByRole("dialog", { name: "Switch ChatGPT to the server" }).getByRole("button", { name: "Switch and continue", exact: true }).click();

  await expect(page.getByText("On server", { exact: true })).toHaveCount(0);
  await expect.poll(() => page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string }> }).__TAURI_TEST_INVOKES__.filter((item) => item.command === "restore_codex_account_profile").length)).toBe(1);
  const commands = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string }> }).__TAURI_TEST_INVOKES__.map((item) => item.command));
  expect(commands).toEqual(expect.arrayContaining(["attach_codex_to_remote_gateway", "move_local_accounts_to_remote", "restore_codex_account_profile"]));
});

test("bulk deletion only removes accounts selected by the active filter", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, accountCount: 3 });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();

  const personalSelection = page.locator(".account-card").filter({ hasText: "Personal Plus" }).locator(".account-select-button");
  await expect(personalSelection).toHaveAttribute("aria-pressed", "false");
  await personalSelection.click();
  await expect(personalSelection).toHaveAttribute("aria-pressed", "true");
  await page.getByRole("button", { name: "Clear selection" }).click();
  await chooseOption(page, page.locator(".account-filter-stack"), "Filter by plan", "free");
  await page.getByLabel("Select all accounts").check();
  await page.getByRole("button", { name: "Delete selected accounts" }).click();
  await settleConfirmation(page);

  const deleted = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: { accountId?: string } }> }).__TAURI_TEST_INVOKES__.filter((call) => call.command === "delete_local_account").map((call) => call.args.accountId));
  expect(deleted).toEqual(["account_synthetic_3"]);
});

test("icon actions explain themselves and scrollbars follow the active theme", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "ru", theme: "light", populated: true, accountCount: 3 });
  await page.setViewportSize({ width: 840, height: 560 });
  await page.goto("/");
  await page.getByRole("button", { name: "Подключения", exact: true }).click();

  const refreshAll = page.getByRole("button", { name: "Обновить все квоты" });
  await refreshAll.hover();
  const tooltip = page.getByRole("tooltip");
  await expect(tooltip).toHaveText("Обновить все квоты");
  const box = await tooltip.boundingBox();
  expect(box).not.toBeNull();
  expect(box!.x).toBeGreaterThanOrEqual(8);
  expect(box!.x + box!.width).toBeLessThanOrEqual(832);

  await page.mouse.move(2, 2);
  await expect(tooltip).toHaveCount(0);
  await refreshAll.focus();
  await expect(page.getByRole("tooltip")).toHaveCount(0);
  await page.keyboard.press("Shift+Tab");
  await page.keyboard.press("Tab");
  await expect(page.getByRole("tooltip")).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(page.getByRole("tooltip")).toHaveCount(0);

  const readScrollbarTheme = () => page.evaluate(() => {
    const root = getComputedStyle(document.documentElement);
    const content = getComputedStyle(document.querySelector(".relay-content")!);
    return {
      thumb: root.getPropertyValue("--relay-scrollbar-thumb").trim(),
      hover: root.getPropertyValue("--relay-scrollbar-thumb-hover").trim(),
      scrollbar: content.getPropertyValue("scrollbar-color"),
    };
  });
  const light = await readScrollbarTheme();
  await page.evaluate(() => { document.documentElement.dataset.theme = "dark"; });
  const dark = await readScrollbarTheme();
  expect(light.thumb).toBe("#aab6bb");
  expect(dark.thumb).toBe("#536169");
  expect(light.hover).not.toBe(dark.hover);
  expect(light.scrollbar).not.toBe(dark.scrollbar);
});

test("pool summary shows routing states and current errors", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, accountCount: 3 });
  await page.goto("/");
  await page.getByRole("button", { name: "Pool", exact: true }).click();
  const summary = page.locator(".pool-summary");
  await expect(summary.locator("div")).toHaveCount(4);
  await expect(summary.locator("div").nth(0)).toHaveText("In rotation3");
  await expect(summary.locator("div").nth(1)).toHaveText("Waiting for quota1");
  await expect(summary.locator("div").nth(2)).toHaveText("With errors1");
  await expect(summary.locator("div").nth(3)).toHaveText("Disabled0");
});

for (const mode of ["local", "remote"] as const) {
  test(`${mode} pool stays off Usage and uses the lightweight runtime snapshot`, async ({ page }) => {
    await installTauriMock(page, {
      mode,
      locale: "en",
      populated: true,
      usageActive: false,
    });
    await page.goto("/");
    const usageCommand = mode === "local" ? "get_local_usage_page" : "get_remote_server_usage";
    const usageReads = () => page.evaluate((command) => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string }> }).__TAURI_TEST_INVOKES__.filter((call) => call.command === command).length, usageCommand);
    const before = await usageReads();
    await page.getByRole("button", { name: "Pool", exact: true }).click();
    await page.waitForTimeout(300);

    const routing = page.locator(".pool-priority-label");
    const member = page.locator('[data-member-label="Personal Plus"]');
    await expect(routing.locator("[data-ready-route]")).toHaveCount(0);
    expect(await usageReads()).toBe(before);
    await expect(member.locator(".pool-member-kind-icon")).toHaveAttribute("aria-label", "Waiting for quota");
  });
}

test("pool account avatar alone carries routing and quota refresh status", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, quotaAvailable: true, quotaRefreshStatus: "refreshing" });
  await page.goto("/");
  await page.getByRole("button", { name: "Pool", exact: true }).click();

  const member = page.locator('[data-member-label="Personal Plus"]');
  const indicator = member.locator(".pool-member-kind-icon");
  await expect(member.locator(".pool-member-state")).toHaveCount(0);
  await expect(indicator).toHaveAttribute("data-status", "disabled");
  await expect(indicator).toHaveAttribute("aria-label", "Checking quota · In rotation");
  await expect(indicator).not.toHaveClass(/refreshing/);
  await indicator.hover();
  await expect(page.getByRole("tooltip")).toHaveText("Checking quota · In rotation");
  expect(await member.locator(".pool-member-actions .relay-icon-button").evaluateAll((buttons) => buttons.every((button) => {
    const rect = button.getBoundingClientRect();
    return rect.width >= 60 && rect.height >= 44;
  }))).toBe(true);
});

test("pool account avatar opens the shared error details", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, accountCount: 3 });
  await page.goto("/");
  await page.getByRole("button", { name: "Pool", exact: true }).click();

  const member = page.locator('[data-member-label="Backup account"]');
  await member.getByRole("button", { name: "Connection error" }).click();
  const dialog = page.getByRole("dialog", { name: "Technical error details" });
  await expect(dialog).toBeVisible();
  await expect(dialog.locator("pre")).toContainText('"code": "quota_transport"');
});

test("pool places unavailable accounts after routable members", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, accountCount: 4, accountAuthReason: "invalid_grant" });
  await page.goto("/");
  await page.getByRole("button", { name: "Pool", exact: true }).click();

  await expect(page.locator(".pool-member-card").last()).toHaveAttribute("data-member-label", "Personal Plus");
});

test("connections stay outside the pool until the user adds selected members", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, accountCount: 3, poolMembers: false, gatewayRunning: false });
  await page.goto("/");
  await page.getByRole("button", { name: "Pool", exact: true }).click();
  await expect(page.getByText("No pool members", { exact: true })).toBeVisible();
  await expect(page.getByRole("button", { name: "Start pool", exact: true })).toBeEnabled();

  await page.getByRole("button", { name: "Add member", exact: true }).first().click();
  const dialog = page.getByRole("dialog", { name: "Add connections to pool" });
  await dialog.getByText("Business Workspace", { exact: true }).click();
  await dialog.getByRole("button", { name: "Add selected (1)" }).click();

  const rows = page.locator(".pool-member-card");
  await expect(rows).toHaveCount(1);
  await expect(rows.first()).toContainText("Business Workspace");
  await rows.first().getByRole("button", { name: "Remove from pool: Business Workspace" }).click();
  const confirmation = page.getByRole("dialog", { name: "Confirm action" });
  await expect(confirmation).toContainText("Remove Business Workspace from the pool?");
  await confirmation.getByRole("button", { name: "Remove from pool" }).click();
  await expect(page.getByText("No pool members", { exact: true })).toBeVisible();
  const calls = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: Record<string, unknown> }> }).__TAURI_TEST_INVOKES__.filter((item) => item.command === "set_local_pool_membership"));
  expect(calls.map((call) => call.args)).toEqual([
    { input: { accountIds: ["account_synthetic_2"], sourceIds: [], inPool: true } },
    { input: { accountIds: ["account_synthetic_2"], sourceIds: [], inPool: false } },
  ]);
});

test("pool member removal on right click skips confirmation", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, accountCount: 3 });
  await page.goto("/");
  await page.getByRole("button", { name: "Pool", exact: true }).click();

  const member = page.locator(".pool-member-card").first();
  const memberLabel = await member.getAttribute("data-member-label");
  expect(memberLabel).toBeTruthy();
  const remove = member.getByRole("button", { name: /Remove from pool:/ });
  await remove.click({ button: "right" });

  await expect(page.getByRole("dialog", { name: "Confirm action" })).toHaveCount(0);
  await expect(page.locator(".pool-member-card").filter({ hasText: memberLabel! })).toHaveCount(0);
  await expect.poll(() => page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string }> }).__TAURI_TEST_INVOKES__.filter((call) => call.command === "set_local_pool_membership").length)).toBe(1);
});

for (const mode of ["local", "remote"] as const) {
  test(`${mode} API sources expose routing role and pool membership`, async ({ page }) => {
    await installTauriMock(page, { mode, locale: "en", populated: true });
    await page.goto("/");
    await page.getByRole("button", { name: "Connections", exact: true }).click();
    await page.getByRole("tab", { name: "Sources" }).click();

    const row = page.getByRole("row").filter({ hasText: "Example compatible API" });
    await expect(row).not.toContainText("Stabilizer");
    await row.locator(".relay-action-menu summary").click();
    await page.getByRole("menuitem", { name: "Remove from pool" }).click();
    await row.locator(".relay-action-menu summary").click();
    await page.getByRole("menuitem", { name: "Add to pool" }).click();

    const calls = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: Record<string, unknown> }> }).__TAURI_TEST_INVOKES__);
    if (mode === "local") {
      expect(calls.filter((call) => call.command === "set_local_pool_membership").map((call) => call.args)).toEqual([
        { input: { accountIds: [], sourceIds: ["source_synthetic"], inPool: false } },
        { input: { accountIds: [], sourceIds: ["source_synthetic"], inPool: true } },
      ]);
    } else {
      expect(calls.filter((call) => call.command === "execute_remote_server_action").map((call) => call.args.input).filter((input) => (input as { action?: { type?: string } }).action?.type === "set_pool_membership")).toEqual([
        { action: { type: "set_pool_membership" }, payload: { accountIds: [], sourceIds: ["source_synthetic"], inPool: false } },
        { action: { type: "set_pool_membership" }, payload: { accountIds: [], sourceIds: ["source_synthetic"], inPool: true } },
      ]);
    }
  });

  test(`${mode} pool creates an API source through the shared picker and applies all three routing roles`, async ({ page }) => {
    await installTauriMock(page, { mode, locale: "en", populated: false, gatewayRunning: false });
    await page.goto("/");
    await page.getByRole("button", { name: "Pool", exact: true }).click();
    await page.getByRole("button", { name: "Add member", exact: true }).first().click();
    await page.getByRole("dialog", { name: "Add connections to pool" }).getByRole("button", { name: "Add API source" }).click();

    const sourceDialog = page.getByRole("dialog", { name: "Add API source" });
    await sourceDialog.getByRole("radio", { name: /Custom API/ }).click();
    await sourceDialog.getByLabel("Name", { exact: true }).fill("Failover API");
    await sourceDialog.getByLabel("API address").fill("https://failover.example.invalid/v1");
    await sourceDialog.getByLabel("Upstream API key").fill("synthetic-upstream-key");
    await sourceDialog.getByRole("button", { name: "Save" }).click();

    const member = page.locator(".pool-member-card").filter({ hasText: "Failover API" });
    await expect(member).toContainText("Stabilizer");
    await member.getByRole("button", { name: "Pool member policy: Failover API" }).click();
    let editor = page.getByRole("dialog", { name: /Pool member policy/ });
    await editor.getByRole("radiogroup", { name: "API source role" }).getByRole("radio", { name: /API first/ }).click();
    await editor.getByRole("button", { name: "Save policy" }).click();
    await expect(member).toContainText("API first");

    await member.getByRole("button", { name: "Pool member policy: Failover API" }).click();
    editor = page.getByRole("dialog", { name: /Pool member policy/ });
    await editor.getByRole("radiogroup", { name: "API source role" }).getByRole("radio", { name: /Last resort/ }).click();
    await editor.getByRole("button", { name: "Save policy" }).click();
    await expect(member).toContainText("Last resort");

    const calls = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: Record<string, unknown> }> }).__TAURI_TEST_INVOKES__);
    if (mode === "local") {
      expect(calls.find((call) => call.command === "create_local_source")?.args.input).toMatchObject({ name: "Failover API", priority: 0 });
      expect(calls.find((call) => call.command === "set_local_pool_membership")?.args).toEqual({ input: { accountIds: [], sourceIds: ["source_created_1"], inPool: true } });
      expect(calls.filter((call) => call.command === "update_local_source").map((call) => (call.args.input as { priority: number }).priority)).toEqual([1_000_001, -1_000_000]);
    } else {
      const actions = calls.filter((call) => call.command === "execute_remote_server_action").map((call) => call.args.input as { action: { type: string }; payload?: Record<string, unknown> });
      expect(actions.find((call) => call.action.type === "create_source")?.payload).toMatchObject({ name: "Failover API", priority: 0 });
      expect(actions.find((call) => call.action.type === "set_pool_membership")?.payload).toMatchObject({ sourceIds: ["source_remote_created_1"] });
      expect(actions.filter((call) => call.action.type === "update_source").map((call) => call.payload?.priority)).toEqual([1_000_001, -1_000_000]);
    }
  });

}

test("pool keeps access key management internal", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Pool", exact: true }).click();
  await expect(page.locator(".relay-tabs").getByRole("tab")).toHaveText(["Members", "Model Rules"]);
  await expect(page.getByRole("tab", { name: "Client Access" })).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Create access key" })).toHaveCount(0);
});

test("pool priority follows the backend scheduler order without display heuristics", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, accountCount: 4, usageAccountIndex: 3 });
  await page.goto("/");
  await page.getByRole("button", { name: "Pool", exact: true }).click();
  await expect(page.locator(".pool-sort-menu")).toHaveCount(0);
  const priority = page.locator(".pool-priority-label");
  await expect(priority).toContainText("Usage order");
  await expect(priority).toContainText("Active now: Pro account");
  await expect(priority.locator("[data-active-models]")).toHaveAttribute("data-active-models", "gpt-5.4:1");
  await expect(priority.locator("[data-active-models]")).toHaveText("Active now (1): gpt-5.4");
  await expect(page.locator(".pool-member-card").first()).toHaveAttribute("data-member-label", "Pro account");
  await expect(page.locator(".pool-member-card").first()).toHaveAttribute("data-current", "true");
  const names = () => page.locator(".pool-member-card").evaluateAll((items) => items.map((item) => item.getAttribute("data-member-label") ?? ""));
  expect(await names()).toEqual(["Pro account", "Business Workspace", "Example compatible API", "Personal Plus", "Backup account"]);
  await expect(page.locator(".pool-member-list")).not.toContainText("Priority 30");
});

test("pool groups concurrent requests by their active model", async ({ page }) => {
  await installTauriMock(page, {
    mode: "local",
    locale: "en",
    populated: true,
    accountCount: 4,
    usageAccountIndex: 3,
    activeModelCounts: [
      { model: "gpt-5.4", requestCount: 3 },
      { model: "gpt-5.4-mini", requestCount: 2 },
    ],
  });
  await page.goto("/");
  await page.getByRole("button", { name: "Pool", exact: true }).click();

  const activeModels = page.locator(".pool-priority-label [data-active-models]");
  await expect(activeModels).toHaveAttribute("data-active-request-count", "5");
  await expect(activeModels).toHaveAttribute("data-active-models", "gpt-5.4:3,gpt-5.4-mini:2");
  await expect(activeModels).toHaveText("Active now (5): gpt-5.4 ×3 · gpt-5.4-mini ×2");
  await expect(page.locator('.pool-member-card[data-member-label="Pro account"]')).toHaveAttribute("data-current", "true");
});

test("pool reflects active models from the live runtime order", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, usageActive: false });
  await page.goto("/");
  await page.getByRole("button", { name: "Pool", exact: true }).click();
  await expect(page.locator('.pool-priority-label [data-ready-route="source_synthetic"]')).toHaveCount(0);

  await page.evaluate(() => {
    const internals = (window as unknown as { __TAURI_INTERNALS__: { invoke: (command: string, args?: unknown, options?: unknown) => Promise<unknown> } }).__TAURI_INTERNALS__;
    const invoke = internals.invoke.bind(internals);
    internals.invoke = async (command, args, options) => {
      const result = await invoke(command, args, options);
      if (command !== "get_local_runtime_order") return result;
      const order = structuredClone(result) as Array<{
        candidateId: string;
        inFlight: number;
        activeRequestCount: number;
        activeModels: Array<{ model: string; requestCount: number }>;
      }>;
      const account = order.find((candidate) => candidate.candidateId === "account_synthetic");
      if (account) {
        account.inFlight = 3;
        account.activeRequestCount = 3;
        account.activeModels = [
          { model: "gpt-5.4", requestCount: 2 },
          { model: "gpt-5.4-mini", requestCount: 1 },
        ];
      }
      return order;
    };
  });

  const activeModels = page.locator(".pool-priority-label [data-active-models]");
  await expect(activeModels).toHaveText("Active now (3): gpt-5.4 ×2 · gpt-5.4-mini");
  await expect(page.locator('[data-member-label="Personal Plus"]')).toHaveAttribute("data-current", "true");
});

test("pool keeps the last completed route visible after its lease is released", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, accountCount: 4, usageAccountIndex: 3, usageActive: false });
  await page.goto("/");
  await page.getByRole("button", { name: "Pool", exact: true }).click();
  const priority = page.locator(".pool-priority-label");
  await expect(priority).toContainText("Usage order");
  await expect(priority).toContainText("Last request: Pro account");
  await expect(priority.locator("[data-ready-route]")).toHaveCount(0);
  await expect(page.locator('.pool-member-card[data-member-label="Pro account"]')).toHaveAttribute("data-last-used", "true");
  await expect(page.locator(".pool-member-card[data-current=true]")).toHaveCount(0);
});

test("pool does not show the next route's models before any request", async ({ page }) => {
  await installTauriMock(page, {
    mode: "local",
    locale: "en",
    populated: true,
    mixedModels: true,
    quotaAvailable: true,
    usagePresent: false,
    usageActive: false,
  });
  await page.goto("/");
  await page.getByRole("button", { name: "Pool", exact: true }).click();

  const priority = page.locator(".pool-priority-label");
  await expect(priority).toContainText("Next choice: Personal Plus");
  await expect(priority.locator("[data-ready-route]")).toHaveCount(0);
  await expect(priority).not.toContainText("gpt-5.4");
  await expect(priority).not.toContainText("claude-opus-4-8");
});

test("pool member picker lists individual accounts instead of subscription groups", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, accountCount: 4, poolMembers: false });
  await page.goto("/");
  await page.getByRole("button", { name: "Pool", exact: true }).click();
  await page.getByRole("button", { name: "Add member", exact: true }).first().click();
  const dialog = page.getByRole("dialog", { name: "Add connections to pool" });
  const accountRows = dialog.locator(".pool-member-picker section").first().locator(".pool-member-options > label");
  await expect(accountRows).toHaveCount(4);
  await expect(accountRows.locator("strong")).toHaveText(["Personal Plus", "Pro account", "Business Workspace", "Backup account"]);
  await expect(accountRows.locator("small")).toHaveCount(0);
  await expect(accountRows.locator(".account-plan-badge")).toHaveText(["Plus", "Pro", "Business", "Free"]);

  await expect(dialog.getByRole("group", { name: "Filter by plan" }).getByRole("button")).toHaveCount(5);
  await dialog.getByRole("button", { name: "Business (1)" }).click();
  await expect(accountRows).toHaveCount(1);
  await expect(accountRows).toContainText("Business Workspace");
  await dialog.getByRole("button", { name: "Select shown (1)" }).click();
  await expect(dialog.getByRole("button", { name: "Add selected (1)" })).toBeEnabled();

  await dialog.getByRole("button", { name: "Select all (5)" }).click();
  await expect(dialog.getByRole("button", { name: "Add selected (5)" })).toBeEnabled();
  await dialog.getByRole("button", { name: "Clear selection" }).click();
  await expect(dialog.getByRole("button", { name: "Add selected (0)" })).toBeDisabled();

  await dialog.getByRole("button", { name: "All (4)" }).click();
  await dialog.getByLabel("Find an account").fill("team");
  await expect(accountRows).toHaveCount(1);
  await expect(accountRows).toContainText("Business Workspace");
});

test("pool members use one responsive card grid", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, accountCount: 4 });
  await page.goto("/");
  await page.getByRole("button", { name: "Pool", exact: true }).click();
  const members = page.locator(".pool-member-list");
  await expect(members.locator('.account-plan-badge[data-plan="plus"]')).toBeVisible();
  await expect(members.locator('.account-plan-badge[data-plan="business"]')).toBeVisible();
  await expect(page.getByRole("button", { name: "Compact pool view" })).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Pool card grid" })).toHaveCount(0);
  await expect(members.locator(".pool-member-rank")).toHaveCount(0);
  await page.setViewportSize({ width: 2048, height: 1152 });
  expect(await members.evaluate((list) => getComputedStyle(list).gridTemplateColumns.split(" ").filter((track) => Number.parseFloat(track) > 1).length)).toBe(await members.locator(".pool-member-card").count());
  expect(await page.evaluate(() => localStorage.getItem("relay.poolLayout"))).toBeNull();
});

test("local pool refreshes all account quotas without an interval setting", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, accountCount: 3, freeAccountHealthy: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Pool", exact: true }).click();

  const freeMember = page.locator('[data-member-label="Backup account"]');
  await expect(freeMember.locator('.relay-status-icon[aria-label^="In rotation"]')).toBeVisible();

  await page.getByRole("button", { name: "Refresh quotas", exact: true }).click();
  await expect(page.getByText("Updated: 3 · Errors: 0", { exact: true })).toBeVisible();
  await expect(page.getByRole("button", { name: "Quota refresh settings", exact: true })).toHaveCount(0);
  await expect(freeMember.locator(".relay-status-icon")).toHaveAttribute("aria-label", "In rotation");

  const calls = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: Record<string, unknown> }> }).__TAURI_TEST_INVOKES__);
  expect(calls.some((call) => call.command === "refresh_all_local_account_quotas")).toBe(true);
});

test("local pool saves adaptive distribution without chat pinning", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Pool", exact: true }).click();
  const personalPlus = page.locator('[data-member-label="Personal Plus"]');
  await expect(personalPlus.locator(".pool-member-subscription-date")).toHaveText(/\d{1,2}\/\d{1,2}\/\d{4}/);
  await expect(personalPlus.locator(".pool-member-subscription-expiry")).toHaveText(/^\d+ d \d+ h \d+ min$/);
  await expect(personalPlus.locator(".quota-meter-heading small").first()).toHaveText(/^\d+ h \d+ min$/);
  await expect(personalPlus.locator(".quota-meter-heading small").nth(1)).toHaveText(/^\d+ d \d+ h \d+ min$/);

  const speed = page.getByRole("switch", { name: "Request mode" });
  await expect(speed).not.toBeChecked();
  await speed.check();
  await expect(speed).toBeChecked();
  await speed.uncheck();
  await expect(speed).not.toBeChecked();
  await speed.check();
  await expect(speed).toBeChecked();

  await page.getByRole("button", { name: "Distribution settings", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "Distribution" });
  await expect(dialog).not.toContainText("Request mode");
  await expect(dialog).not.toContainText("Keep one chat on one account");
  await expect(dialog).not.toContainText("Accounts tried after an error");
  await expect(dialog).toContainText("Uses the greatest available headroom and distributes requests fairly when values are equal.");
  await expect(dialog.getByRole("button", { name: /^Distribution strategy:/ })).toHaveAttribute("data-value", "adaptive");
  await chooseOption(page, dialog, "Distribution strategy", "quota_highest");
  await expect(dialog).toContainText("Uses the greatest remaining quota. Equal values go to the less busy account.");
  await chooseOption(page, dialog, "Distribution strategy", "subscription_expiry");
  await expect(dialog).not.toContainText("Minimum image model");
  await expect(dialog).toContainText("Uses the nearest known expiry first, then accounts without a date. Quota and load break ties.");
  const retryCandidates = dialog.getByLabel("Retry candidates");
  const cooldownAfterFailures = dialog.getByLabel("Failures before cooldown");
  await retryCandidates.fill("20");
  await cooldownAfterFailures.fill("20");
  await expect(retryCandidates).toHaveValue("8");
  await expect(cooldownAfterFailures).toHaveValue("8");
  await retryCandidates.fill("0");
  await cooldownAfterFailures.fill("0");
  await expect(retryCandidates).toHaveValue("1");
  await expect(cooldownAfterFailures).toHaveValue("1");
  await dialog.getByRole("button", { name: "Save", exact: true }).click();
  await expect(personalPlus.locator(".pool-member-subscription-date")).toHaveText(/\d{1,2}\/\d{1,2}\/\d{4}/);
  await expect(personalPlus.locator(".pool-member-subscription-expiry")).toHaveText(/^\d+ d \d+ h \d+ min$/);

  const calls = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: Record<string, unknown> }> }).__TAURI_TEST_INVOKES__);
  expect(calls.findLast((call) => call.command === "update_local_routing")?.args).toEqual({ input: { routingStrategy: "subscription_expiry", maxRetryCandidates: 1, cooldownAfterFailures: 1, keepLastCandidateAvailable: true, defaultServiceTier: "fast", subscriptionPlanOrder: [] } });
});

test("pool card grid preserves scheduler order at every width", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "ru", populated: true, accountCount: 8, quotaAvailable: true, usageActive: false });
  await page.goto("/");
  await page.getByRole("button", { name: "Пул", exact: true }).click();

  const members = page.locator(".pool-member-list");
  const labels = () => members.locator(".pool-member-card").evaluateAll((cards) => cards.map((card) => card.getAttribute("data-member-label")));
  await expect(members.locator(".pool-member-rank")).toHaveCount(0);
  await expect(page.getByRole("radio", { name: "Компактный вид пула" })).toHaveCount(0);
  await expect(members.locator(".pool-member-card-quota").first()).toBeVisible();
  const expectedOrder = await labels();

  await page.setViewportSize({ width: 2048, height: 1152 });
  expect(await labels()).toEqual(expectedOrder);
  expect(await members.evaluate((list) => getComputedStyle(list).gridTemplateColumns.split(" ").filter((track) => Number.parseFloat(track) > 1).length)).toBeGreaterThan(5);
  expect(await members.evaluate((list) => list.scrollWidth <= list.clientWidth)).toBe(true);

  await page.setViewportSize({ width: 840, height: 900 });
  expect(await labels()).toEqual(expectedOrder);
  expect(await members.evaluate((list) => getComputedStyle(list).gridTemplateColumns.split(" ").filter((track) => Number.parseFloat(track) > 1).length)).toBe(2);
  expect(await members.evaluate((list) => list.scrollWidth <= list.clientWidth)).toBe(true);

  await page.reload();
  await page.getByRole("button", { name: "Пул", exact: true }).click();
  await expect(members.locator(".pool-member-card").first()).toBeVisible();
  expect(await labels()).toEqual(expectedOrder);
  expect(await page.locator(".pool-member-list").evaluate((list) => list.scrollWidth <= list.clientWidth)).toBe(true);
  expect(await page.evaluate(() => localStorage.getItem("relay.poolLayout"))).toBeNull();
});

test("subscription group routing saves, reorders, and restores the default order", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, accountCount: 4 });
  await page.goto("/");
  await page.getByRole("button", { name: "Pool", exact: true }).click();
  await page.getByRole("button", { name: "Distribution settings", exact: true }).click();
  let dialog = page.getByRole("dialog", { name: "Distribution" });
  await chooseOption(page, dialog, "Distribution strategy", "subscription_plan");

  const plans = dialog.locator("[data-subscription-plan]");
  await expect(plans).toHaveCount(4);
  expect(await plans.evaluateAll((items) => items.map((item) => item.getAttribute("data-subscription-plan")))).toEqual(["team", "pro", "plus", "free"]);
  await dialog.locator('[data-subscription-plan="free"]').dragTo(dialog.locator('[data-subscription-plan="team"]'));
  expect(await plans.evaluateAll((items) => items.map((item) => item.getAttribute("data-subscription-plan")))).toEqual(["free", "team", "pro", "plus"]);
  await dialog.getByRole("button", { name: "Save", exact: true }).click();

  let calls = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: Record<string, unknown> }> }).__TAURI_TEST_INVOKES__);
  expect(calls.findLast((call) => call.command === "update_local_routing")?.args).toEqual({ input: { routingStrategy: "subscription_plan", maxRetryCandidates: 3, cooldownAfterFailures: 3, keepLastCandidateAvailable: true, defaultServiceTier: "standard", subscriptionPlanOrder: ["free", "team", "pro", "plus"] } });

  await page.getByRole("button", { name: "Distribution settings", exact: true }).click();
  dialog = page.getByRole("dialog", { name: "Distribution" });
  await dialog.getByRole("button", { name: "Restore default order" }).click();
  await expect(dialog.getByRole("button", { name: /^Distribution strategy:/ })).toHaveAttribute("data-value", "subscription_plan");
  await dialog.getByRole("button", { name: "Save", exact: true }).click();
  calls = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: Record<string, unknown> }> }).__TAURI_TEST_INVOKES__);
  expect(calls.findLast((call) => call.command === "update_local_routing")?.args).toEqual({ input: { routingStrategy: "subscription_plan", maxRetryCandidates: 3, cooldownAfterFailures: 3, keepLastCandidateAvailable: true, defaultServiceTier: "standard", subscriptionPlanOrder: ["team", "pro", "plus", "free"] } });
});

test("remote pool saves distribution settings on the connected runtime", async ({ page }) => {
  await installTauriMock(page, { mode: "remote", locale: "en", populated: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Pool", exact: true }).click();
  const speed = page.getByRole("switch", { name: "Request mode" });
  await speed.check();
  await expect(speed).toBeChecked();
  await page.getByRole("button", { name: "Distribution settings", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "Distribution" });
  await expect(dialog).not.toContainText("Keep one chat on one account");
  await expect(dialog).not.toContainText("Request mode");
  await chooseOption(page, dialog, "Distribution strategy", "subscription_expiry");
  await dialog.getByRole("button", { name: "Save", exact: true }).click();

  const calls = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: Record<string, unknown> }> }).__TAURI_TEST_INVOKES__);
  expect(calls.findLast((call) => call.command === "execute_remote_server_action")?.args.input).toEqual({
    action: { type: "set_routing_policy" },
    payload: { maxRetryCandidates: 3, cooldownAfterFailures: 3, keepLastCandidateAvailable: true, routingStrategy: "subscription_expiry", defaultServiceTier: "fast", subscriptionPlanOrder: [] },
  });
  expect(calls.some((call) => call.command === "sync_codex_default_service_tier")).toBe(false);
});

test("remote configuration presets require preview before an explicit apply", async ({ page }) => {
  await installTauriMock(page, { mode: "remote", locale: "en", populated: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Pool", exact: true }).click();

  await page.getByRole("button", { name: "Save preset", exact: true }).click();
  await page.getByRole("button", { name: "Apply preset", exact: true }).click();

  const dialog = page.getByRole("dialog", { name: "Configuration preset" });
  await expect(dialog.getByText("Changes: 1", { exact: true })).toBeVisible();
  await expect(dialog.getByText("routing / maxRetryCandidates", { exact: true })).toBeVisible();
  await expect(dialog.getByRole("cell", { name: "3", exact: true })).toBeVisible();
  await expect(dialog.getByRole("cell", { name: "4", exact: true })).toBeVisible();
  await dialog.getByRole("button", { name: "Apply changes" }).click();
  await expect(dialog).toBeHidden();

  const calls = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: Record<string, unknown> }> }).__TAURI_TEST_INVOKES__);
  expect(calls.some((call) => call.command === "export_remote_configuration_preset")).toBe(true);
  expect(calls.some((call) => call.command === "preview_remote_configuration_preset")).toBe(true);
  expect(calls.findLast((call) => call.command === "apply_remote_configuration_preset")?.args).toMatchObject({ input: { baseRevision: "cfg_synthetic_current" } });
});

test("local pool can save a portable configuration without exposing server apply", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Pool", exact: true }).click();

  await expect(page.getByRole("button", { name: "Apply preset", exact: true })).toHaveCount(0);
  await page.getByRole("button", { name: "Save preset", exact: true }).click();

  await expect.poll(() => page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string }> }).__TAURI_TEST_INVOKES__.some((call) => call.command === "export_local_configuration_preset"))).toBe(true);
});

test("remote pool refreshes quotas without exposing an interval setting", async ({ page }) => {
  await installTauriMock(page, { mode: "remote", locale: "en", populated: true, accountCount: 3, freeAccountHealthy: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Pool", exact: true }).click();

  await page.getByRole("button", { name: "Refresh quotas", exact: true }).click();
  await expect(page.getByRole("button", { name: "Quota refresh settings", exact: true })).toHaveCount(0);

  const actions = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: { input?: { action?: { type?: string }; payload?: unknown } } }> }).__TAURI_TEST_INVOKES__.filter((call) => call.command === "execute_remote_server_action").map((call) => call.args.input));
  expect(actions).toContainEqual({ action: { type: "refresh_all_quotas" }, payload: null });
  expect(actions.some((input) => input?.action?.type === "set_quota_policy")).toBe(false);
});

test("connections route Free accounts like other pool members", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, accountCount: 3, freeAccountHealthy: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  const freeAccount = page.locator(".account-card").filter({ hasText: "Backup account" });
  await expect(freeAccount.locator(".relay-status-icon")).toHaveAttribute("aria-label", "In rotation");
  await expect(freeAccount).toContainText("95%");
  await expect(freeAccount).toContainText("5 weeks");
});

test("page navigation resets the shared content scroll position", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, accountCount: 6 });
  await page.setViewportSize({ width: 840, height: 560 });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await expect(page.getByRole("heading", { name: "Connections", exact: true })).toBeVisible();
  await page.locator(".relay-content").evaluate((element) => { element.scrollTop = element.scrollHeight; });
  await expect.poll(() => page.locator(".relay-content").evaluate((element) => element.scrollTop)).toBeGreaterThan(0);
  await page.getByRole("button", { name: "Usage", exact: true }).click();
  await expect(page.locator(".relay-content")).toHaveJSProperty("scrollTop", 0);
  await expect(page.getByRole("heading", { name: "Usage", exact: true })).toBeInViewport();
});

test("invalid OAuth grants keep the account and explain the required action", async ({ page }) => {
  await installTauriMock(page, {
    mode: "local",
    locale: "en",
    populated: true,
    accountAuthReason: "invalid_grant",
    quotaAvailable: true,
  });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();

  await expect(page.locator(".account-card")).toHaveCount(1);
  await expect(page.locator(".account-card")).toContainText("Personal Plus");
  await expect(page.locator(".account-status-button")).toHaveAttribute("aria-label", "Signed out or account changed");
  await expect(page.locator(".account-card")).not.toContainText("auth_invalid_grant");

  await page.getByRole("button", { name: "Pool", exact: true }).click();
  const member = page.locator('[data-member-label="Personal Plus"]');
  await expect(member.locator('.pool-member-kind-icon[data-status="error"]')).toHaveAttribute("aria-label", "Signed out or account changed");
  await expect(member.locator('.relay-status-icon[aria-label="In rotation"]')).toHaveCount(0);
});

test("source and automation rows keep rare actions in consistent menus", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, codexBindings: false });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();

  await page.getByRole("tab", { name: "Sources" }).click();
  let actions = page.locator(".relay-table .row-actions");
  expect(await actions.locator(":scope > *").evaluateAll((items) => items.map((item) => item.tagName === "DETAILS" ? item.querySelector("summary")?.getAttribute("aria-label") : item.getAttribute("aria-label")))).toEqual(["Actions", "Edit", "Launch in ChatGPT"]);
  await actions.locator("summary").click();
  expect(await page.getByRole("menuitem").allTextContents()).toEqual(["Refresh models", "Remove from pool", "Disable", "Delete"]);
  await page.keyboard.press("Escape");

  await page.getByRole("tab", { name: "Automations" }).click();
  actions = page.locator(".relay-table .row-actions");
  expect(await actions.locator(":scope > *").evaluateAll((items) => items.map((item) => item.tagName === "DETAILS" ? item.querySelector("summary")?.getAttribute("aria-label") : item.getAttribute("aria-label")))).toEqual(["Edit", "Test", "Actions"]);
  await actions.locator("summary").click();
  await expect(page.getByRole("menuitem")).toHaveText("Delete");
});

test("empty connection views keep the page header as the single action area", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: false });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await page.getByRole("tab", { name: "Sources" }).click();
  await expect(page.getByRole("button", { name: "Add source", exact: true })).toHaveCount(1);
  await expect(page.getByPlaceholder("Search")).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Refresh", exact: true })).toHaveCount(0);

  await page.getByRole("tab", { name: "Automations" }).click();
  await expect(page.getByRole("button", { name: "Add automation", exact: true })).toHaveCount(1);
  await expect(page.getByPlaceholder("Search")).toHaveCount(0);
});

test("ChatGPT client setup combines account selection, fixed reserve, and forced switching", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, accountCount: 2, gatewayRunning: true, historyRepairChanges: false });
  await page.goto("/");
  await page.getByRole("button", { name: "API & ChatGPT", exact: true }).click();
  const setup = page.locator(".client-setup");
  const accountMenu = setup.getByRole("button", { name: /^Account:/ });
  await expect(accountMenu).toHaveAttribute("data-value", "auto");
  expect(await page.evaluate(() => localStorage.getItem("relay.codexPoolOauthSelection"))).toBe("auto");
  const reserve = setup.getByRole("checkbox", { name: "Keep 1% reserved" });
  await expect(reserve).toBeChecked();
  await reserve.uncheck();
  await expect.poll(() => page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: Record<string, unknown> }> }).__TAURI_TEST_INVOKES__.findLast((item) => item.command === "update_chatgpt_interface_quota_reserve")?.args)).toEqual({ input: { reserveBasisPoints: 0 } });
  await reserve.check();
  await expect.poll(() => page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: Record<string, unknown> }> }).__TAURI_TEST_INVOKES__.findLast((item) => item.command === "update_chatgpt_interface_quota_reserve")?.args)).toEqual({ input: { reserveBasisPoints: 100 } });
  await expect(setup).not.toContainText("Generated configuration");
  const selectionMatchesTheme = await setup.evaluate((element) => {
    const probe = document.createElement("span");
    probe.style.backgroundColor = "var(--relay-accent-soft)";
    element.appendChild(probe);
    const expected = getComputedStyle(probe).backgroundColor;
    probe.remove();
    return getComputedStyle(element, "::selection").backgroundColor === expected;
  });
  expect(selectionMatchesTheme).toBe(true);

  await chooseOption(page, setup, "Account", "account_synthetic");
  await expect(accountMenu).toHaveAttribute("data-value", "account_synthetic");

  await chooseOption(page, setup, "Account", "none");
  await expect(setup.getByRole("checkbox", { name: "Keep 1% reserved" })).toHaveCount(0);
  expect(await page.evaluate(() => localStorage.getItem("relay.codexPoolOauthSelection"))).toBe("none");
  await setup.getByRole("button", { name: "Switch", exact: true }).click();
  await expect.poll(() => page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string }> }).__TAURI_TEST_INVOKES__.filter((item) => item.command === "attach_codex_to_local_gateway").length)).toBe(1);
  let call = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: Record<string, unknown> }> }).__TAURI_TEST_INVOKES__.findLast((item) => item.command === "attach_codex_to_local_gateway"));
  expect(call?.args).toEqual({ boundOauthAccountId: null, disableOauthBinding: true });

  await chooseOption(page, page, "Account", "account_synthetic_2");
  expect(await page.evaluate(() => localStorage.getItem("relay.codexPoolOauthSelection"))).toBe("account_synthetic_2");
  await setup.getByRole("button", { name: "Switch", exact: true }).click();
  await expect.poll(() => page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string }> }).__TAURI_TEST_INVOKES__.filter((item) => item.command === "attach_codex_to_local_gateway").length)).toBe(2);
  call = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: Record<string, unknown> }> }).__TAURI_TEST_INVOKES__.findLast((item) => item.command === "attach_codex_to_local_gateway"));
  expect(call?.args).toEqual({ boundOauthAccountId: "account_synthetic_2" });
});

test("ChatGPT pool identity migrates the previous stored account selection", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, accountCount: 2 });
  await page.addInitScript(() => localStorage.setItem("relay.codexPoolOauthAccountId", "account_synthetic_2"));
  await page.goto("/");
  await page.getByRole("button", { name: "API & ChatGPT", exact: true }).click();
  await expect(page.getByRole("button", { name: /^Account:/ })).toHaveAttribute("data-value", "account_synthetic_2");
  expect(await page.evaluate(() => ({ current: localStorage.getItem("relay.codexPoolOauthSelection"), legacy: localStorage.getItem("relay.codexPoolOauthAccountId") }))).toEqual({ current: "account_synthetic_2", legacy: null });
});

test("ChatGPT account picker includes quota-wait and Free pool accounts", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, accountCount: 3, quotaAvailable: false });
  await page.goto("/");
  await page.getByRole("button", { name: "API & ChatGPT", exact: true }).click();
  const setup = page.locator(".client-setup");
  await setup.getByRole("button", { name: /^Account:/ }).click();
  await expect(page.locator('[role="option"][data-value="account_synthetic"]')).toContainText("Personal Plus");
  await expect(page.locator('[role="option"][data-value="account_synthetic_2"]')).toContainText("Business Workspace");
  await expect(page.locator('[role="option"][data-value="account_synthetic_3"]')).toContainText("Backup account");
  await page.locator('[role="option"][data-value="account_synthetic"]').click();
  await setup.getByRole("button", { name: "Switch", exact: true }).click();

  const call = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: Record<string, unknown> }> }).__TAURI_TEST_INVOKES__.findLast((item) => item.command === "attach_codex_to_local_gateway"));
  expect(call?.args).toEqual({ boundOauthAccountId: "account_synthetic" });
});

test("usage filters are named and stay scoped to the request report", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Usage", exact: true }).click();
  await page.getByRole("button", { name: /^Status:/ }).click();
  await expect(page.getByRole("option").first()).toHaveText("Any status");
  await page.locator('[role="option"][data-value="all"]').click();
  await page.getByRole("button", { name: "More filters" }).click();
  await page.getByRole("button", { name: /^Protocol:/ }).click();
  await expect(page.getByRole("option").first()).toHaveText("Any protocol");
  await page.locator('[role="option"][data-value="responses"]').click();
  await chooseOption(page, page, "Status", "failed");
  await expect.poll(() => page.evaluate(() => {
    const calls = (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: { input?: { success?: boolean; wireApi?: string } } }> }).__TAURI_TEST_INVOKES__;
    return calls.findLast((call) => call.command === "get_local_usage_page")?.args.input;
  })).toMatchObject({ success: false, wireApi: "responses" });

  await page.getByRole("tab", { name: "Models" }).click();
  await expect.poll(() => page.evaluate(() => {
    const calls = (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: { input?: { success?: boolean; wireApi?: string } } }> }).__TAURI_TEST_INVOKES__;
    return calls.findLast((call) => call.command === "get_local_usage_page")?.args.input ?? {};
  })).not.toMatchObject({ success: false, wireApi: "responses" });
});

test("account usage shows measured quota economics and the model cost table", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, quotaAvailable: true, accountCount: 4, planBenchmark: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Usage", exact: true }).click();
  await chooseOption(page, page, "Account", "account_synthetic");

  const economics = page.locator(".usage-account-economics");
  await expect(economics).toContainText("Personal Plus");
  await expect(economics).toContainText("Estimated potential≈$24");
  await expect(economics).toContainText("Calibration3.4%");
  await expect(economics).toContainText("Plan benchmark");
  await expect(economics).toContainText("Plus · Follow ChatGPT · 3 accounts");
  await expect(economics.locator(".usage-window-table thead th")).toHaveText([
    "Window", "Remaining", "Estimated potential", "Plan benchmark", "Tokens", "Similar requests", "Mode", "Reset",
  ]);
  await expect(economics.locator("tbody tr").nth(1).locator("td").nth(2)).toContainText("≈$28.8");
  await expect(economics.getByRole("row")).toHaveCount(3);
  await expect.poll(() => page.evaluate(() => {
    const calls = (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: { input?: { sourceOrAccountQuery?: string } } }> }).__TAURI_TEST_INVOKES__;
    return calls.findLast((call) => call.command === "get_local_usage_page")?.args.input?.sourceOrAccountQuery;
  })).toMatch(/^(account_synthetic|a1b2c3d4e5f6)$/);

  await page.getByRole("tab", { name: "Models" }).click();
  await expect(page.locator(".usage-aggregate-table thead th")).toHaveText([
    "Model", "Requests", "Input tokens", "Output tokens", "Cache reads", "Value",
  ]);
  await page.setViewportSize({ width: 840, height: 560 });
  await expect(economics).toBeVisible();
});

test("usage request columns reorder, resize, and open details only from the request id", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Usage", exact: true }).click();

  const table = page.locator(".usage-request-table");
  const row = table.getByRole("row").filter({ hasText: "req_synthetic_local" });
  const requestCell = row.locator('td[data-column="request"]');
  const requestLink = requestCell.getByRole("button", { name: "Request details: req_synthetic_local" });
  const [cellBounds, linkWidth] = await Promise.all([requestCell.boundingBox(), requestLink.evaluate((element) => element.getBoundingClientRect().width)]);
  expect(cellBounds).not.toBeNull();
  const cellWidth = cellBounds!.width;
  expect(linkWidth).toBeLessThan(cellWidth);
  await requestCell.click({ position: { x: cellWidth - 2, y: cellBounds!.height / 2 } });
  await expect(page.getByRole("dialog", { name: "Request details" })).toHaveCount(0);
  await requestLink.click();
  await expect(page.getByRole("dialog", { name: "Request details" })).toBeVisible();
  await page.getByRole("dialog", { name: "Request details" }).getByRole("button", { name: "Close" }).first().click();

  expect(await table.locator("th, td").evaluateAll((cells) => cells.every((cell) => getComputedStyle(cell).textAlign === "center"))).toBe(true);
  const statusHeading = table.getByLabel(/^Move the Status column/);
  const timeHeading = table.getByLabel(/^Move the Time column/);
  const [statusBounds, timeBounds] = await Promise.all([statusHeading.boundingBox(), timeHeading.boundingBox()]);
  expect(statusBounds).not.toBeNull();
  expect(timeBounds).not.toBeNull();
  await page.mouse.move(statusBounds!.x + statusBounds!.width / 2, statusBounds!.y + statusBounds!.height / 2);
  await page.mouse.down();
  await page.mouse.move(timeBounds!.x + 3, timeBounds!.y + timeBounds!.height / 2, { steps: 5 });
  await page.mouse.up();
  await expect.poll(() => table.locator("thead th").evaluateAll((headers) => headers.map((header) => header.getAttribute("data-column")))).toEqual(["status", "time", "model", "tier", "connection", "timing", "speed", "tokens", "equivalent", "request"]);

  const modelResize = table.getByRole("separator", { name: /^Resize the Model column/ });
  const bounds = await modelResize.boundingBox();
  expect(bounds).not.toBeNull();
  await page.mouse.move(bounds!.x + bounds!.width / 2, bounds!.y + bounds!.height / 2);
  await page.mouse.down();
  await page.mouse.move(bounds!.x + bounds!.width / 2 + 36, bounds!.y + bounds!.height / 2, { steps: 4 });
  await page.mouse.up();
  await expect(table).toHaveAttribute("data-resized", "true");
  await expect.poll(() => page.evaluate(() => JSON.parse(localStorage.getItem("relay.usageRequestTableLayout") ?? "null"))).toMatchObject({ order: ["status", "time", "model", "tier", "connection", "timing", "speed", "tokens", "equivalent", "request"], widths: { model: expect.any(Number) } });

  await page.reload();
  await page.getByRole("button", { name: "Usage", exact: true }).click();
  await expect(page.locator(".usage-request-table thead th").first()).toHaveAttribute("data-column", "status");
  await expect(page.locator(".usage-request-table")).toHaveAttribute("data-resized", "true");
  await page.setViewportSize({ width: 840, height: 560 });
  expect(await page.locator(".usage-request-table").evaluate((element) => element.parentElement!.scrollWidth <= element.parentElement!.clientWidth)).toBe(true);
});

test("usage details warn when forwarded tools yield a text-only response", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, usageToolDiagnostics: "forwarded_text_only" });
  await page.goto("/");
  await page.getByRole("button", { name: "Usage", exact: true }).click();
  await page.getByRole("button", { name: "Request details: req_synthetic_local" }).click();

  const dialog = page.getByRole("dialog", { name: "Request details" });
  await expect(dialog.getByText("Tool diagnostics", { exact: true })).toBeVisible();
  await expect(dialog.getByText("Tools received from client", { exact: true })).toBeVisible();
  await expect(dialog.getByText("Tools forwarded upstream", { exact: true })).toBeVisible();
  await expect(dialog.getByText("Automatic", { exact: true })).toBeVisible();
  await expect(dialog.getByText("Text only", { exact: true })).toBeVisible();
  await expect(dialog.getByRole("button", { name: "Copy request ID" })).toBeVisible();
  await expect(dialog.getByText(/Relay forwarded 3 tool definitions/)).toBeVisible();
});

test("usage details do not blame the upstream when tools were not forwarded", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, usageToolDiagnostics: "dropped_text_only" });
  await page.goto("/");
  await page.getByRole("button", { name: "Usage", exact: true }).click();
  await page.getByRole("button", { name: "Request details: req_synthetic_local" }).click();

  await expect(page.getByRole("dialog", { name: "Request details" }).getByText(/Relay forwarded \d+ tool definitions/)).toHaveCount(0);
});

test("local usage omits the obsolete ChatGPT routing banner", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, codexBindingActive: false });
  await page.goto("/");
  await page.getByRole("button", { name: "Usage", exact: true }).click();

  await expect(page.getByText("ChatGPT currently uses another provider. New requests will not appear in this local history.")).toHaveCount(0);
});

test("usage attributes API token totals to the selected account", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await expect(page.locator(".account-card .account-token-speed")).toHaveCount(0);
  await page.getByRole("button", { name: "Pool", exact: true }).click();
  await expect(page.locator('[data-member-label="Personal Plus"] .account-economics-strip')).toContainText("API equiv.");
  await expect(page.locator('[data-member-label="Personal Plus"] .account-economics-strip')).not.toContainText("Latest output speed");
  await page.getByRole("button", { name: "Usage", exact: true }).click();
  await page.getByRole("tab", { name: "Pool members" }).click();

  const account = page.getByRole("row").filter({ hasText: "Personal Plus" });
  await expect(account.getByRole("cell")).toHaveText(["Personal Plus", "1", "100%", "In20Cache ↓12Reason5Out8", "28", "≈$0.0001", "10 tok/s", "128 / 428 ms"]);
  await expect(account.locator(".usage-token-breakdown span")).toHaveText(["In20", "Cache ↓12", "Reason5", "Out8"]);
  await expect(page.locator(".usage-performance")).toContainText("Generation speed10 tok/s");
  await expect(page.locator(".usage-performance")).toContainText("Effective end-to-end speed7 tok/s");

  await page.getByRole("tab", { name: "Requests" }).click();
  await page.getByRole("button", { name: "Request details: req_synthetic_local" }).click();
  const details = page.getByRole("dialog", { name: "Request details" });
  await expect(details).toContainText("Input tokens20");
  await expect(details).toContainText("Cache reads12");
  await expect(details).toContainText("Reasoning tokens5");
  await expect(details).toContainText("Output tokens8");
  await expect(details).toContainText("Total tokens28");
  await expect(details).toContainText("API equivalent≈$0.0001");
  await expect(details).toContainText("First output128 ms");
  await expect(details).toContainText("Generation time300 ms");
  await expect(details).toContainText("Total time428 ms");
  await expect(details).toContainText("Generation speed10 tok/s");
  await expect(details).toContainText("Effective end-to-end speed7 tok/s");
  await expect(details).toContainText("Selection reasonGreatest quota remaining");
  await expect(details).toContainText("Eligible participants4");
  await expect(details).toContainText("Quota at selection63.00%");
});

test("partial API equivalents state their coverage without an asterisk", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, usageUnpricedTokens: 7 });
  await page.goto("/");
  await page.getByRole("button", { name: "Usage", exact: true }).click();

  const metric = page.locator(".usage-metrics > div").filter({ hasText: "API equivalent" });
  await expect(metric).toContainText("21 priced · 7 unpriced");
  await expect(metric).not.toContainText("*");
  await expect(page.locator('.usage-request-table tbody td[data-column="equivalent"]')).not.toContainText("*");
});

test("OAuth member policy hides manual routing controls", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Pool", exact: true }).click();
  await page.getByRole("button", { name: "Pool member policy: Personal Plus", exact: true }).click();

  const dialog = page.getByRole("dialog", { name: /Pool member policy/ });
  await expect(dialog).not.toContainText("Tie-break priority");
  await expect(dialog).not.toContainText("Traffic share");
  await expect(dialog.getByText("Drain", { exact: true })).toBeVisible();
  await expect(dialog.locator(".member-model-rules > summary")).toContainText("Models");
  await expect(dialog.locator("[data-member-model-id]")).toHaveCount(2);
  await expect(dialog.getByRole("button", { name: "Remove from pool" })).toHaveCount(0);
});

for (const mode of ["local", "remote"] as const) {
  test(`${mode} model rules keep catalog order and toggle the same runtime contract`, async ({ page }) => {
    await installTauriMock(page, { mode, locale: "en", populated: true, accountCount: 2 });
    await page.goto("/");
    await page.getByRole("button", { name: "Pool", exact: true }).click();
    await page.getByRole("tab", { name: "Model Rules" }).click();

    const rows = page.locator(".model-rules tbody tr[data-model-id]");
    await expect(rows).toHaveCount(3);
    expect(await rows.evaluateAll((items) => items.map((item) => item.getAttribute("data-model-id")))).toEqual(["gpt-5.4", "gpt-5.4-mini", "o3"]);
    await expect(rows.first().locator(".model-price-value small")).toHaveText(["Input", "Output", "Cache read"]);
    await expect(rows.first().locator(".model-price-value strong")).toHaveText(["$2.5", "$15", "$0.25"]);
    await expect(rows.first().locator(".model-codex-state")).toContainText("Shown in model list");
    await expect(page.locator('.model-rules tbody tr[data-model-id="o3"]')).toContainText("Price not listed");

    await expect(page.locator(".model-sort-select")).toHaveCount(0);

    const mini = page.locator('.model-rules tbody tr[data-model-id="gpt-5.4-mini"]');
    await mini.getByRole("button", { name: "Disable gpt-5.4-mini" }).click();
    await expect(mini).toHaveAttribute("data-enabled", "false");
    await expect(mini.locator('.relay-status-icon[aria-label="Disabled"]')).toBeVisible();
    await mini.getByRole("button", { name: "Enable gpt-5.4-mini" }).click();
    await expect(mini).toHaveAttribute("data-enabled", "true");

    const calls = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: Record<string, unknown> }> }).__TAURI_TEST_INVOKES__);
    if (mode === "local") {
      expect(calls.filter((call) => call.command === "set_local_model_enabled").map((call) => call.args)).toEqual([
        { input: { modelId: "gpt-5.4-mini", enabled: false } },
        { input: { modelId: "gpt-5.4-mini", enabled: true } },
      ]);
    } else {
      expect(calls.filter((call) => call.command === "execute_remote_server_action" && (call.args.input as { action?: { type?: string } } | undefined)?.action?.type === "set_model_enabled").map((call) => call.args)).toEqual([
        { input: { action: { type: "set_model_enabled" }, payload: { modelId: "gpt-5.4-mini", enabled: false } } },
        { input: { action: { type: "set_model_enabled" }, payload: { modelId: "gpt-5.4-mini", enabled: true } } },
      ]);
    }
  });
}

test("remote model rules preserve the server group and model order", async ({ page }) => {
  await installTauriMock(page, {
    mode: "remote",
    locale: "en",
    populated: true,
    serverModelOrder: [
      "gemini-3.6-flash-high",
      "gemini-3.6-flash-medium",
      "gemini-3.6-flash-low",
    ],
  });
  await page.goto("/");
  await page.getByRole("button", { name: "Pool", exact: true }).click();
  await page.getByRole("tab", { name: "Model Rules" }).click();

  const rows = page.locator(".model-rules tbody tr[data-model-id]");
  expect(await rows.evaluateAll((items) => items.map((item) => item.getAttribute("data-model-id")))).toEqual([
    "gpt-5.4",
    "gpt-5.4-mini",
    "gemini-3.6-flash-high",
    "gemini-3.6-flash-medium",
    "gemini-3.6-flash-low",
  ]);
});

test("local model prices can override and restore API-equivalent valuation", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, accountCount: 2 });
  await page.goto("/");
  await page.getByRole("button", { name: "Pool", exact: true }).click();
  await page.getByRole("tab", { name: "Model Rules" }).click();

  const model = page.locator('.model-rules tbody tr[data-model-id="o3"]');
  await model.getByRole("button", { name: "Edit price for o3" }).click();
  const dialog = page.getByRole("dialog", { name: "Model price" });
  await expect(dialog.locator(".model-price-label")).toHaveText(["Input", "Output", "Cache read"]);
  await dialog.getByLabel("Input", { exact: true }).fill("1.25");
  await dialog.getByLabel("Output", { exact: true }).fill("7.5");
  await dialog.getByLabel("Cache read").fill("0.125");
  await dialog.getByRole("button", { name: "Save" }).click();
  await expect(model.locator(".model-price-value small")).toHaveText(["Input", "Output", "Cache read"]);
  await expect(model.locator(".model-price-value strong")).toHaveText(["$1.25", "$7.5", "$0.125"]);
  await expect(model).toContainText("Custom price per 1M tokens");

  await model.getByRole("button", { name: "Edit price for o3" }).click();
  await page.getByRole("dialog", { name: "Model price" }).getByRole("button", { name: "Restore catalog price" }).click();
  await expect(model).toContainText("Price not listed");

  const calls = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: Record<string, unknown> }> }).__TAURI_TEST_INVOKES__);
  expect(calls.filter((call) => call.command === "set_local_model_price").map((call) => call.args)).toEqual([
    { input: { modelId: "o3", inputMicroUsdPerMillion: 1_250_000, cachedInputMicroUsdPerMillion: 125_000, cacheWrite5mMicroUsdPerMillion: null, cacheWrite1hMicroUsdPerMillion: null, outputMicroUsdPerMillion: 7_500_000 } },
    { input: { modelId: "o3", inputMicroUsdPerMillion: null, cachedInputMicroUsdPerMillion: null, cacheWrite5mMicroUsdPerMillion: null, cacheWrite1hMicroUsdPerMillion: null, outputMicroUsdPerMillion: null } },
  ]);
});

test("remote model prices use the server-owned override", async ({ page }) => {
  await installTauriMock(page, { mode: "remote", locale: "en", populated: true, accountCount: 2 });
  await page.goto("/");
  await page.getByRole("button", { name: "Pool", exact: true }).click();
  await page.getByRole("tab", { name: "Model Rules" }).click();

  const model = page.locator('.model-rules tbody tr[data-model-id="o3"]');
  await model.getByRole("button", { name: "Edit price for o3" }).click();
  const dialog = page.getByRole("dialog", { name: "Model price" });
  await dialog.getByLabel("Input", { exact: true }).fill("1.25");
  await dialog.getByLabel("Output", { exact: true }).fill("7.5");
  await dialog.getByLabel("Cache read").fill("0.125");
  await dialog.getByRole("button", { name: "Save" }).click();
  await expect(model.locator(".model-price-value small")).toHaveText(["Input", "Output", "Cache read"]);
  await expect(model.locator(".model-price-value strong")).toHaveText(["$1.25", "$7.5", "$0.125"]);

  await model.getByRole("button", { name: "Edit price for o3" }).click();
  await page.getByRole("dialog", { name: "Model price" }).getByRole("button", { name: "Restore catalog price" }).click();
  await expect(model).toContainText("Price not listed");

  const calls = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: Record<string, unknown> }> }).__TAURI_TEST_INVOKES__);
  expect(calls.filter((call) => call.command === "execute_remote_server_action" && (call.args.input as { action?: { type?: string } } | undefined)?.action?.type === "set_model_price").map((call) => call.args)).toEqual([
    { input: { action: { type: "set_model_price" }, payload: { modelId: "o3", inputMicroUsdPerMillion: 1_250_000, cachedInputMicroUsdPerMillion: 125_000, cacheWrite5mMicroUsdPerMillion: null, cacheWrite1hMicroUsdPerMillion: null, outputMicroUsdPerMillion: 7_500_000 } } },
    { input: { action: { type: "set_model_price" }, payload: { modelId: "o3", inputMicroUsdPerMillion: null, cachedInputMicroUsdPerMillion: null, cacheWrite5mMicroUsdPerMillion: null, cacheWrite1hMicroUsdPerMillion: null, outputMicroUsdPerMillion: null } } },
  ]);
});

test("local model reasoning uses only confirmed source levels", async ({ page }) => {
  await installTauriMock(page, {
    mode: "local",
    locale: "en",
    populated: true,
    mixedModels: true,
    modelReasoning: { "claude-opus-4-8": ["low", "medium", "high", "ultra"] },
  });
  await page.goto("/");
  await page.getByRole("button", { name: "Pool", exact: true }).click();
  await page.getByRole("tab", { name: "Model Rules" }).click();

  const claude = page.locator('.model-rules tbody tr[data-model-id="claude-opus-4-8"]');
  await claude.getByRole("button", { name: "Set reasoning modes for claude-opus-4-8" }).click();
  const dialog = page.getByRole("dialog", { name: "Reasoning modes" });
  await expect(dialog.getByRole("checkbox")).toHaveText(["Low", "Medium", "High", "Ultra"]);
  await dialog.getByRole("checkbox", { name: "High" }).click();
  await dialog.getByRole("button", { name: "Save" }).click();

  await expect(page.locator('[data-model-reasoning-edit="gpt-5.4"]')).toHaveCount(0);
  const calls = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: Record<string, unknown> }> }).__TAURI_TEST_INVOKES__);
  expect(calls.filter((call) => call.command === "set_local_model_reasoning").map((call) => call.args)).toEqual([
    { input: { modelId: "claude-opus-4-8", allowedLevels: ["high"] } },
  ]);
});

test("remote model reasoning is saved through the server action", async ({ page }) => {
  await installTauriMock(page, {
    mode: "remote",
    locale: "en",
    populated: true,
    mixedModels: true,
    modelReasoning: { "claude-opus-4-8": ["low", "high", "ultra"] },
  });
  await page.goto("/");
  await page.getByRole("button", { name: "Pool", exact: true }).click();
  await page.getByRole("tab", { name: "Model Rules" }).click();

  const claude = page.locator('.model-rules tbody tr[data-model-id="claude-opus-4-8"]');
  await claude.getByRole("button", { name: "Set reasoning modes for claude-opus-4-8" }).click();
  const dialog = page.getByRole("dialog", { name: "Reasoning modes" });
  await dialog.getByRole("checkbox", { name: "Ultra" }).click();
  await dialog.getByRole("button", { name: "Save" }).click();

  const calls = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: Record<string, unknown> }> }).__TAURI_TEST_INVOKES__);
  expect(calls.filter((call) => call.command === "execute_remote_server_action" && (call.args.input as { action?: { type?: string } } | undefined)?.action?.type === "set_model_reasoning").map((call) => call.args)).toEqual([
    { input: { action: { type: "set_model_reasoning" }, payload: { modelId: "claude-opus-4-8", allowedLevels: ["ultra"] } } },
  ]);
});

test("Help opens the current mode guide and keeps quick setup explicit", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Help" }).click();
  await expect(page.getByRole("heading", { name: "Help" })).toBeVisible();
  await expect(page.getByRole("tab")).toHaveText(["This computer", "Choose API", "My server"]);
  await expect(page.getByRole("tab", { name: "This computer" })).toHaveAttribute("aria-selected", "true");
  await expect(page.getByRole("heading", { name: "This computer", level: 1 })).toBeVisible();
  const troubleshooting = page.getByRole("link", { name: "If requests fail" });
  await expect(troubleshooting).toHaveAttribute("href", "#if-requests-fail");
  await troubleshooting.click();
  await expect(page.getByRole("heading", { name: "If requests fail", level: 2 })).toBeVisible();
  await page.getByRole("tab", { name: "My server" }).click();
  await expect(page.getByRole("heading", { name: "My server", level: 1 })).toBeVisible();
  await expect(page.getByText("Never use the management token as a profile credential.", { exact: false })).toBeVisible();
  await page.getByRole("button", { name: "Repeat quick setup" }).click();
  await expect(page.getByRole("button", { name: "Get started" })).toBeVisible();
});

test("updates are checked without downloading and require an explicit action", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, updateVersion: "1.1.0", updateBody: "Faster parallel routing\nUpdated settings" });
  await page.goto("/");

  const updateButton = page.getByRole("button", { name: "Open update 1.1.0" });
  await expect(updateButton).toBeVisible();
  let commands = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string }> }).__TAURI_TEST_INVOKES__.map((call) => call.command));
  expect(commands).toContain("plugin:updater|check");
  expect(commands).not.toContain("plugin:updater|download_and_install");

  await updateButton.click();
  let dialog = page.getByRole("dialog", { name: "Update 1.1.0" });
  await expect(dialog).toContainText("Faster parallel routing");
  await dialog.getByRole("button", { name: "Skip 1.1.0" }).click();
  await expect(updateButton).toHaveCount(0);
  await expect.poll(() => page.evaluate(() => localStorage.getItem("relay.skippedUpdate"))).toBe("1.1.0");

  await page.getByRole("button", { name: "Settings", exact: true }).click();
  await page.locator(".settings-group").filter({ hasText: "Application" }).getByRole("button", { name: "Check" }).click();
  dialog = page.getByRole("dialog", { name: "Update 1.1.0" });
  await expect(dialog).toBeVisible();
  await dialog.getByRole("button", { name: "Update", exact: true }).click();
  await expect.poll(() => page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string }> }).__TAURI_TEST_INVOKES__.map((call) => call.command))).toEqual(expect.arrayContaining(["plugin:updater|download_and_install", "plugin:process|restart"]));
  commands = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string }> }).__TAURI_TEST_INVOKES__.map((call) => call.command));
  expect(commands.filter((command) => command === "plugin:updater|download_and_install")).toHaveLength(1);
});

test("portable updates replace the same executable through the verified helper path", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, bundleType: null, updateVersion: "1.1.1", updateBody: "<!-- relay-notes:en -->\nPortable self-update\n<!-- relay-notes:ru -->\nСамообновление portable-версии" });
  await page.goto("/");

  await page.getByRole("button", { name: "Open update 1.1.1" }).click();
  const dialog = page.getByRole("dialog", { name: "Update 1.1.1" });
  await expect(dialog).toContainText("Portable self-update");
  await expect(dialog).not.toContainText("Самообновление portable-версии");
  await expect(dialog.getByRole("button", { name: "Skip 1.1.1", exact: true })).toBeVisible();
  await expect(dialog.getByRole("button", { name: "Update", exact: true })).toBeVisible();
  await expect(dialog.getByRole("button", { name: "Later", exact: true })).toHaveCount(0);
  await dialog.getByRole("button", { name: "Update", exact: true }).click();

  const calls = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: Record<string, unknown> }> }).__TAURI_TEST_INVOKES__);
  expect(calls.find((call) => call.command === "plugin:updater|check")?.args).toMatchObject({ target: "windows-x86_64-portable" });
  expect(calls.some((call) => call.command === "install_portable_update")).toBe(true);
  expect(calls.some((call) => call.command === "plugin:updater|download_and_install")).toBe(false);
  expect(calls.some((call) => call.command === "plugin:process|restart")).toBe(false);
});

test("a portable executable ignores a valid manifest without its portable target", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, bundleType: null, portableUpdateTargetMissing: true, updateVersion: "1.1.1" });
  await page.goto("/");

  await expect.poll(() => page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string }> }).__TAURI_TEST_INVOKES__.filter((call) => call.command === "plugin:updater|check").length)).toBe(2);
  await expect(page.getByRole("button", { name: "Open update 1.1.1" })).toHaveCount(0);

  const checks = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: Record<string, unknown> }> }).__TAURI_TEST_INVOKES__.filter((call) => call.command === "plugin:updater|check").map((call) => call.args));
  expect(checks).toEqual([{ target: "windows-x86_64-portable" }, {}]);
});

test("a portable updater check keeps a verification failure visible", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, bundleType: null, updateCheckError: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Settings", exact: true }).click();

  await expect(page.getByText("Update failed", { exact: true })).toBeVisible();
  const checks = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string }> }).__TAURI_TEST_INVOKES__.filter((call) => call.command === "plugin:updater|check"));
  expect(checks).toHaveLength(2);
});

test("OAuth sign-in exposes only safe recovery actions", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await page.getByRole("button", { name: "Sign in", exact: true }).first().click();
  const dialog = page.getByRole("dialog", { name: "Sign in" });
  await expect(dialog.getByRole("button", { name: "Copy sign-in link" })).toBeVisible();
  const open = dialog.getByRole("button", { name: "Open in browser" });
  await expect(open).toBeEnabled();
  let calls = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: Record<string, unknown> }> }).__TAURI_TEST_INVOKES__);
  expect(calls.findLast((call) => call.command === "start_codex_oauth")?.args).toEqual({ openBrowser: false });
  expect(calls.some((call) => call.command === "resume_codex_oauth")).toBe(false);
  await open.click();
  await expect(dialog.getByRole("button", { name: /Open again in 3 s|Reopen sign-in page/ })).toBeVisible();
  calls = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: Record<string, unknown> }> }).__TAURI_TEST_INVOKES__);
  expect(calls.some((call) => call.command === "resume_codex_oauth")).toBe(true);
  await expect(dialog.locator("input, textarea, details")).toHaveCount(0);
});

test("OAuth countdown follows the active locale", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "ru", populated: true, codexBindings: false });
  await page.goto("/");

  await page.getByRole("button", { name: "Подключения", exact: true }).click();
  await page.getByRole("button", { name: "Войти", exact: true }).first().click();
  const dialog = page.getByRole("dialog", { name: "Войти" });
  await expect(dialog).toContainText(/Осталось времени\d+:\d{2}/);
  await expect(dialog).not.toContainText(/\b(?:AM|PM)\b/);
});

test("named ChatGPT snapshots save the current profile by default and allow opting out", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, canonicalProfilePath: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Recovery", exact: true }).click();

  await expect(page.getByRole("tab")).toHaveCount(0);
  await expect(page.getByText("History repair", { exact: true })).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Launch ChatGPT" })).toHaveCount(0);
  const automaticBackup = page.locator(".profile-automatic-backup");
  await expect(automaticBackup).toContainText("Before using Zenith");
  await expect(automaticBackup).toContainText("C:\\Users\\Test\\.codex");
  await expect(automaticBackup).not.toContainText("\\\\?\\");
  await automaticBackup.getByRole("button", { name: "Restore", exact: true }).click();
  await settleConfirmation(page);
  await page.getByRole("button", { name: "Open backups folder" }).click();

  await expect(page.getByRole("row").filter({ hasText: "Original profile" })).toBeVisible();
  await page.getByLabel("Snapshot name").fill("Before migration");
  await page.getByLabel("Snapshot name").press("Enter");
  const createdSnapshotId = "22222222-2222-4222-8222-000000000002";
  const created = page.getByRole("row").filter({ has: page.getByText("Before migration", { exact: true }) });
  await expect(created).toBeVisible();
  const snapshotRows = page.locator(".profile-snapshot-table tbody tr");
  await expect(snapshotRows).toHaveCount(2);

  await created.getByRole("button", { name: "Restore Before migration" }).click();
  const restoreDialog = page.getByRole("dialog", { name: "Restore snapshot" });
  const backupChoice = restoreDialog.getByRole("checkbox", { name: "Save the current profile first" });
  await expect(restoreDialog).toContainText("MCP connections, plugins, and unrelated settings stay untouched.");
  await expect(restoreDialog.getByRole("button", { name: "Restore full profile" })).toBeVisible();
  await expect(backupChoice).toBeChecked();
  await page.screenshot({ path: "output/playwright/profile-restore-dialog-1160x760.png" });
  await restoreDialog.getByRole("button", { name: "Restore full profile" }).click();
  const fullRestoreDialog = page.getByRole("dialog", { name: "Restore full ChatGPT profile" });
  await expect(fullRestoreDialog).toContainText('Restore the full ChatGPT profile from "Before migration"?');
  await expect(fullRestoreDialog).toContainText("replaces config.toml and auth.json completely");
  await fullRestoreDialog.getByRole("button", { name: "Restore full profile" }).click();
  await expect(fullRestoreDialog).toHaveCount(0);
  await expect(snapshotRows).toHaveCount(3);
  let calls = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: Record<string, unknown> }> }).__TAURI_TEST_INVOKES__);
  const fullRestoreCalls = calls.filter((call) => call.command === "restore_full_codex_profile_snapshot");
  expect(fullRestoreCalls).toHaveLength(1);
  expect(fullRestoreCalls[0]?.args).toEqual({
    snapshotId: createdSnapshotId,
    safetyName: "Before restoring Before migration",
  });

  await created.getByRole("button", { name: "Restore Before migration" }).click();
  await expect(backupChoice).toBeChecked();
  await restoreDialog.getByRole("button", { name: "Restore Relay settings" }).click();
  await expect(snapshotRows).toHaveCount(4);
  calls = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: Record<string, unknown> }> }).__TAURI_TEST_INVOKES__);
  let restoreCalls = calls.filter((call) => call.command === "restore_codex_profile_snapshot");
  expect(restoreCalls).toHaveLength(1);
  expect(restoreCalls[0]?.args).toEqual({ snapshotId: createdSnapshotId, safetyName: "Before restoring Before migration" });

  await created.getByRole("button", { name: "Restore Before migration" }).click();
  await backupChoice.uncheck();
  await restoreDialog.getByRole("button", { name: "Restore Relay settings" }).click();
  await expect(snapshotRows).toHaveCount(4);
  calls = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: Record<string, unknown> }> }).__TAURI_TEST_INVOKES__);
  restoreCalls = calls.filter((call) => call.command === "restore_codex_profile_snapshot");
  expect(restoreCalls).toHaveLength(2);
  expect(restoreCalls[1]?.args).toEqual({ snapshotId: createdSnapshotId, safetyName: null });

  await page.getByRole("row").filter({ has: page.getByText("Before migration", { exact: true }) }).getByRole("button", { name: "Delete Before migration", exact: true }).click();
  await settleConfirmation(page);
  await expect(page.getByRole("row").filter({ has: page.getByText("Before migration", { exact: true }) })).toHaveCount(0);

  calls = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: Record<string, unknown> }> }).__TAURI_TEST_INVOKES__);
  expect(calls.some((call) => call.command === "restore_codex_profile")).toBe(true);
  expect(calls.filter((call) => call.command === "restore_codex_profile_snapshot")).toHaveLength(2);
  expect(calls.filter((call) => call.command === "create_codex_profile_snapshot")).toHaveLength(1);
  expect(calls.findLast((call) => call.command === "open_relay_folder")?.args).toEqual({ folder: "profile_backups" });
});

test("recovery restores an OAuth profile with its profile-specific command", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, profileSnapshotsEmpty: true, codexBindingKind: "oauth_account" });
  await page.goto("/");
  await page.getByRole("button", { name: "Recovery", exact: true }).click();

  const backup = page.locator(".profile-automatic-backup");
  await expect(backup).toContainText("C:\\Users\\Test\\.codex");
  await backup.getByRole("button", { name: "Restore", exact: true }).click();
  await settleConfirmation(page);

  const call = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: Record<string, unknown> }> }).__TAURI_TEST_INVOKES__.findLast((item) => item.command === "restore_codex_account_profile"));
  expect(call?.args).toEqual({ profileDir: "C:\\Users\\Test\\.codex" });
});

test("recovery hides actions that have no available backup", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, codexBindings: false, profileSnapshotsEmpty: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Recovery", exact: true }).click();

  await expect(page.getByText("No named snapshots", { exact: true })).toBeVisible();
  await expect(page.locator(".profile-automatic-backup")).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Restore", exact: true })).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Open backups folder" })).toBeVisible();
});

test("recovery reports load failures instead of claiming backups are empty", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", recoveryLoadError: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Recovery", exact: true }).click();

  await expect(page.getByRole("alert")).toContainText("Some recovery data could not be loaded");
  await expect(page.getByRole("button", { name: "Retry" })).toBeVisible();
  await expect(page.getByText("No named snapshots", { exact: true })).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Open backups folder" })).toBeVisible();
});

test("account identities are controlled only from the global action", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();

  const identity = page.locator(".account-card").first().locator(".account-identity > strong");
  await expect(identity).toHaveText("Personal Plus");
  await expect(page.getByText("p***@example.test")).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Show full identity", exact: true })).toHaveCount(0);
  await page.getByRole("button", { name: "Show all account identities" }).click();
  await expect(identity).toHaveText("person@example.test");
  await expect(page.locator(".account-card").first().getByText("Personal Plus", { exact: true })).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Hide full identity", exact: true })).toHaveCount(0);
  await page.getByRole("button", { name: "Hide all account identities" }).click();
  await expect(identity).toHaveText("Personal Plus");

  const call = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: Record<string, unknown> }> }).__TAURI_TEST_INVOKES__.find((item) => item.command === "reveal_local_account_identity"));
  expect(call?.args).toEqual({ accountId: "account_synthetic" });
});

test("stale identity reveal cannot replace or finish a newer mode request", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true });
  await page.goto("/");
  await page.evaluate(() => {
    type RevealMode = "local" | "remote";
    type PendingReveal = { accountId: string; resolve: (value: unknown) => void };
    const pending: Record<RevealMode, PendingReveal[]> = { local: [], remote: [] };
    const internals = (window as unknown as {
      __TAURI_INTERNALS__: {
        invoke: (command: string, args?: unknown, options?: unknown) => Promise<unknown>;
      };
    }).__TAURI_INTERNALS__;
    const originalInvoke = internals.invoke.bind(internals);
    internals.invoke = (command, args, options) => {
      const mode = command === "reveal_local_account_identity" ? "local"
        : command === "reveal_remote_account_identity" ? "remote"
        : null;
      if (!mode) return originalInvoke(command, args, options);
      const accountId = String((args as { accountId?: unknown } | undefined)?.accountId ?? "");
      return new Promise((resolve) => pending[mode].push({ accountId, resolve }));
    };
    Object.defineProperty(window, "__RESOLVE_IDENTITY_REVEAL__", {
      configurable: true,
      value: (mode: RevealMode, identity: string) => {
        const request = pending[mode].shift();
        if (!request) throw new Error(`no pending ${mode} identity reveal`);
        request.resolve({ accountId: request.accountId, identity });
      },
    });
    Object.defineProperty(window, "__PENDING_IDENTITY_REVEALS__", {
      configurable: true,
      value: (mode: RevealMode) => pending[mode].length,
    });
  });

  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await page.getByRole("button", { name: "Show all account identities" }).click();
  await expect.poll(() => page.evaluate(() => (window as unknown as { __PENDING_IDENTITY_REVEALS__: (mode: "local" | "remote") => number }).__PENDING_IDENTITY_REVEALS__("local"))).toBe(1);

  await page.locator('.mode-picker > button[aria-haspopup="menu"]').click();
  await page.getByRole("menuitemradio", { name: "On your server", exact: true }).click();
  await expect.poll(() => page.evaluate(() => localStorage.getItem("relay.mode"))).toBe("remote");
  await expect.poll(() => page.evaluate(() => (window as unknown as { __PENDING_IDENTITY_REVEALS__: (mode: "local" | "remote") => number }).__PENDING_IDENTITY_REVEALS__("remote"))).toBe(1);

  await page.evaluate(() => (window as unknown as { __RESOLVE_IDENTITY_REVEAL__: (mode: "local" | "remote", identity: string) => void }).__RESOLVE_IDENTITY_REVEAL__("local", "stale@example.test"));
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  const identityAction = page.getByRole("button", { name: "Hide all account identities", exact: true });
  await expect(identityAction).toBeVisible();
  await expect(identityAction).toBeDisabled();
  await expect(page.getByText("stale@example.test", { exact: true })).toHaveCount(0);

  await page.evaluate(() => (window as unknown as { __RESOLVE_IDENTITY_REVEAL__: (mode: "local" | "remote", identity: string) => void }).__RESOLVE_IDENTITY_REVEAL__("remote", "remote@example.test"));
  await expect(identityAction).toBeEnabled();
  await expect(page.locator(".account-identity > strong").first()).toHaveText("remote@example.test");

  await page.locator('.mode-picker > button[aria-haspopup="menu"]').click();
  await page.getByRole("menuitemradio", { name: "Computer", exact: true }).click();
  await expect.poll(() => page.evaluate(() => localStorage.getItem("relay.mode"))).toBe("local");
  await expect.poll(() => page.evaluate(() => (window as unknown as { __PENDING_IDENTITY_REVEALS__: (mode: "local" | "remote") => number }).__PENDING_IDENTITY_REVEALS__("local"))).toBe(1);
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await expect(page.getByText("stale@example.test", { exact: true })).toHaveCount(0);
  await page.evaluate(() => (window as unknown as { __RESOLVE_IDENTITY_REVEAL__: (mode: "local" | "remote", identity: string) => void }).__RESOLVE_IDENTITY_REVEAL__("local", "fresh@example.test"));
  await expect(page.locator(".account-identity > strong").first()).toHaveText("fresh@example.test");
});

test("account identity visibility applies across the workspace and survives reloads", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, accountCount: 3 });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await page.getByRole("button", { name: "Show all account identities" }).click();
  await expect(page.locator(".account-identity > strong", { hasText: "person@example.test" })).toHaveCount(3);
  await expect.poll(() => page.evaluate(() => localStorage.getItem("relay.accountIdentitiesVisible"))).toBe("1");

  await page.getByRole("button", { name: "Pool", exact: true }).click();
  await expect(page.locator('.pool-member-card[data-member-kind="account"] .pool-member-name')).toHaveText(["person@example.test", "person@example.test", "person@example.test"]);

  await page.getByRole("button", { name: "Usage", exact: true }).click();
  await expect(page.locator('.usage-request-table tbody tr td[data-column="connection"]')).toHaveText("person@example.test");
  await page.getByRole("tab", { name: "Pool members", exact: true }).click();
  await expect(page.locator(".usage-aggregate-table tbody tr td").first()).toHaveText("person@example.test");

  await page.reload();
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await expect(page.getByRole("button", { name: "Hide all account identities" })).toBeVisible();
  await expect(page.locator(".account-identity > strong").first()).toHaveText("person@example.test");
  await page.getByRole("button", { name: "Hide all account identities" }).click();
  await expect(page.getByText("Personal Plus", { exact: true })).toBeVisible();
  await expect(page.getByText("Business Workspace", { exact: true })).toBeVisible();
  await expect(page.getByText("Backup account", { exact: true })).toBeVisible();
  await expect.poll(() => page.evaluate(() => localStorage.getItem("relay.accountIdentitiesVisible"))).toBe("0");

  await page.reload();
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await expect(page.getByRole("button", { name: "Show all account identities" })).toBeVisible();
  expect(await page.locator(".account-identity > strong").allTextContents()).toEqual(expect.arrayContaining(["Personal Plus", "Business Workspace", "Backup account"]));
  await expect(page.getByText("person@example.test", { exact: true })).toHaveCount(0);

  const calls = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string }> }).__TAURI_TEST_INVOKES__.filter((item) => item.command === "reveal_local_account_identity").length);
  expect(calls).toBe(0);
});

test("remote account identity reveal uses the negotiated server capability", async ({ page }) => {
  await installTauriMock(page, { mode: "remote", locale: "en", populated: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await page.getByRole("button", { name: "Show all account identities" }).click();
  await expect(page.getByText("person@example.test")).toBeVisible();

  const call = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: Record<string, unknown> }> }).__TAURI_TEST_INVOKES__.find((item) => item.command === "reveal_remote_account_identity"));
  expect(call?.args).toEqual({ accountId: "account_synthetic" });
});

test("remote usage never exposes an unresolved internal account hash", async ({ page }) => {
  await installTauriMock(page, { mode: "remote", locale: "en", populated: true, remoteUsageLabelMissing: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Usage", exact: true }).click();

  await expect(page.locator('.usage-request-table tbody tr td[data-column="connection"]')).toHaveText("Unknown account");
  await expect(page.locator('.usage-request-table tbody tr td[data-column="tier"]')).toHaveText("Fast for all → Follow ChatGPT");
  await expect(page.getByText("4f5c821a909b", { exact: true })).toHaveCount(0);
});

test("stale local usage and proxy references never expose internal account IDs", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, proxyCount: 1, staleAccountReferences: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Usage", exact: true }).click();
  await expect(page.locator('.usage-request-table tbody tr td[data-column="connection"]')).toHaveText("Unknown account");

  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await page.getByRole("tab", { name: "Proxies" }).click();
  await expect(page.locator(".proxy-storage-row").first()).toContainText("Unknown account");
  await expect(page.getByText("account_deleted_internal", { exact: true })).toHaveCount(0);
});

test("an exhausted weekly quota makes the account effectively unavailable in connections and pool", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, exhaustedQuotaWindow: "secondary" });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await expect(page.locator(".account-card").first().locator(".quota-meter strong")).toHaveText(["0%", "0%"]);

  await page.getByRole("button", { name: "Pool", exact: true }).click();
  const accountCard = page.locator('.pool-member-card[data-member-label="Personal Plus"]');
  await expect(accountCard.locator(".quota-meter strong")).toHaveText(["0%", "0%"]);
});

test("account economics visibility is shared by connections and pool and persists", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  const connectionEconomics = page.locator(".account-card .account-economics-strip");

  await expect(connectionEconomics.first()).toBeVisible();
  await expect(page.getByRole("button", { name: "Hide account economics" })).toHaveAttribute("aria-pressed", "true");
  await page.getByRole("button", { name: "Hide account economics" }).click();
  await expect(connectionEconomics).toHaveCount(0);
  await expect.poll(() => page.evaluate(() => localStorage.getItem("relay.poolEconomicsVisible"))).toBe("false");

  await page.getByRole("button", { name: "Pool", exact: true }).click();
  const poolEconomics = page.locator('.pool-member-card[data-member-kind="account"] .account-economics-strip');
  await expect(page.getByRole("button", { name: "Show account economics" })).toHaveAttribute("aria-pressed", "false");
  await expect(poolEconomics).toHaveCount(0);
  await page.getByRole("button", { name: "Show account economics" }).click();
  await expect(poolEconomics.first()).toBeVisible();

  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await expect(page.getByRole("button", { name: "Hide account economics" })).toHaveAttribute("aria-pressed", "true");
  await expect(connectionEconomics.first()).toBeVisible();
  await page.reload();
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await expect(connectionEconomics.first()).toBeVisible();
});

test("quota cards name provider windows and make remaining percentages explicit", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, accountCount: 3 });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();

  const free = page.locator(".account-card").filter({ hasText: "Backup account" });
  await expect(free.locator(".quota-meter-heading > span")).toHaveText("5 weeks");
  await expect(free.locator(".quota-meter-heading > strong")).toHaveText("95%");
  await expect(free.locator(".quota-track")).toHaveAttribute("aria-label", "5 weeks: 95%");
});

test("pool toggle changes state without switching ChatGPT", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, gatewayRunning: false });
  await page.goto("/");
  await page.getByRole("button", { name: "Pool", exact: true }).click();
  await page.getByRole("button", { name: "Start pool", exact: true }).click();
  await expect(page.getByText("Endpoint started.")).toBeVisible();
  const header = page.locator(".relay-page-header");
  await expect(header.getByRole("button", { name: "Switch ChatGPT to pool", exact: true })).toBeVisible();
  await expect(header.locator(".pool-header-actions > *")).toHaveCount(4);
  await expect(header.getByRole("button", { name: "Save preset", exact: true })).toBeVisible();
  await header.getByRole("button", { name: "Stop pool", exact: true }).click();
  await expect(page.getByText("Endpoint stopped.")).toBeVisible();

  const calls = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: Record<string, unknown> }> }).__TAURI_TEST_INVOKES__);
  const workflow = calls.filter((call) => ["start_local_gateway", "stop_local_gateway", "attach_codex_to_local_gateway", "launch_managed_codex_profile"].includes(call.command));
  expect(workflow.map((call) => call.command)).toEqual(["start_local_gateway", "stop_local_gateway"]);
});

test("pool controls delegate an exhausted OAuth account to the backend", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, accountCount: 1, poolMembers: false, gatewayRunning: false });
  await page.goto("/");
  await page.getByRole("button", { name: "Pool", exact: true }).click();

  await page.getByRole("button", { name: "Add member", exact: true }).first().click();
  const dialog = page.getByRole("dialog", { name: "Add connections to pool" });
  await dialog.getByText("Personal Plus", { exact: true }).click();
  await dialog.getByRole("button", { name: "Add selected (1)" }).click();

  const start = page.getByRole("button", { name: "Start pool", exact: true });
  await expect(start).toBeEnabled();
  await start.click();
  const switchToPool = page.getByRole("button", { name: "Switch ChatGPT to pool", exact: true });
  await expect(switchToPool).toBeEnabled();
  await switchToPool.click();

  await expect.poll(() => page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string }> }).__TAURI_TEST_INVOKES__.map((call) => call.command))).toEqual(expect.arrayContaining([
    "start_local_gateway",
    "attach_codex_to_local_gateway",
  ]));
});

test("switch ChatGPT uses the backend system key and relaunches ChatGPT without starting the gateway", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, gatewayRunning: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Pool", exact: true }).click();
  await page.getByRole("button", { name: "Switch ChatGPT to pool", exact: true }).click();
  const feedback = page.locator(".global-feedback.success");
  await expect(feedback).toContainText("Client launched.");
  const [feedbackBox, headerBox, poolControlsBox] = await Promise.all([
    feedback.boundingBox(),
    page.locator(".relay-page-header").boundingBox(),
    page.locator(".pool-controls").boundingBox(),
  ]);
  expect(feedbackBox).not.toBeNull();
  expect(headerBox).not.toBeNull();
  expect(poolControlsBox).not.toBeNull();
  expect(feedbackBox!.x).toBeGreaterThan(page.viewportSize()!.width / 2);
  expect(feedbackBox!.y + feedbackBox!.height).toBeLessThanOrEqual(headerBox!.y);
  expect(feedbackBox!.y + feedbackBox!.height).toBeLessThanOrEqual(poolControlsBox!.y);

  const calls = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: Record<string, unknown> }> }).__TAURI_TEST_INVOKES__);
  const workflow = calls.filter((call) => ["start_local_gateway", "attach_codex_to_local_gateway", "launch_managed_codex_profile"].includes(call.command));
  expect(workflow.map((call) => call.command)).toEqual(["attach_codex_to_local_gateway", "launch_managed_codex_profile"]);
  expect(workflow[0].args).toEqual({ boundOauthAccountId: null });
  expect(calls.some((call) => call.command === "create_codex_profile_snapshot")).toBe(false);
  await expect(page.getByRole("dialog", { name: "Confirm action" })).toHaveCount(0);
  await expect(feedback).toBeHidden({ timeout: 5_000 });
});

test("profile switch errors remain readable and then dismiss automatically", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, gatewayRunning: true, profileSwitchError: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Pool", exact: true }).click();
  await page.getByRole("button", { name: "Switch ChatGPT to pool", exact: true }).click();

  const feedback = page.locator(".global-feedback.error");
  await expect(feedback).toContainText("The profile changed during the operation.");
  await expect(feedback).toBeHidden({ timeout: 9_000 });
});

test("global errors expose sanitized details and a copy confirmation", async ({ page }) => {
  await page.context().grantPermissions(["clipboard-read", "clipboard-write"], { origin: "http://127.0.0.1:1420" });
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, gatewayRunning: true, profileSwitchError: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Pool", exact: true }).click();
  await page.getByRole("button", { name: "Switch ChatGPT to pool", exact: true }).click();

  const feedback = page.locator(".global-feedback.error");
  const errorMenu = feedback.locator(".global-feedback-menu summary");
  await expect(errorMenu).toBeVisible();
  await errorMenu.click();
  await feedback.getByRole("menuitem", { name: "Show details" }).click();
  const details = feedback.getByRole("region", { name: "Error details" });
  await expect(details).toContainText('"code": "profile_restore_blocked"');
  await expect(details).toContainText('"message": "Synthetic profile conflict"');

  await errorMenu.click();
  await feedback.getByRole("menuitem", { name: "Copy error JSON" }).click();
  await expect(feedback.locator(".global-feedback-copy-state")).toHaveText("Copied");
  await expect.poll(() => page.evaluate(() => navigator.clipboard.readText())).toContain('"profile_restore_blocked"');
});

test("focus refreshes runtime only after a state revision changes", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true });
  await page.goto("/");
  const stateReads = () => page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string }> }).__TAURI_TEST_INVOKES__.filter((call) => call.command === "get_local_runtime_state").length);
  const before = await stateReads();
  await page.evaluate(() => window.dispatchEvent(new Event("focus")));
  await page.waitForTimeout(300);
  expect(await stateReads()).toBe(before);

  await emitTauriEvent(page, "zenith-state-changed", null);
  await expect.poll(stateReads).toBeGreaterThan(before);
  const refreshed = await stateReads();
  await page.evaluate(() => window.dispatchEvent(new Event("focus")));
  await page.waitForTimeout(300);
  expect(await stateReads()).toBe(refreshed);
});

test("background refresh catches a revision emitted during an in-flight snapshot", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true });
  await page.goto("/");
  await page.evaluate(() => {
    const internals = (window as unknown as { __TAURI_INTERNALS__: { invoke: (command: string, args?: unknown, options?: unknown) => Promise<unknown> } }).__TAURI_INTERNALS__;
    const invoke = internals.invoke.bind(internals);
    let delayNextStateRead = true;
    internals.invoke = (command, args, options) => {
      if (command !== "get_local_runtime_state" || !delayNextStateRead) return invoke(command, args, options);
      delayNextStateRead = false;
      (window as unknown as { __DELAYED_STATE_READ_STARTED__: boolean }).__DELAYED_STATE_READ_STARTED__ = true;
      return new Promise((resolve, reject) => window.setTimeout(() => invoke(command, args, options).then(resolve, reject), 300));
    };
  });
  const stateReads = () => page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string }> }).__TAURI_TEST_INVOKES__.filter((call) => call.command === "get_local_runtime_state").length);
  const before = await stateReads();
  await emitTauriEvent(page, "zenith-state-changed", null);
  await expect.poll(() => page.evaluate(() => Boolean((window as unknown as { __DELAYED_STATE_READ_STARTED__?: boolean }).__DELAYED_STATE_READ_STARTED__))).toBe(true);
  await emitTauriEvent(page, "zenith-state-changed", null);
  await expect.poll(stateReads).toBeGreaterThanOrEqual(before + 2);
});

test("background snapshots and analytics stay dormant on inactive pages", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Settings", exact: true }).click();
  const stateReads = () => page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string }> }).__TAURI_TEST_INVOKES__.filter((call) => call.command === "get_local_runtime_state").length);
  const usageReads = () => page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string }> }).__TAURI_TEST_INVOKES__.filter((call) => call.command === "get_local_usage_page").length);
  const stateReadsBefore = await stateReads();
  const before = await usageReads();
  await emitTauriEvent(page, "zenith-state-changed", null);
  await page.waitForTimeout(800);
  expect(await stateReads()).toBe(stateReadsBefore);
  expect(await usageReads()).toBe(before);

  await page.getByRole("button", { name: "Overview", exact: true }).click();
  await expect.poll(stateReads).toBeGreaterThan(stateReadsBefore);
});

test("runtime snapshots stay off Usage while active Usage reloads its own data", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true });
  await page.goto("/");
  const stateReads = () => page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string }> }).__TAURI_TEST_INVOKES__.filter((call) => call.command === "get_local_runtime_state").length);
  const usageReads = () => page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string }> }).__TAURI_TEST_INVOKES__.filter((call) => call.command === "get_local_usage_page").length);
  await expect.poll(usageReads).toBeGreaterThan(0);
  const overviewReads = await usageReads();
  await emitTauriEvent(page, "zenith-state-changed", null);
  await expect.poll(usageReads).toBeGreaterThan(overviewReads);

  await page.getByRole("button", { name: "Usage", exact: true }).click();
  await expect.poll(usageReads).toBeGreaterThan(overviewReads + 1);
  const usagePageReads = await usageReads();
  const usagePageStateReads = await stateReads();
  await emitTauriEvent(page, "zenith-state-changed", null);
  await expect.poll(usageReads).toBeGreaterThan(usagePageReads);
  expect(await stateReads()).toBe(usagePageStateReads);
});

test("Overview keeps rendered analytics while a background refresh is pending or fails", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true });
  await page.goto("/");
  const analytics = page.locator(".overview-analytics");
  const tokenSummary = page.locator(".overview-chart.tokens .overview-chart-summary");
  const activity = page.locator(".activity-section li");
  await expect(tokenSummary).toHaveText("28");
  await expect(activity).toHaveCount(1);

  await page.evaluate(() => {
    const internals = (window as unknown as { __TAURI_INTERNALS__: { invoke: (command: string, args?: unknown, options?: unknown) => Promise<unknown> } }).__TAURI_INTERNALS__;
    const invoke = internals.invoke.bind(internals);
    let delayNextUsageRead = true;
    internals.invoke = (command, args, options) => {
      if (command !== "get_local_usage_page" || !delayNextUsageRead) return invoke(command, args, options);
      delayNextUsageRead = false;
      (window as unknown as { __OVERVIEW_USAGE_REFRESH_PENDING__?: boolean }).__OVERVIEW_USAGE_REFRESH_PENDING__ = true;
      return new Promise((resolve, reject) => window.setTimeout(() => invoke(command, args, options).then(resolve, reject), 400));
    };
  });
  await emitTauriEvent(page, "zenith-state-changed", null);
  await expect.poll(() => page.evaluate(() => Boolean((window as unknown as { __OVERVIEW_USAGE_REFRESH_PENDING__?: boolean }).__OVERVIEW_USAGE_REFRESH_PENDING__))).toBe(true);
  await expect(analytics).toHaveAttribute("aria-busy", "true");
  await expect(tokenSummary).toHaveText("28");
  await expect(activity).toHaveCount(1);
  await expect(analytics).toHaveAttribute("aria-busy", "false");

  await page.evaluate(() => {
    const internals = (window as unknown as { __TAURI_INTERNALS__: { invoke: (command: string, args?: unknown, options?: unknown) => Promise<unknown> } }).__TAURI_INTERNALS__;
    const invoke = internals.invoke.bind(internals);
    let failNextUsageRead = true;
    internals.invoke = (command, args, options) => {
      if (command !== "get_local_usage_page" || !failNextUsageRead) return invoke(command, args, options);
      failNextUsageRead = false;
      return Promise.reject(new Error("Synthetic overview usage error"));
    };
  });
  await emitTauriEvent(page, "zenith-state-changed", null);
  await expect(analytics.getByRole("alert")).toBeVisible();
  await expect(tokenSummary).toHaveText("28");
  await expect(activity).toHaveCount(1);
});

test("open request details follow the terminal fallback result", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, usageFailure: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Usage", exact: true }).click();
  await page.getByRole("button", { name: "Request details: req_synthetic_local" }).click();
  const dialog = page.getByRole("dialog", { name: "Request details" });
  const httpStatus = dialog.locator(".detail-list > div").filter({ hasText: "HTTP status" }).locator("dd");
  await expect(dialog.locator(".detail-list > div").nth(1).locator("dd")).toHaveText("Failed");
  await expect(httpStatus).toHaveText("502");

  await page.evaluate(() => {
    const internals = (window as unknown as { __TAURI_INTERNALS__: { invoke: (command: string, args?: unknown, options?: unknown) => Promise<unknown> } }).__TAURI_INTERNALS__;
    const invoke = internals.invoke.bind(internals);
    internals.invoke = async (command, args, options) => {
      const result = await invoke(command, args, options);
      if (command !== "get_local_usage_page") return result;
      const usage = structuredClone(result) as { events: Array<{ attempt: number; success: boolean; httpStatus: number; errorCategory: string | null; latencyMs: number }>; totals: { requests: number; successfulRequests: number } };
      if (usage.events[0]) Object.assign(usage.events[0], { attempt: 2, success: true, httpStatus: 200, errorCategory: null, latencyMs: 16_157 });
      usage.totals.successfulRequests = usage.totals.requests;
      return usage;
    };
  });
  await emitTauriEvent(page, "zenith-state-changed", null);

  await expect(dialog.locator(".detail-list > div").nth(1).locator("dd")).toHaveText("Success");
  await expect(httpStatus).toHaveText("200");
});

test("switching modes ignores a late failure from the previous mode", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, gatewayRunning: true });
  await page.goto("/");
  await page.evaluate(() => {
    const internals = (window as unknown as { __TAURI_INTERNALS__: { invoke: (command: string, args?: unknown, options?: unknown) => Promise<unknown> } }).__TAURI_INTERNALS__;
    const invoke = internals.invoke.bind(internals);
    internals.invoke = (command, args, options) => command === "stop_local_gateway"
      ? new Promise((_, reject) => window.setTimeout(() => reject({ code: "not_found", message: "Synthetic stale record" }), 150))
      : invoke(command, args, options);
  });

  await page.getByRole("button", { name: "Stop API", exact: true }).click();
  await page.locator('.mode-picker > button[aria-haspopup="menu"]').click();
  await page.getByRole("menuitemradio", { name: "Choose API", exact: true }).click();

  await expect.poll(() => page.evaluate(() => localStorage.getItem("relay.mode"))).toBe("zenith");
  await page.waitForTimeout(250);
  await expect(page.getByText("The requested record was not found.", { exact: true })).toHaveCount(0);
});

test("startup records theme, i18n, first-frame, and interactive timings", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", theme: "dark", populated: true });
  await page.goto("/");
  await expect.poll(() => page.evaluate(() => performance.getEntriesByType("measure").filter((entry) => entry.name.startsWith("zenith:")).map((entry) => entry.name))).toEqual(expect.arrayContaining(["zenith:i18n", "zenith:first-frame", "zenith:interactive"]));
  const timings = await page.evaluate(() => Object.fromEntries(performance.getEntriesByType("measure").filter((entry) => entry.name.startsWith("zenith:")).map((entry) => [entry.name, entry.duration])));
  expect(timings["zenith:i18n"]).toBeGreaterThanOrEqual(0);
  expect(timings["zenith:first-frame"]).toBeGreaterThanOrEqual(timings["zenith:i18n"]);
  expect(timings["zenith:interactive"]).toBeGreaterThanOrEqual(timings["zenith:i18n"]);
});

test("navigation records Pool, Connections, and mode switch timings", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true });
  await page.goto("/");
  const samples = () => page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: { name?: string; durationMs?: number; context?: string } }> }).__TAURI_TEST_INVOKES__.filter((call) => call.command === "record_local_performance_sample"));

  await page.getByRole("button", { name: "Pool", exact: true }).click();
  await expect.poll(async () => (await samples()).some((call) => call.args.name === "page_open" && call.args.context === "pool")).toBe(true);
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await expect.poll(async () => (await samples()).some((call) => call.args.name === "page_open" && call.args.context === "connections")).toBe(true);
  await page.locator('.mode-picker > button[aria-haspopup="menu"]').click();
  await page.getByRole("menuitemradio", { name: "Choose API", exact: true }).click();
  await expect.poll(async () => (await samples()).some((call) => call.args.name === "mode_switch" && call.args.context === "zenith")).toBe(true);

  const measured = (await samples()).filter((call) =>
    (call.args.name === "page_open" && ["pool", "connections"].includes(call.args.context ?? ""))
    || (call.args.name === "mode_switch" && call.args.context === "zenith"));
  expect(measured).toHaveLength(3);
  expect(measured.every((call) => Number.isFinite(call.args.durationMs) && call.args.durationMs! >= 0)).toBe(true);
});

test("row launch keeps all quota windows visible", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();

  await expect(page.getByRole("button", { name: "Launch selected" })).toHaveCount(0);
  await expect(page.locator(".quota-display-menu")).toHaveCount(0);
  await expect(page.locator(".account-list .quota-meter")).toHaveCount(2);
  const launch = page.getByRole("button", { name: "Launch in ChatGPT" });
  await expect(launch).toBeEnabled();
  await launch.click();
  await expect(page.getByText("Client launched.")).toBeVisible();
  await expect(page.getByRole("dialog", { name: /sessions visible|видимость чатов/i })).toHaveCount(0);

  await page.getByRole("button", { name: "Settings", exact: true }).click();
  await expect(page.getByText("Visible quota windows")).toHaveCount(0);

  const call = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: Record<string, unknown> }> }).__TAURI_TEST_INVOKES__.findLast((item) => item.command === "launch_codex_account"));
  expect(call?.args).toEqual({ accountId: "account_synthetic" });
  const profileCommands = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string }> }).__TAURI_TEST_INVOKES__.map((item) => item.command).filter((command) => ["launch_codex_account", "launch_managed_codex_profile"].includes(command)));
  expect(profileCommands).toEqual(["launch_codex_account", "launch_managed_codex_profile"]);
});

test("remote account export uses the capability-gated server command", async ({ page }) => {
  await installTauriMock(page, { mode: "remote", locale: "en", populated: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await page.locator(".account-bulk-menu summary").click();
  await page.getByRole("menuitem", { name: "Export all" }).click();
  const dialog = page.getByRole("dialog", { name: "Export accounts" });
  await expect(dialog.getByRole("radio", { name: "Zenith" })).toHaveAttribute("aria-checked", "true");
  await dialog.getByRole("button", { name: "Download JSON" }).click();

  const call = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: Record<string, unknown> }> }).__TAURI_TEST_INVOKES__.findLast((item) => item.command === "export_remote_accounts"));
  expect(call?.args.input).toEqual({ accountIds: ["account_synthetic"], format: "zenith", destination: "download" });
});

test("stored account proxy controls keep saved addresses hidden", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true });
  await page.goto("/");

  await page.getByRole("button", { name: "Connections", exact: true }).click();
  const accountCard = page.locator(".account-card").first();
  await accountCard.getByRole("button", { name: "Proxy: Common", exact: true }).click();
  let accountDialog = page.getByRole("dialog", { name: "Account proxy" });
  await accountDialog.getByRole("radio", { name: /No proxy/ }).click();
  await accountDialog.getByRole("button", { name: "Save" }).click();
  await expect(accountCard.getByRole("button", { name: "Proxy: No proxy", exact: true })).toBeVisible();
  await accountCard.getByRole("button", { name: "Proxy: No proxy", exact: true }).click();
  const accountProxy = "account-user:account-pass@us-account.example:8081";
  accountDialog = page.getByRole("dialog", { name: "Account proxy" });
  await accountDialog.getByRole("radio", { name: /Add a new proxy/ }).click();
  await accountDialog.getByLabel("HTTP(S) proxy").fill(accountProxy);
  await accountDialog.getByRole("button", { name: "Save" }).click();
  await expect(page.getByText(accountProxy)).toHaveCount(0);
  await expect(accountCard.getByRole("button", { name: "Proxy: Per-account", exact: true })).toBeVisible();

  await page.locator(".account-bulk-menu summary").click();
  await page.getByRole("menuitem", { name: "Assign proxies" }).click();
  const bulkDialog = page.getByRole("dialog", { name: "Assign account proxies" });
  await bulkDialog.getByRole("button", { name: "Assign automatically" }).click();
  await expect(bulkDialog.getByRole("status")).toContainText("Assigned 0; unchanged 1; unavailable 0.");
  await expect(page.getByText("account-pass")).toHaveCount(0);

  const calls = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: Record<string, unknown> }> }).__TAURI_TEST_INVOKES__);
  expect(calls.some((call) => call.command === "set_local_account_proxy" && (call.args.input as { bypassCommonProxy?: boolean })?.bypassCommonProxy === true)).toBe(true);
  expect(calls.findLast((call) => call.command === "set_local_account_proxy")?.args).toEqual({ input: { accountId: "account_synthetic", proxyUrl: accountProxy, bypassCommonProxy: false } });
  expect(calls.findLast((call) => call.command === "assign_free_local_account_proxies")?.args).toEqual({ input: { accountIds: ["account_synthetic"] } });
});

test("remote proxy controls use the capability-gated management actions", async ({ page }) => {
  await installTauriMock(page, { mode: "remote", locale: "en", populated: true });
  await page.goto("/");

  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await page.locator(".account-card").first().getByRole("button", { name: "Proxy: Common", exact: true }).click();
  const accountDialog = page.getByRole("dialog", { name: "Account proxy" });
  await accountDialog.getByRole("radio", { name: /Add a new proxy/ }).click();
  await accountDialog.getByLabel("HTTP(S) proxy").fill("remote-account:secret@us-account.example:8081");
  await accountDialog.getByRole("button", { name: "Save" }).click();
  await page.locator(".account-bulk-menu summary").click();
  await page.getByRole("menuitem", { name: "Assign proxies" }).click();
  const bulkDialog = page.getByRole("dialog", { name: "Assign account proxies" });
  await bulkDialog.getByLabel("Proxy list").fill("remote-bulk:secret@us-bulk.example:8082");
  await bulkDialog.getByRole("button", { name: "Assign", exact: true }).click();

  const actions = await page.evaluate(() => {
    const calls = (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: { input?: { action?: { type?: string } } } }> }).__TAURI_TEST_INVOKES__;
    return calls.filter((call) => call.command === "execute_remote_server_action").map((call) => call.args.input?.action?.type);
  });
  expect(actions).toEqual(["set_account_proxy", "assign_account_proxies"]);
});

test("remote trust and deployment secrets require explicit actions", async ({ page }) => {
  await installTauriMock(page, { mode: "remote", locale: "en", populated: true, remoteConnected: false });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await page.getByRole("tab", { name: "Remote Server" }).click();
  await page.getByRole("button", { name: "Connect existing server" }).click();
  await page.getByLabel("Server address").fill("http://127.0.0.1:14999");
  await page.getByLabel("Management token").fill("synthetic-management-token-000000");
  await expect(page.getByRole("button", { name: "Test and connect" })).toBeDisabled();
  await page.getByLabel("Allow unencrypted HTTP").check();
  await page.getByLabel("Trust a new identity").check();
  await page.getByRole("button", { name: "Test and connect" }).click();
  const connectInput = await page.evaluate(() => {
    const calls = (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: { input?: Record<string, unknown> } }> }).__TAURI_TEST_INVOKES__;
    return calls.findLast((call) => call.command === "connect_remote_server")?.args.input;
  });
  expect(connectInput).toMatchObject({ allowInsecureHttp: true, confirmIdentityChange: true });

  await page.getByRole("button", { name: "Deploy new server" }).click();
  await page.getByLabel("Public server URL").fill("https://relay.example.invalid");
  await page.getByRole("button", { name: "Generate bundle" }).click();
  await expect(page.getByLabel("Management token")).toHaveAttribute("type", "password");
  await expect(page.getByLabel("Vault key")).toHaveAttribute("type", "password");
  await expect(page.getByText("These values are shown once.")).toBeVisible();
});

test("remote bulk import previews multiple files and confirms selected rows", async ({ page }) => {
  await installTauriMock(page, { mode: "remote", locale: "en", populated: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await page.getByRole("button", { name: "Import", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "Import accounts" });
  await dialog.getByRole("button", { name: "Choose account files" }).click();
  const imported = dialog.getByLabel("Select Imported account for import");
  const secondImported = dialog.getByLabel("Select Second imported account for import");
  const existing = dialog.getByLabel("Select Existing account for import");
  await expect(imported).toBeChecked();
  await expect(secondImported).toBeChecked();
  await expect(existing).not.toBeChecked();
  await dialog.getByLabel("Add selected to pool after import").check();
  await dialog.getByRole("button", { name: "Import 2 account(s)" }).click();
  await expect(dialog).toBeHidden();

  const importCalls = await page.evaluate(() => {
    const calls = (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: { input?: { action?: { type?: string }; payload?: Record<string, unknown> } } }> }).__TAURI_TEST_INVOKES__;
    return {
      filePreviewCalls: calls.filter((call) => call.command === "preview_remote_account_import_files").length,
      actions: calls
        .filter((call) => call.command === "execute_remote_server_action")
        .map((call) => call.args.input),
    };
  });
  expect(importCalls.filePreviewCalls).toBe(1);
  expect(importCalls.actions).toEqual([
    {
      action: { type: "confirm_account_batch_import" },
      payload: {
        sessionId: "remote_import",
        selectedItemIds: ["import_0123456789abcdef", "import_1111222233334444"],
        probeMetadata: true,
        addToPool: true,
      },
    },
  ]);
});

test("remote server-side usage filters and clear logs use managed commands", async ({ page }) => {
  await installTauriMock(page, { mode: "remote", locale: "en", populated: true });
  await page.goto("/");

  await page.getByRole("button", { name: "API & ChatGPT", exact: true }).click();
  await expect(page.locator(".gateway-runtime-panel")).toContainText("API is running");
  await expect(page.getByText("https://relay.example.invalid/v1")).toHaveCount(0);

  await page.getByRole("button", { name: "Usage", exact: true }).click();
  await chooseOption(page, page, "Model", "gpt-5.4");
  await chooseOption(page, page, "Pool member", "a1b2c3d4e5f6");
  await page.getByRole("button", { name: "More filters" }).click();
  await expect(page.getByLabel("Local key")).toHaveCount(0);
  await expect(page.getByRole("button", { name: /^Error category:/ })).toBeVisible();
  await page.getByRole("textbox", { name: "Request ID" }).fill("req_synthetic_remote");
  await expect(page.getByText("req_synthetic_remote")).toBeVisible();
  await expect.poll(() => page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: { input?: { modelQuery?: string; sourceOrAccountQuery?: string; requestIdQuery?: string } } }> }).__TAURI_TEST_INVOKES__.some((call) => call.command === "get_remote_server_usage" && call.args.input?.modelQuery === "gpt-5.4" && call.args.input?.sourceOrAccountQuery === "a1b2c3d4e5f6" && call.args.input?.requestIdQuery === "req_synthetic_remote"))).toBe(true);
  await page.getByLabel("Actions").click();
  await page.getByRole("menuitem", { name: "Clear logs" }).click();
  await settleConfirmation(page);
  await expect(page.getByText("Request logs cleared.")).toBeVisible();

  const calls = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: Record<string, unknown> }> }).__TAURI_TEST_INVOKES__);
  expect(calls.some((call) => call.command === "get_remote_server_state")).toBe(true);
  expect(calls.findLast((call) => call.command === "get_remote_server_usage" && (call.args.input as { modelQuery?: string } | undefined)?.modelQuery === "gpt-5.4")?.args.input).toMatchObject({ modelQuery: "gpt-5.4", sourceOrAccountQuery: "a1b2c3d4e5f6", requestIdQuery: "req_synthetic_remote" });
  expect(calls.findLast((call) => call.command === "execute_remote_server_action")?.args).toMatchObject({ input: { action: { type: "clear_usage" } } });
});

test("remote account economics uses the server usage identity without exposing its hash", async ({ page }) => {
  await installTauriMock(page, { mode: "remote", locale: "en", populated: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Usage", exact: true }).click();
  await chooseOption(page, page, "Account", "account_synthetic");

  await expect(page.locator(".usage-account-economics")).toContainText("Personal Plus");
  await expect.poll(() => page.evaluate(() => {
    const calls = (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: { input?: { sourceOrAccountQuery?: string } } }> }).__TAURI_TEST_INVOKES__;
    return calls.findLast((call) => call.command === "get_remote_server_usage")?.args.input?.sourceOrAccountQuery;
  })).toBe("account_synthetic");
  await expect(page.getByText("a1b2c3d4e5f6", { exact: true })).toHaveCount(0);
});

test("remote ChatGPT setup stays behind the managed profile command", async ({ page }) => {
  await installTauriMock(page, { mode: "remote", locale: "en", populated: true });
  await page.goto("/");
  await page.getByRole("button", { name: "API & ChatGPT", exact: true }).click();

  await page.getByRole("button", { name: "Connect ChatGPT", exact: true }).click();
  await expect.poll(() => page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string }> }).__TAURI_TEST_INVOKES__.some((call) => call.command === "attach_codex_to_remote_gateway"))).toBe(true);
});

test("remote capability omissions disable or hide unsupported operations", async ({ page }) => {
  await installTauriMock(page, { mode: "remote", locale: "en", populated: true, remoteFeatures: ["accounts"] });
  await page.goto("/");

  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await expect(page.getByRole("button", { name: "Import", exact: true })).toHaveCount(1);
  await page.locator(".account-bulk-menu summary").click();
  await expect(page.getByRole("menuitem", { name: "Assign proxies" })).toBeDisabled();
  await expect(page.getByRole("menuitem", { name: "Export all" })).toBeDisabled();
  await page.locator(".account-bulk-menu summary").click();
  await page.locator(".account-card .account-row-menu summary").click();
  await expect(page.getByRole("menuitem", { name: "Export" })).toBeDisabled();
  await expect(page.getByRole("button", { name: "Show all account identities" })).toHaveCount(0);

  await page.getByRole("button", { name: "API & ChatGPT", exact: true }).click();
  await expect(page.getByRole("button", { name: "Connect ChatGPT", exact: true })).toBeDisabled();
  await expect(page.locator(".proxy-settings")).toHaveCount(0);
  await page.locator(".relay-page-actions .relay-action-menu summary").click();
  await expect(page.getByRole("menuitem", { name: "Restart API" })).toBeDisabled();

  await page.getByRole("button", { name: "Pool", exact: true }).click();
  await expect(page.getByRole("tab", { name: "Client Access" })).toHaveCount(0);
  await expect(page.getByRole("tab", { name: "Model Rules" })).toHaveCount(0);

  await page.getByRole("button", { name: "Usage", exact: true }).click();
  await expect(page.getByText("The connected server does not support this action.")).toBeVisible();
});

test("remote server keeps capability refresh in the page header only", async ({ page }) => {
  await installTauriMock(page, { locale: "en", mode: "remote", theme: "light", populated: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await page.getByRole("tab", { name: "Remote Server" }).click();
  await expect(page.getByRole("button", { name: "Refresh capabilities" })).toHaveCount(1);
});

test("overview presents time-based usage analytics for the local relay", async ({ page }) => {
  await installTauriMock(page, { locale: "en", mode: "local", theme: "light", populated: true });
  await page.goto("/");
  await expect(page.getByRole("heading", { name: "Usage over time" })).toBeVisible();
  await expect(page.getByText("Token usage", { exact: true })).toBeVisible();
  await expect(page.getByText("API equivalent", { exact: true })).toBeVisible();
  await expect(page.locator(".overview-chart.cost .overview-chart-summary")).toHaveText("≈$0.000148");
  await expect(page.getByText("Response time", { exact: true })).toBeVisible();
  await expect(page.getByText("Generation speed", { exact: true })).toBeVisible();
  await expect(page.getByText("Runtime", { exact: true })).toHaveCount(0);
  await expect(page.getByText("Connections and capacity", { exact: true })).toHaveCount(0);
  await page.getByRole("tab", { name: "Week" }).click();
  await expect(page.getByRole("tab", { name: "Week" })).toHaveAttribute("aria-selected", "true");
});

test("overview asks the runtime for one aggregated series per selected period", async ({ page }) => {
  await installTauriMock(page, { locale: "en", mode: "local", theme: "light", populated: true });
  await page.goto("/");
  await page.getByRole("tab", { name: "Month" }).click();
  await expect.poll(() => page.evaluate(() => {
    const calls = (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: { input?: { range?: string; bucketMs?: number; fromMs?: number; toMs?: number } } }> }).__TAURI_TEST_INVOKES__;
    return calls.findLast((call) => call.command === "get_local_usage_page" && call.args.input?.bucketMs === 86_400_000)?.args.input;
  })).toMatchObject({ range: "custom", bucketMs: 86_400_000 });
});
