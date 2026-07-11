import { expect, test } from "@playwright/test";
import { installTauriMock } from "./tauri-mock";

test("local commands are reachable from the operational UI", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await page.getByRole("tab", { name: "Sources" }).click();
  await page.getByRole("button", { name: "Edit" }).click();
  const sourceDialog = page.getByRole("dialog", { name: "Edit source" });
  await sourceDialog.getByLabel("Protocol").selectOption("chat_completions");
  await sourceDialog.getByLabel("Models", { exact: true }).fill("gpt-5.4-mini, gpt-5.4");
  await sourceDialog.getByLabel("Allowed models", { exact: true }).fill("gpt-5.4-mini");
  await sourceDialog.getByLabel("Excluded models", { exact: true }).fill("gpt-5.4");
  await sourceDialog.getByRole("button", { name: "Save" }).click();
  await page.getByRole("row").filter({ hasText: "Example compatible API" }).getByRole("button", { name: "Test" }).click();
  await page.getByRole("tab", { name: "Accounts" }).click();
  await page.getByRole("button", { name: "Sign in" }).first().click();
  await page.getByRole("button", { name: "Open sign-in" }).click();
  await expect(page.getByLabel(/Callback URL/)).toBeVisible();
  await page.getByRole("button", { name: "Cancel" }).click();

  await page.getByRole("button", { name: "Import", exact: true }).click();
  const importDialog = page.getByRole("dialog", { name: "Import account" });
  await importDialog.getByLabel("Account import data").fill('{"accounts":[]}');
  await importDialog.getByRole("button", { name: "Preview import" }).click();
  const imported = importDialog.getByLabel("Select Imported account for import");
  const existing = importDialog.getByLabel("Select Existing account for import");
  await expect(imported).toBeChecked();
  await expect(existing).not.toBeChecked();
  await imported.uncheck();
  await expect(importDialog.getByRole("button", { name: "Import 0 account(s)" })).toBeDisabled();
  await existing.check();
  await importDialog.getByRole("button", { name: "Import 1 account(s)" }).click();
  await expect(importDialog).toBeHidden();
  const confirmedIds = await page.evaluate(() => {
    const calls = (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: { input?: { selectedItemIds?: string[] } } }> }).__TAURI_TEST_INVOKES__;
    return calls.findLast((call) => call.command === "confirm_local_account_import")?.args.input?.selectedItemIds;
  });
  expect(confirmedIds).toEqual(["import_fedcba9876543210"]);

  await page.getByRole("tab", { name: "Automations" }).click();
  await page.getByRole("button", { name: "Edit" }).click();
  const automation = page.getByRole("dialog", { name: "Edit automation" });
  await automation.getByLabel("Secondary").uncheck();
  await automation.getByLabel("Account selection").selectOption("account_ids");
  await automation.getByLabel("Personal Plus").check();
  await automation.getByLabel("Model policy").selectOption("explicit");
  await automation.getByRole("combobox", { name: "Model", exact: true }).selectOption("gpt-5.4-mini");
  await automation.getByRole("button", { name: "Save" }).click();
  const automationRow = page.getByRole("row").filter({ hasText: "Start quota countdown" });
  await expect(automationRow).toContainText("Personal Plus");
  await expect(automationRow).toContainText("Primary");
  await expect(automationRow).not.toContainText("Secondary");
  await expect(automationRow).toContainText("gpt-5.4-mini");
  await automationRow.getByRole("button", { name: "Test" }).click();

  await page.getByRole("button", { name: "Pool", exact: true }).click();
  await page.getByRole("button", { name: "Personal Plus" }).click();
  await page.getByLabel("Drain").check();
  await page.getByLabel("Allowed models", { exact: true }).fill("gpt-5.4-mini");
  await page.getByRole("button", { name: "Save policy" }).click();
  await expect(page.getByText("Saved.")).toBeVisible();
  await page.getByRole("tab", { name: "Keys" }).click();
  await page.getByRole("button", { name: "Edit policy" }).click();
  const keyPolicy = page.getByRole("dialog", { name: "Edit policy" });
  await keyPolicy.getByLabel("Model prefix").fill("team");
  await keyPolicy.getByRole("button", { name: "Save" }).click();
  page.once("dialog", (dialog) => dialog.accept());
  await page.getByRole("button", { name: "Rotate" }).click();
  await expect(page.getByText("zlr_synthetic_rotated_key")).toBeVisible();

  await page.getByRole("button", { name: "Gateway", exact: true }).click();
  await page.getByRole("button", { name: "Restart endpoint" }).click();
  await page.getByRole("spinbutton", { name: "Port" }).fill("15001");
  await page.getByRole("button", { name: "Apply and restart" }).click();
  await expect(page.getByText("http://127.0.0.1:15001/v1")).toBeVisible();
  await page.getByRole("tab", { name: "Client Setup" }).click();
  await page.getByRole("button", { name: "Attach current endpoint" }).click();
  await expect(page.getByText(/backup was preserved/i)).toBeVisible();
  await page.getByRole("button", { name: "OpenCode" }).click();
  await page.getByRole("button", { name: "Attach current endpoint" }).click();
  await page.getByRole("tab", { name: "Diagnostics" }).click();
  await page.locator(".diagnostics-list > section").filter({ hasText: "Endpoint health" }).getByRole("button", { name: "Run" }).click();
  await expect(page.getByText(/gpt-5.4-mini completed in 321 ms/)).toBeVisible();
  await page.locator(".diagnostics-list > section").filter({ hasText: "Streaming test" }).getByRole("button", { name: "Run" }).click();
  const diagnosticCalls = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: { stream?: boolean } }> }).__TAURI_TEST_INVOKES__.filter((call) => call.command === "diagnose_local_gateway").map((call) => call.args.stream));
  expect(diagnosticCalls).toEqual([false, true]);
  const gatewayCalls = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: { port?: number } }> }).__TAURI_TEST_INVOKES__.filter((call) => call.command === "restart_local_gateway" || call.command === "update_local_gateway_port"));
  expect(gatewayCalls).toEqual([{ command: "restart_local_gateway", args: {} }, { command: "update_local_gateway_port", args: { port: 15001 } }]);
  const profileCalls = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string }> }).__TAURI_TEST_INVOKES__.filter((call) => call.command === "attach_codex_to_local_gateway" || call.command === "attach_opencode_to_local_gateway").map((call) => call.command));
  expect(profileCalls).toEqual(["attach_codex_to_local_gateway", "attach_opencode_to_local_gateway"]);
  const policyCalls = await page.evaluate(() => {
    const calls = (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: Record<string, unknown> }> }).__TAURI_TEST_INVOKES__;
    return Object.fromEntries(calls
      .filter((call) => ["update_local_source", "test_quota_wake_automation", "update_local_account", "update_local_gateway_key"].includes(call.command))
      .map((call) => [call.command, call.args]));
  });
  expect(policyCalls.update_local_source).toMatchObject({ input: { wireApi: "chat_completions", models: ["gpt-5.4-mini", "gpt-5.4"], allowedModels: ["gpt-5.4-mini"], excludedModels: ["gpt-5.4"] } });
  expect(policyCalls.test_quota_wake_automation).toEqual({ taskId: "wake_synthetic" });
  expect(policyCalls.update_local_account).toMatchObject({ input: { draining: true, allowedModels: ["gpt-5.4-mini"] } });
  expect(policyCalls.update_local_gateway_key).toMatchObject({ input: { modelPrefix: "team" } });
});

