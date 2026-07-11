import { expect, test } from "@playwright/test";
import { installTauriMock } from "./tauri-mock";

test("local commands are reachable from the operational UI", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await page.getByRole("tab", { name: "Sources" }).click();
  await page.getByRole("button", { name: "Edit" }).click();
  await expect(page.getByRole("dialog", { name: "Edit source" })).toBeVisible();
  await page.getByRole("button", { name: "Cancel" }).click();
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

  await page.getByRole("button", { name: "Pool", exact: true }).click();
  await page.getByRole("button", { name: "Personal Plus" }).click();
  await page.getByRole("button", { name: "Save policy" }).click();
  await expect(page.getByText("Saved.")).toBeVisible();
  await page.getByRole("tab", { name: "Keys" }).click();
  await page.getByRole("button", { name: "Edit policy" }).click();
  await expect(page.getByRole("dialog", { name: "Edit policy" })).toBeVisible();
  await page.getByRole("button", { name: "Cancel" }).click();
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
  await page.getByRole("button", { name: "Export", exact: true }).click();

  await page.getByRole("button", { name: "Profiles", exact: true }).click();
  await expect(page.getByRole("row").filter({ hasText: "OpenCode" })).toBeVisible();
  await page.getByRole("tab", { name: "Backups" }).click();
  await page.getByRole("button", { name: "Open folder" }).click();

  await page.getByRole("button", { name: "Settings", exact: true }).click();
  await page.getByRole("button", { name: "Storage", exact: true }).click();
  await page.getByRole("button", { name: "Open data folder" }).click();
  await page.getByRole("button", { name: "Recovery", exact: true }).click();
  page.once("dialog", (dialog) => dialog.dismiss());
  await page.getByRole("button", { name: "Reset local pool data" }).click();

  const commands = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string }> }).__TAURI_TEST_INVOKES__.map((call) => call.command));
  expect(commands).toEqual(expect.arrayContaining(["export_usage", "export_support_bundle", "open_relay_folder"]));
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