test("import failures remain actionable without reusing a consumed session", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, importResult: "item_failure" });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await page.getByRole("button", { name: "Import", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "Import account" });
  await dialog.getByLabel("Account import data").fill('{"accounts":[]}');
  await dialog.getByRole("button", { name: "Preview import" }).click();
  await dialog.getByRole("button", { name: "Import 1 account(s)" }).click();
  await expect(dialog.getByRole("alert")).toContainText("Some accounts were not imported");
  await expect(dialog.getByText("item_not_found")).toBeVisible();
  await dialog.getByRole("button", { name: "Close" }).last().click();
  const canceled = await page.evaluate(() => {
    const calls = (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string }> }).__TAURI_TEST_INVOKES__;
    return calls.some((call) => call.command === "cancel_local_account_import");
  });
  expect(canceled).toBe(false);
});

test("missing import session keeps the dialog open with recovery guidance", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, importResult: "not_found" });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await page.getByRole("button", { name: "Import", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "Import account" });
  await dialog.getByLabel("Account import data").fill('{"accounts":[]}');
  await dialog.getByRole("button", { name: "Preview import" }).click();
  await dialog.getByRole("button", { name: "Import 1 account(s)" }).click();
  await expect(dialog.getByRole("alert")).toContainText("Start a fresh preview");
  await expect(dialog.getByLabel("Resume import session ID")).toHaveValue("11111111-2222-4333-8444-555555555555");
});

test("Ready API top-up uses the stored-key backend command", async ({ page }) => {
  await installTauriMock(page, { mode: "zenith", locale: "en", populated: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await page.locator(".relay-page-header").getByRole("button", { name: "Top up" }).click();
  await expect(page.getByRole("dialog", { name: "Top up" })).toBeVisible();
  await page.getByLabel("Top-up amount, USD").fill("10");
  await page.getByRole("button", { name: "Open top-up" }).click();
  await expect(page.getByText("Top-up opened in Telegram.")).toBeVisible();
});

test("recovery and export controls call the Rust-owned operations", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true });
  await page.goto("/");

  await page.getByRole("button", { name: "Usage", exact: true }).click();
  await page.getByRole("button", { name: "Export", exact: true }).click();
  await expect(page.getByText("Redacted export created.")).toBeVisible();

  await page.getByRole("button", { name: "Gateway", exact: true }).click();
  await page.getByRole("tab", { name: "Diagnostics" }).click();
  await page.getByRole("button", { name: "Preview", exact: true }).click();
  await expect(page.getByRole("dialog", { name: "Support bundle preview" })).toContainText("Raw account identities");
  await page.getByRole("button", { name: "Export", exact: true }).click();

  await page.getByRole("button", { name: "Profiles", exact: true }).click();
  await expect(page.getByRole("row").filter({ hasText: "OpenCode" })).toBeVisible();
  await page.getByRole("tab", { name: "Backups" }).click();
  await page.getByRole("button", { name: "Open folder" }).click();
  await page.getByRole("tab", { name: "Repair" }).click();
  await page.getByRole("button", { name: "Preview repair" }).click();
  await expect(page.getByText("2", { exact: true }).first()).toBeVisible();
  page.once("dialog", (dialog) => dialog.accept());
  await page.getByRole("button", { name: "Apply repair" }).click();
  await expect(page.getByText("History repaired")).toBeVisible();
  page.once("dialog", (dialog) => dialog.accept());
  await page.getByRole("button", { name: "Roll back repair" }).click();

  await page.getByRole("button", { name: "Settings", exact: true }).click();
  await page.getByRole("button", { name: "Storage", exact: true }).click();
  await page.getByRole("button", { name: "Open data folder" }).click();
  await page.getByRole("button", { name: "Recovery", exact: true }).click();
  page.once("dialog", (dialog) => dialog.dismiss());
  await page.getByRole("button", { name: "Reset local pool data" }).click();

  const commands = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string }> }).__TAURI_TEST_INVOKES__.map((call) => call.command));
  expect(commands).toEqual(expect.arrayContaining(["export_usage", "preview_support_bundle", "export_support_bundle", "open_relay_folder", "preview_codex_history_repair", "apply_codex_history_repair", "rollback_codex_history_repair"]));
  expect(commands).not.toContain("reset_local_pool_data");
});

test("connection search and request ID filters change visible rows", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true });
  await page.goto("/");

  await page.getByRole("button", { name: "Connections", exact: true }).click();
  const connectionSearch = page.getByPlaceholder("Search");
  await connectionSearch.fill("no such account");
  await expect(page.getByText("No matching results")).toBeVisible();
  await connectionSearch.fill("Personal Plus");
  await expect(page.getByText("Personal Plus")).toBeVisible();

  await page.getByRole("button", { name: "Usage", exact: true }).click();
  const requestFilter = page.getByRole("textbox", { name: "Request ID" });
  await requestFilter.fill("missing-request");
  await expect(page.getByText("No matching results")).toBeVisible();
  await requestFilter.fill("req_synthetic_local");
  await expect(page.getByText("req_synthetic_local")).toBeVisible();
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
  await page.getByLabel("Allow this unencrypted HTTP connection.").check();
  await page.getByLabel("Trust this server if its saved identity has changed.").check();
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

test("remote bulk import previews portable content and confirms selected rows", async ({ page }) => {
  await installTauriMock(page, { mode: "remote", locale: "en", populated: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await page.getByRole("button", { name: "Import", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "Import account" });
  const content = JSON.stringify({ version: 1, accounts: [{ name: "Portable account" }] });
  await dialog.getByLabel("Account import data").fill(content);
  await dialog.getByRole("button", { name: "Preview import" }).click();
  const imported = dialog.getByLabel("Select Imported account for import");
  const existing = dialog.getByLabel("Select Existing account for import");
  await expect(imported).toBeChecked();
  await expect(existing).not.toBeChecked();
  await imported.uncheck();
  await existing.check();
  await dialog.getByRole("button", { name: "Import 1 account(s)" }).click();
  await expect(dialog).toBeHidden();
  await page.getByRole("tab", { name: "Sources" }).click();
  await page.getByRole("row").filter({ hasText: "Example compatible API" }).getByRole("button", { name: "Test" }).click();

  const actions = await page.evaluate(() => {
    const calls = (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: { input?: { action?: { type?: string }; payload?: Record<string, unknown> } } }> }).__TAURI_TEST_INVOKES__;
    return calls
      .filter((call) => call.command === "execute_remote_server_action")
      .map((call) => call.args.input);
  });
  expect(actions).toEqual([
    { action: { type: "preview_account_batch_import" }, payload: { content } },
    {
      action: { type: "confirm_account_batch_import" },
      payload: { sessionId: "remote_import", selectedItemIds: ["import_fedcba9876543210"] },
    },
    { action: { type: "test_source", id: "source_synthetic" }, payload: null },
  ]);
});

test("remote diagnostics, server-side usage filters, and clear logs use managed commands", async ({ page }) => {
  await installTauriMock(page, { mode: "remote", locale: "en", populated: true });
  await page.goto("/");

  await page.getByRole("button", { name: "Gateway", exact: true }).click();
  await page.getByRole("tab", { name: "Diagnostics" }).click();
  await page.locator(".diagnostics-list > section").filter({ hasText: "Endpoint health" }).getByRole("button", { name: "Run" }).click();
  await page.locator(".diagnostics-list > section").filter({ hasText: "Streaming test" }).getByRole("button", { name: "Run" }).click();
  await expect(page.getByText(/gpt-5.4-mini completed in 345 ms/)).toBeVisible();
  const diagnosticCalls = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: { stream?: boolean } }> }).__TAURI_TEST_INVOKES__.filter((call) => call.command === "diagnose_remote_gateway").map((call) => call.args.stream));
  expect(diagnosticCalls).toEqual([false, true]);

  await page.reload();
  await expect(page.getByText("https://relay.example.invalid/v1").first()).toBeVisible();

  await page.getByRole("button", { name: "Usage", exact: true }).click();
  await page.getByRole("textbox", { name: "Model" }).fill("gpt-5.4");
  await page.getByRole("textbox", { name: "Request ID" }).fill("req_synthetic_remote");
  await expect(page.getByText("req_synthetic_remote")).toBeVisible();
  await expect.poll(() => page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: { input?: { modelQuery?: string; requestIdQuery?: string } } }> }).__TAURI_TEST_INVOKES__.some((call) => call.command === "get_remote_server_usage" && call.args.input?.modelQuery === "gpt-5.4" && call.args.input?.requestIdQuery === "req_synthetic_remote"))).toBe(true);
  await page.getByLabel("Actions").click();
  page.once("dialog", (dialog) => dialog.accept());
  await page.getByRole("menuitem", { name: "Clear logs" }).click();
  await expect(page.getByText("Request logs cleared.")).toBeVisible();

  const calls = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: Record<string, unknown> }> }).__TAURI_TEST_INVOKES__);
  expect(calls.some((call) => call.command === "get_remote_server_state")).toBe(true);
  expect(calls.find((call) => call.command === "get_remote_server_usage" && (call.args.input as { modelQuery?: string } | undefined)?.modelQuery === "gpt-5.4")?.args.input).toMatchObject({ modelQuery: "gpt-5.4", requestIdQuery: "req_synthetic_remote" });
  expect(calls.findLast((call) => call.command === "execute_remote_server_action")?.args).toMatchObject({ input: { action: { type: "clear_usage" } } });
});

test("remote capability omissions disable or hide unsupported operations", async ({ page }) => {
  await installTauriMock(page, { mode: "remote", locale: "en", populated: true, remoteFeatures: ["accounts"] });
  await page.goto("/");

  await page.getByRole("button", { name: "Gateway", exact: true }).click();
  await expect(page.getByRole("button", { name: "Restart endpoint" })).toBeDisabled();
  await page.getByRole("tab", { name: "Diagnostics" }).click();
  await expect(page.locator(".diagnostics-list > section").filter({ hasText: "Streaming test" }).getByRole("button", { name: "Run" })).toBeDisabled();

  await page.getByRole("button", { name: "Pool", exact: true }).click();
  await expect(page.getByRole("tab", { name: "Keys" })).toHaveCount(0);
  await expect(page.getByRole("tab", { name: "Model Rules" })).toHaveCount(0);

  await page.getByRole("button", { name: "Usage", exact: true }).click();
  await expect(page.getByText("The connected server does not support this action.")).toBeVisible();
});
