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
  const importDialog = page.getByRole("dialog", { name: "Import accounts" });
  await importDialog.getByRole("button", { name: "Choose JSON files" }).click();
  const imported = importDialog.getByLabel("Select Imported account for import");
  const secondImported = importDialog.getByLabel("Select Second imported account for import");
  const existing = importDialog.getByLabel("Select Existing account for import");
  await expect(imported).toBeChecked();
  await expect(secondImported).toBeChecked();
  await expect(existing).not.toBeChecked();
  await importDialog.getByRole("button", { name: "Import 2 account(s)" }).click();
  await expect(importDialog).toBeHidden();
  const importCalls = await page.evaluate(() => {
    const calls = (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: { input?: { selectedItemIds?: string[] } } }> }).__TAURI_TEST_INVOKES__;
    return {
      filePreviewCalls: calls.filter((call) => call.command === "preview_local_account_import_files").length,
      selected: calls.findLast((call) => call.command === "confirm_local_account_import")?.args.input?.selectedItemIds,
    };
  });
  expect(importCalls.filePreviewCalls).toBe(1);
  expect(importCalls.selected).toEqual([
    "import_0123456789abcdef",
    "import_1111222233334444",
  ]);

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
  await page.getByRole("button", { name: "Pool member policy: Personal Plus" }).click();
  await page.getByLabel("Drain").check();
  await page.getByLabel("Allowed models", { exact: true }).fill("gpt-5.4-mini");
  await page.getByRole("button", { name: "Save policy" }).click();
  await expect(page.getByText("Saved.")).toBeVisible();
  await page.getByRole("tab", { name: "Keys" }).click();
  const keyRow = page.getByRole("row").filter({ has: page.getByRole("button", { name: "Edit policy" }) });
  await keyRow.getByRole("button", { name: "Edit policy" }).click();
  const keyPolicy = page.getByRole("dialog", { name: "Edit policy" });
  await keyPolicy.getByLabel("Model prefix").fill("team");
  await keyPolicy.getByRole("button", { name: "Save" }).click();
  await keyRow.locator(".relay-action-menu summary").click();
  page.once("dialog", (dialog) => dialog.accept());
  await page.getByRole("menuitem", { name: "Rotate" }).click();
  await expect(page.getByText("zlr_synthetic_rotated_key")).toBeVisible();

  await page.getByRole("button", { name: "Gateway", exact: true }).click();
  await page.getByRole("button", { name: "Restart endpoint" }).click();
  await page.getByRole("spinbutton", { name: "Port" }).fill("15001");
  await page.getByRole("button", { name: "Apply and restart" }).click();
  await expect(page.getByText("http://127.0.0.1:15001/v1")).toBeVisible();
  await page.getByRole("tab", { name: "Codex Setup" }).click();
  await expect(page.getByLabel("Codex interface account")).toHaveValue("auto");
  await expect(page.getByRole("heading", { name: "Codex in pool mode" })).toBeVisible();
  await page.getByRole("tab", { name: "Diagnostics" }).click();
  await page.locator(".diagnostics-list > section").filter({ hasText: "Endpoint health" }).getByRole("button", { name: "Run" }).click();
  await expect(page.getByText(/gpt-5.4-mini completed in 321 ms/)).toBeVisible();
  await page.locator(".diagnostics-list > section").filter({ hasText: "Streaming test" }).getByRole("button", { name: "Run" }).click();
  const diagnosticCalls = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: { stream?: boolean } }> }).__TAURI_TEST_INVOKES__.filter((call) => call.command === "diagnose_local_gateway").map((call) => call.args.stream));
  expect(diagnosticCalls).toEqual([false, true]);
  const gatewayCalls = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: { port?: number } }> }).__TAURI_TEST_INVOKES__.filter((call) => call.command === "restart_local_gateway" || call.command === "update_local_gateway_port"));
  expect(gatewayCalls).toEqual([{ command: "restart_local_gateway", args: {} }, { command: "update_local_gateway_port", args: { port: 15001 } }]);
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

test("pasted Cockpit arrays reach the Rust batch preview unchanged", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await page.getByRole("button", { name: "Import", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "Import accounts" });
  const payload = JSON.stringify([
    { type: "codex", access_token: "synthetic-access-one", account_id: "synthetic-one", email: "one@example.test" },
    { type: "codex", access_token: "synthetic-access-two", account_id: "synthetic-two", email: "two@example.test" },
    { auth_mode: "apikey", OPENAI_API_KEY: "synthetic-api-key", api_base_url: "https://api.example.test/v1", api_provider_name: "Example API" },
  ]);
  await dialog.getByLabel("Account import JSON").fill(payload);
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

test("dropping JSON files anywhere opens the shared import preview", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true });
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
  await expect(page.getByText("Drop JSON files to preview accounts")).toBeVisible();
  await page.evaluate((droppedPaths) => {
    const emit = (window as unknown as { __TAURI_TEST_EMIT__: (event: string, payload: unknown) => void }).__TAURI_TEST_EMIT__;
    emit("tauri://drag-drop", { paths: droppedPaths, position: { x: 200, y: 160 } });
  }, paths);

  const dialog = page.getByRole("dialog", { name: "Import accounts" });
  await expect(dialog.getByLabel("Select Imported account for import")).toBeChecked();
  await expect(dialog.getByLabel("Select Second imported account for import")).toBeChecked();
  await expect(page.getByText("Drop JSON files to preview accounts")).toBeHidden();
  const call = await page.evaluate(() => {
    const calls = (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: { paths?: string[] } }> }).__TAURI_TEST_INVOKES__;
    return calls.findLast((item) => item.command === "preview_local_account_import_files");
  });
  expect(call?.args.paths).toEqual(paths);
});

for (const scenario of [
  { mode: "local", locale: "en", code: "provider_account_id_missing", nav: "Connections", action: "Import", title: "Import accounts", input: "Account import JSON", preview: "Preview import", confirm: "Import 2 account(s)", heading: "Some accounts were not imported", reason: "The imported record and its token claims do not contain a ChatGPT account ID.", close: "Close" },
  { mode: "local", locale: "ru", code: "models_http_status", nav: "Подключения", action: "Импорт", title: "Импортировать учётные записи", input: "JSON для импорта учётных записей", preview: "Проверить импорт", confirm: "Импортировать: 2", heading: "Часть учётных записей не импортирована", reason: "При проверке доступных моделей провайдер вернул неожиданный ответ.", close: "Закрыть" },
  { mode: "remote", locale: "en", code: "models_forbidden", nav: "Connections", action: "Import", title: "Import accounts", input: "Account import JSON", preview: "Preview import", confirm: "Import 2 account(s)", heading: "Some accounts were not imported", reason: "The provider denied access to the model list. Check this account's access and proxy region.", close: "Close" },
  { mode: "remote", locale: "ru", code: "item_not_found", nav: "Подключения", action: "Импорт", title: "Импортировать учётные записи", input: "JSON для импорта учётных записей", preview: "Проверить импорт", confirm: "Импортировать: 2", heading: "Часть учётных записей не импортирована", reason: "Не пройдена финальная проверка аккаунта. Обновите его данные или прокси и повторите импорт.", close: "Закрыть" },
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
      expect(canceled).toBe(false);
    }
  });
}

test("missing import session keeps the dialog open with recovery guidance", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, importResult: "not_found" });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await page.getByRole("button", { name: "Import", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "Import accounts" });
  await dialog.getByLabel("Account import JSON").fill('{"accounts":[]}');
  await dialog.getByRole("button", { name: "Preview import" }).click();
  await dialog.getByRole("button", { name: "Import 2 account(s)" }).click();
  await expect(dialog.getByRole("alert")).toContainText("Start a fresh preview");
  await expect(dialog.getByLabel("Resume import session ID")).toHaveValue("11111111-2222-4333-8444-555555555555");
  await expect(page.locator(".global-feedback.error")).toBeVisible();
  const layers = await page.evaluate(() => ({
    feedback: Number.parseInt(getComputedStyle(document.querySelector(".global-feedback")!).zIndex, 10),
    modal: Number.parseInt(getComputedStyle(document.querySelector(".relay-modal-backdrop")!).zIndex, 10),
  }));
  expect(layers.feedback).toBeGreaterThan(layers.modal);
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
  const usageExport = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: { rows?: Array<{ reasoningTokens?: number }> } }> }).__TAURI_TEST_INVOKES__.findLast((call) => call.command === "export_usage"));
  expect(usageExport?.args.rows?.[0]?.reasoningTokens).toBe(5);

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
  await expect(page.getByText("Personal Plus", { exact: true })).toBeVisible();

  await page.getByRole("button", { name: "Usage", exact: true }).click();
  const requestFilter = page.getByRole("textbox", { name: "Request ID" });
  await requestFilter.fill("missing-request");
  await expect(page.getByText("No matching results")).toBeVisible();
  await requestFilter.fill("req_synthetic_local");
  await expect(page.getByText("req_synthetic_local")).toBeVisible();
});

test("account export supports bulk copy and per-account download", async ({ page, context }) => {
  await context.grantPermissions(["clipboard-read", "clipboard-write"], { origin: "http://127.0.0.1:1420" });
  await installTauriMock(page, { mode: "local", locale: "en", populated: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();

  await page.locator(".account-bulk-menu summary").click();
  await page.getByRole("menuitem", { name: "Export all" }).click();
  let dialog = page.getByRole("dialog", { name: "Export accounts" });
  await expect(dialog.getByRole("radio")).toHaveCount(7);
  await expect(dialog.getByRole("button", { name: "Copy JSON" })).toBeDisabled();
  await dialog.getByRole("radio", { name: "Codex", exact: true }).click();
  await dialog.getByLabel(/I understand that anyone/).check();
  await dialog.getByRole("button", { name: "Copy JSON" }).click();
  await expect(page.getByText("Account export copied.")).toBeVisible();
  await expect.poll(() => page.evaluate(() => navigator.clipboard.readText())).toContain("synthetic-export-token");

  await page.locator(".account-card .account-row-menu summary").click();
  await page.getByRole("menuitem", { name: "Export" }).click();
  dialog = page.getByRole("dialog", { name: "Export accounts" });
  await dialog.getByRole("radio", { name: "9router" }).click();
  await dialog.getByLabel(/I understand that anyone/).check();
  await dialog.getByRole("button", { name: "Download JSON" }).click();
  await expect(page.getByText("Account export saved.")).toBeVisible();

  const calls = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: Record<string, unknown> }> }).__TAURI_TEST_INVOKES__.filter((call) => call.command === "export_local_accounts"));
  expect(calls.map((call) => call.args.input)).toEqual([
    { accountIds: ["account_synthetic"], format: "codex", destination: "copy" },
    { accountIds: ["account_synthetic"], format: "9router", destination: "download" },
  ]);
});

test("frequent account actions stay in the row and secondary actions stay in the menu", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  const actions = page.locator(".account-card").first().locator(".account-row-action-list");
  expect(await actions.locator(":scope > *").evaluateAll((items) => items.map((item) => item.tagName === "DETAILS" ? item.querySelector("summary")?.getAttribute("aria-label") : item.getAttribute("aria-label")))).toEqual([
    "Actions",
    "Show full identity",
    "Refresh quota",
    "Launch in Codex",
  ]);
  await page.locator(".account-card .account-row-menu summary").click();
  const menu = page.getByRole("menu");
  await expect(menu.getByRole("menuitem", { name: "Refresh quota" })).toHaveCount(0);
  await expect(menu.getByRole("menuitem", { name: "Export" })).toBeVisible();
  await expect(menu.getByRole("menuitem")).toHaveCount(3);
  await expect(menu.getByRole("menuitem", { name: "Disable" })).toBeVisible();
  await expect(menu.getByRole("menuitem", { name: "Delete" })).toBeVisible();
  await page.locator(".account-card .account-row-menu summary").click();
  await page.getByRole("button", { name: "Refresh quota" }).click();
  const refreshCall = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: Record<string, unknown> }> }).__TAURI_TEST_INVOKES__.findLast((item) => item.command === "refresh_local_account_quota"));
  expect(refreshCall?.args).toEqual({ accountId: "account_synthetic" });
});

test("remote account quota refresh targets the selected server account", async ({ page }) => {
  await installTauriMock(page, { mode: "remote", locale: "en", populated: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await page.getByRole("button", { name: "Refresh quota" }).click();

  const refreshCall = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: { input?: unknown } }> }).__TAURI_TEST_INVOKES__.findLast((item) => item.command === "execute_remote_server_action"));
  expect(refreshCall?.args.input).toEqual({ action: { type: "refresh_account", id: "account_synthetic" }, payload: null });
});

test("plan filters keep failed accounts visible with typed errors", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, accountCount: 3 });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();

  const filters = page.getByRole("group", { name: "Filter by plan" });
  await expect(filters.getByRole("button", { name: "All (3)" })).toBeVisible();
  await expect(filters.getByRole("button", { name: "Plus (1)" })).toBeVisible();
  await expect(filters.getByRole("button", { name: "Business (1)" })).toBeVisible();
  await expect(filters.getByRole("button", { name: "Free (1)" })).toBeVisible();

  await filters.getByRole("button", { name: "Business (1)" }).click();
  await expect(page.locator(".account-card")).toHaveCount(1);
  await expect(page.locator(".account-card")).toContainText("Business Workspace");
  await expect(page.locator(".account-filter-summary")).toContainText("Showing 1 of 3 accounts");

  await filters.getByRole("button", { name: "Errors (1)" }).click();
  await expect(page.locator(".account-card")).toHaveCount(1);
  await expect(page.locator(".account-card")).toContainText("Backup account");
  await expect(page.locator(".account-error-line")).toContainText("Connection error");
  await expect(page.locator(".account-error-line code")).toHaveText("quota_transport");
  await page.locator(".account-error-line").click();
  const errorDialog = page.getByRole("dialog", { name: "Technical error details" });
  const errorJson = JSON.parse(await errorDialog.locator("pre").innerText()) as Record<string, unknown>;
  expect(errorJson).toMatchObject({ code: "quota_transport", message: "Connection error", account: "r***@example.test", health: "degraded", auth_state: "active", subscription_status: "active" });
  expect(errorJson.observed_at).toEqual(expect.any(String));
  await expect(errorDialog).not.toContainText("zrk_synthetic_ready_key");
  await errorDialog.locator("footer").getByRole("button", { name: "Close" }).click();
  await page.getByRole("button", { name: "Clear filters" }).click();
  await expect(page.locator(".account-card")).toHaveCount(3);
  await expect(page.getByText("Showing 1 of 3 accounts")).toHaveCount(0);
});

test("account sorting follows pool and quota window usage", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, accountCount: 3 });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  const labels = page.locator(".account-card .account-identity > strong");

  await page.getByLabel("Sort accounts").selectOption("primary");
  await expect(labels).toHaveText(["Backup account", "Business Workspace", "Personal Plus"]);
  await page.getByRole("button", { name: "Descending order" }).click();
  await expect(labels).toHaveText(["Personal Plus", "Business Workspace", "Backup account"]);

  await page.getByLabel("Sort accounts").selectOption("secondary");
  await expect(labels).toHaveText(["Personal Plus", "Backup account", "Business Workspace"]);
  await page.getByLabel("Sort accounts").selectOption("pool");
  await expect(labels).toHaveText(["Business Workspace", "Personal Plus", "Backup account"]);
});

test("account layouts switch between compact, detailed, and grid views", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, accountCount: 4 });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  const accounts = page.locator(".account-list");

  await expect(accounts).toHaveAttribute("data-layout", "list");
  await expect(accounts).toContainText("Pro account");
  await page.getByRole("button", { name: "Compact account view" }).click();
  await expect(accounts).toHaveAttribute("data-layout", "compact");
  await expect(accounts.locator(".account-card-quota").first()).toBeHidden();
  await page.getByRole("button", { name: "Account card grid" }).click();
  await expect(accounts).toHaveAttribute("data-layout", "grid");

  await page.reload();
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await expect(page.locator(".account-list")).toHaveAttribute("data-layout", "grid");
});

test("account cards show the subscription end date or an explicit unavailable state", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, accountCount: 3 });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  const cards = page.locator(".account-card");
  await expect(cards.filter({ hasText: "Personal Plus" }).locator(".account-subscription-line")).toContainText("Active until");
  await expect(cards.filter({ hasText: "Personal Plus" }).locator(".account-subscription-countdown")).toHaveText(/in \d+ days/);
  await expect(cards.filter({ hasText: "Business Workspace" }).locator(".account-subscription-line")).toContainText("Active until");
  await expect(cards.filter({ hasText: "Business Workspace" }).locator(".account-subscription-countdown")).toHaveText(/in \d+ days/);
  await expect(cards.filter({ hasText: "Backup account" }).locator(".account-subscription-line")).toHaveText("Subscription end date unavailable");
});

test("subscription countdown switches to a live clock in the final day", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "ru", populated: true, subscriptionExpiresInMs: 70_000 });
  await page.goto("/");
  await page.getByRole("button", { name: "Подключения", exact: true }).click();
  const countdown = page.locator(".account-subscription-countdown");
  await expect(countdown).toHaveText(/Осталось 00:01:\d{2}/);
  const initial = await countdown.textContent();
  await expect.poll(() => countdown.textContent()).not.toBe(initial);
});

test("plan filters and pool controls exclude a selected account without deleting it", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, accountCount: 3 });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await page.getByLabel("Sort accounts").selectOption("participation");
  await page.getByRole("button", { name: "Free (1)", exact: true }).click();
  await expect(page.locator(".account-card")).toHaveCount(1);
  await page.getByLabel("Select all accounts").check();
  await page.getByRole("button", { name: "Exclude selected", exact: true }).click();

  await page.getByRole("button", { name: "Excluded (1)", exact: true }).click();
  const card = page.locator(".account-card").filter({ hasText: "Backup account" });
  await expect(card).toBeVisible();
  const participation = card.getByRole("switch", { name: "Use Backup account in the pool" });
  await expect(participation).not.toBeChecked();
  await participation.click();
  await expect(card).toBeHidden();

  await expect.poll(() => page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string }> }).__TAURI_TEST_INVOKES__.filter((call) => call.command === "set_local_pool_membership").length)).toBe(2);
  const calls = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: Record<string, unknown> }> }).__TAURI_TEST_INVOKES__);
  expect(calls.filter((call) => call.command === "set_local_pool_membership").map((call) => call.args)).toEqual([
    { input: { accountIds: ["account_synthetic_3"], sourceIds: [], inPool: false } },
    { input: { accountIds: ["account_synthetic_3"], sourceIds: [], inPool: true } },
  ]);
  expect(calls.some((call) => call.command === "delete_local_account")).toBe(false);
});

test("pool summary keeps healthy and limited members mutually exclusive", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, accountCount: 3 });
  await page.goto("/");
  await page.getByRole("button", { name: "Pool", exact: true }).click();
  const summary = page.locator(".pool-summary");
  await expect(summary.locator("div")).toHaveCount(3);
  await expect(summary.locator("div").nth(0)).toHaveText("Healthy2");
  await expect(summary.locator("div").nth(1)).toHaveText("Limited2");
  await expect(summary.locator("div").nth(2)).toHaveText("Disabled0");
});

test("connections stay outside the pool until the user adds selected members", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, accountCount: 3, poolMembers: false, gatewayRunning: false });
  await page.goto("/");
  await page.getByRole("button", { name: "Pool", exact: true }).click();
  await expect(page.getByText("No pool members", { exact: true })).toBeVisible();
  await expect(page.getByRole("button", { name: "Start pool", exact: true })).toBeDisabled();

  await page.getByRole("button", { name: "Add member", exact: true }).first().click();
  const dialog = page.getByRole("dialog", { name: "Add connections to pool" });
  await dialog.getByText("Business Workspace", { exact: true }).click();
  await dialog.getByRole("button", { name: "Add selected (1)" }).click();

  const rows = page.locator(".pool-member-card");
  await expect(rows).toHaveCount(1);
  await expect(rows.first()).toContainText("Business Workspace");
  const call = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: Record<string, unknown> }> }).__TAURI_TEST_INVOKES__.findLast((item) => item.command === "set_local_pool_membership"));
  expect(call?.args).toEqual({ input: { accountIds: ["account_synthetic_2"], sourceIds: [], inPool: true } });
});

test("pool display order defaults to routing priority and can be changed to quota", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, accountCount: 3 });
  await page.goto("/");
  await page.getByRole("button", { name: "Pool", exact: true }).click();
  const order = page.getByLabel("Display order");
  await expect(order.locator("option")).toHaveText(["Routing order", "Available quota", "Name"]);
  await expect(page.locator(".pool-member-card").first().getByText("Priority", { exact: true })).toHaveCount(0);
  const names = () => page.locator(".pool-member-card").evaluateAll((items) => items.map((item) => item.getAttribute("data-member-label") ?? ""));
  expect(await names()).toEqual(["Business Workspace", "Personal Plus", "Example compatible API", "Backup account"]);
  await order.selectOption("quota");
  expect(await names()).toEqual(["Backup account", "Business Workspace", "Personal Plus", "Example compatible API"]);
  await order.selectOption("name");
  expect(await names()).toEqual(["Backup account", "Business Workspace", "Example compatible API", "Personal Plus"]);
});

test("pool member picker lists individual accounts instead of subscription groups", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, accountCount: 4, poolMembers: false });
  await page.goto("/");
  await page.getByRole("button", { name: "Pool", exact: true }).click();
  await page.getByRole("button", { name: "Add member", exact: true }).first().click();
  const dialog = page.getByRole("dialog", { name: "Add connections to pool" });
  const accountRows = dialog.locator(".pool-member-picker section").first().locator(".pool-member-options > label");
  await expect(accountRows).toHaveCount(4);
  await expect(accountRows.locator("strong")).toHaveText(["p***@example.test", "q***@example.test", "b***@example.test", "r***@example.test"]);
  await expect(accountRows.locator("small")).toHaveText(["Personal Plus", "Pro account", "Business Workspace", "Backup account"]);
  await expect(accountRows.locator("em")).toHaveText(["Plus", "Pro", "Business", "Free"]);

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

test("pool member layouts switch between compact, detailed, and grid views", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, accountCount: 4 });
  await page.goto("/");
  await page.getByRole("button", { name: "Pool", exact: true }).click();
  const members = page.locator(".pool-member-list");
  await expect(members).toHaveAttribute("data-layout", "list");
  await page.getByRole("button", { name: "Compact pool view" }).click();
  await expect(members).toHaveAttribute("data-layout", "compact");
  await expect(members.locator(".pool-member-quota").first()).toBeHidden();
  await page.getByRole("button", { name: "Pool card grid" }).click();
  await expect(members).toHaveAttribute("data-layout", "grid");
  await page.reload();
  await page.getByRole("button", { name: "Pool", exact: true }).click();
  await expect(page.locator(".pool-member-list")).toHaveAttribute("data-layout", "grid");
});

test("local pool refreshes only pool quotas and saves bounded refresh settings", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, accountCount: 3, freeAccountHealthy: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Pool", exact: true }).click();

  const freeMember = page.locator('[data-member-label="Backup account"]');
  await expect(freeMember).toContainText("Excluded by Free policy");

  await page.getByRole("button", { name: "Refresh quotas", exact: true }).click();
  await expect(page.getByText("Updated.", { exact: true })).toBeVisible();
  await page.getByRole("button", { name: "Quota refresh settings", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "Quota refresh" });
  await dialog.getByLabel("Background refresh interval").selectOption("120");
  await dialog.getByLabel("Request timeout").selectOption("10");
  await dialog.getByLabel("Use Free accounts").check();
  await dialog.getByRole("button", { name: "Save", exact: true }).click();
  await expect(freeMember).toContainText("Ready");

  const calls = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: Record<string, unknown> }> }).__TAURI_TEST_INVOKES__);
  expect(calls.some((call) => call.command === "refresh_local_pool_account_quotas")).toBe(true);
  expect(calls.findLast((call) => call.command === "update_local_quota_policy")?.args).toEqual({ input: { refreshIntervalSeconds: 120, requestTimeoutSeconds: 10, useFreeAccounts: true } });
});

test("remote pool uses the same quota refresh controls", async ({ page }) => {
  await installTauriMock(page, { mode: "remote", locale: "en", populated: true, accountCount: 3, freeAccountHealthy: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Pool", exact: true }).click();

  await page.getByRole("button", { name: "Refresh quotas", exact: true }).click();
  await page.getByRole("button", { name: "Quota refresh settings", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "Quota refresh" });
  await dialog.getByLabel("Background refresh interval").selectOption("600");
  await dialog.getByLabel("Request timeout").selectOption("15");
  await dialog.getByLabel("Use Free accounts").check();
  await dialog.getByRole("button", { name: "Save", exact: true }).click();

  const actions = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: { input?: { action?: { type?: string }; payload?: unknown } } }> }).__TAURI_TEST_INVOKES__.filter((call) => call.command === "execute_remote_server_action").map((call) => call.args.input));
  expect(actions).toEqual(expect.arrayContaining([
    { action: { type: "refresh_pool_quotas" }, payload: null },
    { action: { type: "set_quota_policy" }, payload: { refreshIntervalSeconds: 600, requestTimeoutSeconds: 15, useFreeAccounts: true } },
  ]));
});

test("legacy remote servers keep the unsupported Free policy read-only", async ({ page }) => {
  await installTauriMock(page, {
    mode: "remote",
    locale: "en",
    populated: true,
    remoteFeatures: ["accounts", "quota"],
  });
  await page.goto("/");
  await page.getByRole("button", { name: "Pool", exact: true }).click();
  await page.getByRole("button", { name: "Quota refresh settings", exact: true }).click();
  await expect(page.getByRole("dialog").getByLabel("Use Free accounts")).toBeDisabled();
  await expect(page.getByRole("dialog")).toContainText("Update the server");
});

test("connections distinguish pool membership from Free-policy routing", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, accountCount: 3, freeAccountHealthy: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  const freeAccount = page.locator(".account-card").filter({ hasText: "Backup account" });
  await expect(freeAccount).toContainText("Not routed: Free policy");
  await expect(freeAccount).toContainText("95%");
  await expect(freeAccount).toContainText("30 days");
});

test("page navigation resets the shared content scroll position", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Gateway", exact: true }).click();
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
  });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();

  await expect(page.locator(".account-card")).toHaveCount(1);
  await expect(page.locator(".account-card")).toContainText("Personal Plus");
  await expect(page.locator(".account-error-line")).toContainText("Signed out or account changed");
  await expect(page.locator(".account-error-line code")).toHaveText("auth_invalid_grant");
});

test("source, automation, and key rows keep rare actions in consistent menus", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();

  await page.getByRole("tab", { name: "Sources" }).click();
  let actions = page.locator(".relay-table .row-actions");
  expect(await actions.locator(":scope > *").evaluateAll((items) => items.map((item) => item.tagName === "DETAILS" ? item.querySelector("summary")?.getAttribute("aria-label") : item.getAttribute("aria-label")))).toEqual(["Test", "Edit", "Actions"]);
  await actions.locator("summary").click();
  expect(await page.getByRole("menuitem").allTextContents()).toEqual(["Disable", "Delete"]);
  await page.keyboard.press("Escape");

  await page.getByRole("tab", { name: "Automations" }).click();
  actions = page.locator(".relay-table .row-actions");
  expect(await actions.locator(":scope > *").evaluateAll((items) => items.map((item) => item.tagName === "DETAILS" ? item.querySelector("summary")?.getAttribute("aria-label") : item.getAttribute("aria-label")))).toEqual(["Edit", "Test", "Actions"]);
  await actions.locator("summary").click();
  await expect(page.getByRole("menuitem")).toHaveText("Delete");

  await page.getByRole("button", { name: "Pool", exact: true }).click();
  await page.getByRole("tab", { name: "Keys" }).click();
  actions = page.locator(".relay-table .row-actions");
  expect(await actions.locator(":scope > *").evaluateAll((items) => items.map((item) => item.tagName === "DETAILS" ? item.querySelector("summary")?.getAttribute("aria-label") : item.getAttribute("aria-label")))).toEqual(["Edit policy", "Actions"]);
  await actions.locator("summary").click();
  expect(await page.getByRole("menuitem").allTextContents()).toEqual(["Disable", "Rotate key", "Delete"]);
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

test("Codex client setup offers no account, automatic, and manual pool identity modes", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, accountCount: 2, gatewayRunning: true, historyRepairChanges: false });
  await page.goto("/");
  await page.getByRole("button", { name: "Gateway", exact: true }).click();
  await page.getByRole("tab", { name: "Codex Setup" }).click();
  const setup = page.locator(".client-setup");
  const account = page.getByLabel("Codex interface account");
  await expect(account).toHaveValue("auto");
  await expect(account.locator("option")).toHaveText(["Without account", "Automatic selection", "Business Workspace · Business", "Personal Plus · Plus"]);
  await expect(account.locator("optgroup")).toHaveAttribute("label", "Manual selection");
  expect(await page.evaluate(() => localStorage.getItem("relay.codexPoolOauthSelection"))).toBe("auto");
  await expect(setup.getByRole("button")).toHaveCount(0);
  await expect(setup).not.toContainText("Selected account");
  await expect(page.locator(".codex-oauth-account-summary")).toHaveCount(0);
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

  await account.selectOption("none");
  await expect(setup).toContainText("No OAuth account will be applied");
  expect(await page.evaluate(() => localStorage.getItem("relay.codexPoolOauthSelection"))).toBe("none");
  await page.getByRole("button", { name: "Pool", exact: true }).click();
  await page.getByRole("button", { name: "Switch Codex to pool", exact: true }).click();
  await expect.poll(() => page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string }> }).__TAURI_TEST_INVOKES__.filter((item) => item.command === "attach_codex_to_local_gateway").length)).toBe(1);
  let call = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: Record<string, unknown> }> }).__TAURI_TEST_INVOKES__.findLast((item) => item.command === "attach_codex_to_local_gateway"));
  expect(call?.args).toEqual({ keyId: "key_synthetic", boundOauthAccountId: null, disableOauthBinding: true });

  await page.getByRole("button", { name: "Gateway", exact: true }).click();
  await page.getByRole("tab", { name: "Codex Setup" }).click();
  await page.getByLabel("Codex interface account").selectOption("account_synthetic");
  expect(await page.evaluate(() => localStorage.getItem("relay.codexPoolOauthSelection"))).toBe("account_synthetic");
  await page.getByRole("button", { name: "Pool", exact: true }).click();
  await page.getByRole("button", { name: "Switch Codex to pool", exact: true }).click();
  await expect.poll(() => page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string }> }).__TAURI_TEST_INVOKES__.filter((item) => item.command === "attach_codex_to_local_gateway").length)).toBe(2);
  call = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: Record<string, unknown> }> }).__TAURI_TEST_INVOKES__.findLast((item) => item.command === "attach_codex_to_local_gateway"));
  expect(call?.args).toEqual({ keyId: "key_synthetic", boundOauthAccountId: "account_synthetic" });
});

test("Codex pool identity migrates the previous stored account selection", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true });
  await page.addInitScript(() => localStorage.setItem("relay.codexPoolOauthAccountId", "account_synthetic"));
  await page.goto("/");
  await page.getByRole("button", { name: "Gateway", exact: true }).click();
  await page.getByRole("tab", { name: "Codex Setup" }).click();
  await expect(page.getByLabel("Codex interface account")).toHaveValue("account_synthetic");
  expect(await page.evaluate(() => ({ current: localStorage.getItem("relay.codexPoolOauthSelection"), legacy: localStorage.getItem("relay.codexPoolOauthAccountId") }))).toEqual({ current: "account_synthetic", legacy: null });
});

test("usage filters name independent choices", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Usage", exact: true }).click();
  await expect(page.getByLabel("Status").locator("option").first()).toHaveText("Any status");
  await expect(page.getByLabel("Protocol").locator("option").first()).toHaveText("Any protocol");
});

test("usage attributes API token totals to the selected account", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Usage", exact: true }).click();
  await page.getByRole("tab", { name: "Connections" }).click();

  const account = page.getByRole("row").filter({ hasText: "Personal Plus" });
  await expect(account.getByRole("cell")).toHaveText(["Personal Plus", "1", "100%", "20", "12", "5", "8", "28", "128 / 428 ms"]);

  await page.getByRole("tab", { name: "Requests" }).click();
  await page.getByRole("button", { name: "Request details: req_synthetic_local" }).click();
  const details = page.getByRole("dialog", { name: "Request details" });
  await expect(details).toContainText("Input tokens20");
  await expect(details).toContainText("Cached input12");
  await expect(details).toContainText("Reasoning tokens5");
  await expect(details).toContainText("Output tokens8");
  await expect(details).toContainText("Total tokens28");
  await expect(details).toContainText("First output128 ms");
  await expect(details).toContainText("Total time428 ms");
});

test("pool member fields explain routing priority and traffic share", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Pool", exact: true }).click();
  await page.getByRole("button", { name: "Pool member policy: Personal Plus", exact: true }).click();

  const dialog = page.getByRole("dialog", { name: /Pool member policy/ });
  await expect(dialog.getByText("Routing priority", { exact: true })).toHaveAttribute("title", "Higher-priority eligible members are considered first.");
  await expect(dialog.getByText("Traffic share", { exact: true })).toHaveAttribute("title", "Among equally eligible members, a higher share receives more requests.");
});

for (const mode of ["local", "remote"] as const) {
  test(`${mode} model rules sort and toggle the same runtime contract`, async ({ page }) => {
    await installTauriMock(page, { mode, locale: "en", populated: true, accountCount: 2 });
    await page.goto("/");
    await page.getByRole("button", { name: "Pool", exact: true }).click();
    await page.getByRole("tab", { name: "Model Rules" }).click();

    const rows = page.locator(".model-rules li");
    await expect(rows).toHaveCount(3);
    expect(await rows.evaluateAll((items) => items.map((item) => item.getAttribute("data-model-id")))).toEqual(["gpt-5.4", "gpt-5.4-mini", "o3"]);
    await expect(rows.first()).toContainText("Input $2.50");
    await expect(rows.first()).toContainText("Output $15.00");
    await expect(page.locator('.model-rules li[data-model-id="o3"]')).toContainText("Price not listed");

    await page.getByLabel("Sort models").selectOption("price_asc");
    expect(await rows.evaluateAll((items) => items.map((item) => item.getAttribute("data-model-id")))).toEqual(["gpt-5.4-mini", "gpt-5.4", "o3"]);

    const mini = page.locator('.model-rules li[data-model-id="gpt-5.4-mini"]');
    await mini.getByRole("button", { name: "Disable gpt-5.4-mini" }).click();
    await expect(mini).toHaveAttribute("data-enabled", "false");
    await expect(mini).toContainText("Disabled");
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

test("Help reopens the reversible quick setup", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Help" }).click();
  await expect(page.getByRole("button", { name: "Get started" })).toBeVisible();
});

test("OAuth keeps recovery fields behind an explicit disclosure", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await page.getByRole("button", { name: "Sign in", exact: true }).first().click();
  await expect(page.getByLabel("Resume sign-in ID")).toBeHidden();
  await page.getByText("Continue an unfinished sign-in", { exact: true }).click();
  await expect(page.getByLabel("Resume sign-in ID")).toBeVisible();
});

test("key and OAuth timestamps follow the active locale", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "ru", populated: true });
  await page.goto("/");

  await page.getByRole("button", { name: "Пул", exact: true }).click();
  await page.getByRole("tab", { name: "Ключи" }).click();
  await expect(page.getByRole("row").filter({ hasText: "Codex" })).toContainText(/\d{2}\.\d{2}\.\d{4}, \d{2}:\d{2}/);

  await page.getByRole("button", { name: "Подключения", exact: true }).click();
  await page.getByRole("button", { name: "Войти", exact: true }).first().click();
  await page.getByRole("button", { name: "Открыть вход" }).click();
  const dialog = page.getByRole("dialog", { name: "Войти" });
  await expect(dialog).toContainText(/истекает в \d{2}:\d{2}\./);
  await expect(dialog).not.toContainText(/\b(?:AM|PM)\b/);
});

test("profile header follows the available managed client", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, codexBindings: false });
  await page.goto("/");
  await page.getByRole("button", { name: "Profiles", exact: true }).click();
  await expect(page.getByRole("row").filter({ hasText: "OpenCode" })).toBeVisible();
  await expect(page.getByRole("button", { name: "Add profile" })).toBeVisible();
  await expect(page.getByRole("button", { name: "Attach Codex" })).toHaveCount(0);
  await page.getByRole("button", { name: "Add profile" }).click();
  const dialog = page.getByRole("dialog", { name: "Add profile" });
  await expect(dialog.getByRole("button", { name: /Codex/ })).toBeVisible();
  await expect(dialog.getByRole("button", { name: /OpenCode/ })).toBeEnabled();
});

test("managed Codex profiles use the local profile launcher", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, historyRepairChanges: false });
  await page.goto("/");
  await page.getByRole("button", { name: "Profiles", exact: true }).click();
  await page.getByRole("button", { name: "Launch Codex" }).click();
  const call = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string }> }).__TAURI_TEST_INVOKES__.findLast((item) => item.command === "launch_managed_codex_profile"));
  expect(call).toBeTruthy();
});

test("named Codex snapshots can be created, restored with a safety copy, and deleted", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Profiles", exact: true }).click();
  await page.getByRole("tab", { name: "Backups" }).click();

  await expect(page.getByRole("row").filter({ hasText: "Original profile" })).toBeVisible();
  await page.getByLabel("Snapshot name").fill("Before migration");
  await page.getByRole("button", { name: "Create snapshot" }).click();
  const created = page.getByRole("row").filter({ has: page.getByText("Before migration", { exact: true }) });
  await expect(created).toBeVisible();

  page.once("dialog", (dialog) => dialog.accept());
  await created.getByRole("button", { name: "Restore Before migration" }).click();
  await expect(page.getByRole("row").filter({ hasText: "Before restoring Before migration" })).toBeVisible();

  page.once("dialog", (dialog) => dialog.accept());
  await page.getByRole("row").filter({ has: page.getByText("Before migration", { exact: true }) }).getByRole("button", { name: "Delete Before migration", exact: true }).click();
  await expect(page.getByRole("row").filter({ has: page.getByText("Before migration", { exact: true }) })).toHaveCount(0);
});

test("account identity reveal is explicit and reversible", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();

  const identity = page.locator(".account-card").first().locator(".account-identity > strong");
  await expect(identity).toHaveText("Personal Plus");
  await expect(page.getByText("p***@example.test")).toHaveCount(0);
  await page.getByRole("button", { name: "Show full identity" }).click();
  await expect(identity).toHaveText("person@example.test");
  await expect(page.locator(".account-card").first().getByText("Personal Plus", { exact: true })).toHaveCount(0);
  await page.getByRole("button", { name: "Hide full identity" }).click();
  await expect(identity).toHaveText("Personal Plus");

  const call = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: Record<string, unknown> }> }).__TAURI_TEST_INVOKES__.find((item) => item.command === "reveal_local_account_identity"));
  expect(call?.args).toEqual({ accountId: "account_synthetic" });
});

test("remote account identity reveal uses the negotiated server capability", async ({ page }) => {
  await installTauriMock(page, { mode: "remote", locale: "en", populated: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await page.getByRole("button", { name: "Show full identity" }).click();
  await expect(page.getByText("person@example.test")).toBeVisible();

  const call = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: Record<string, unknown> }> }).__TAURI_TEST_INVOKES__.find((item) => item.command === "reveal_remote_account_identity"));
  expect(call?.args).toEqual({ accountId: "account_synthetic" });
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

test("pool toggle changes state without switching Codex", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, gatewayRunning: false, poolKeyPresent: false });
  await page.goto("/");
  await page.getByRole("button", { name: "Pool", exact: true }).click();
  await page.getByRole("button", { name: "Start pool", exact: true }).click();
  await expect(page.getByText("Endpoint started.")).toBeVisible();
  const header = page.locator(".relay-page-header");
  await expect(header.getByRole("button", { name: "Switch Codex to pool", exact: true })).toBeVisible();
  await expect(header.locator(".relay-page-actions > *")).toHaveCount(2);
  await header.locator(".relay-action-menu summary").click();
  await header.getByRole("menuitem", { name: "Stop pool", exact: true }).click();
  await expect(page.getByText("Endpoint stopped.")).toBeVisible();

  const calls = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: Record<string, unknown> }> }).__TAURI_TEST_INVOKES__);
  const workflow = calls.filter((call) => ["create_local_gateway_key", "start_local_gateway", "stop_local_gateway", "attach_codex_to_local_gateway", "launch_managed_codex_profile"].includes(call.command));
  expect(workflow.map((call) => call.command)).toEqual(["start_local_gateway", "stop_local_gateway"]);
});

test("switch Codex creates a pool key, attaches it, and relaunches Codex without starting the gateway", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, gatewayRunning: true, poolKeyPresent: false, historyRepairChanges: false });
  await page.goto("/");
  await page.getByRole("button", { name: "Pool", exact: true }).click();
  await page.getByRole("button", { name: "Switch Codex to pool", exact: true }).click();
  const feedback = page.locator(".global-feedback.success");
  await expect(feedback).toContainText("Client launched.");
  const feedbackBox = await feedback.boundingBox();
  expect(feedbackBox).not.toBeNull();
  expect(feedbackBox!.x).toBeLessThan(page.viewportSize()!.width / 2);
  expect(feedbackBox!.y + feedbackBox!.height).toBeGreaterThan(page.viewportSize()!.height - 64);

  const calls = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: Record<string, unknown> }> }).__TAURI_TEST_INVOKES__);
  const workflow = calls.filter((call) => ["create_local_gateway_key", "start_local_gateway", "attach_codex_to_local_gateway", "preview_codex_history_repair", "launch_managed_codex_profile"].includes(call.command));
  expect(workflow.map((call) => call.command)).toEqual(["create_local_gateway_key", "attach_codex_to_local_gateway", "preview_codex_history_repair", "launch_managed_codex_profile"]);
  expect(workflow[0].args).toEqual({ label: "Codex pool" });
  expect(workflow[1].args).toEqual({ keyId: "key_synthetic", boundOauthAccountId: null });
  await expect(feedback).toBeHidden({ timeout: 5_000 });
});

test("a Codex account or pool switch offers a protected profile snapshot", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, gatewayRunning: true, poolKeyPresent: true, historyRepairChanges: false, profileSnapshotsEmpty: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Pool", exact: true }).click();
  page.once("dialog", async (dialog) => {
    expect(dialog.message()).toContain("protected snapshot");
    await dialog.accept();
  });
  await page.getByRole("button", { name: "Switch Codex to pool", exact: true }).click();
  await expect(page.getByText("Client launched.")).toBeVisible();

  const calls = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string }> }).__TAURI_TEST_INVOKES__);
  const workflow = calls.filter((call) => ["create_codex_profile_snapshot", "attach_codex_to_local_gateway"].includes(call.command));
  expect(workflow.map((call) => call.command)).toEqual(["create_codex_profile_snapshot", "attach_codex_to_local_gateway"]);
});

test("profile snapshot prompts can be disabled without blocking Codex switching", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, gatewayRunning: true, poolKeyPresent: true, historyRepairChanges: false });
  await page.goto("/");
  await page.getByRole("button", { name: "Settings", exact: true }).click();
  const setting = page.getByRole("checkbox", { name: "Offer a profile snapshot before switching an account or pool" });
  await expect(setting).toBeChecked();
  await setting.uncheck();

  let dialogs = 0;
  page.on("dialog", async (dialog) => { dialogs += 1; await dialog.dismiss(); });
  await page.getByRole("button", { name: "Pool", exact: true }).click();
  await page.getByRole("button", { name: "Switch Codex to pool", exact: true }).click();
  await expect(page.getByText("Client launched.")).toBeVisible();
  expect(dialogs).toBe(0);

  const calls = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string }> }).__TAURI_TEST_INVOKES__);
  expect(calls.some((call) => call.command === "create_codex_profile_snapshot")).toBe(false);
});

test("profile switch errors remain readable and then dismiss automatically", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, gatewayRunning: true, poolKeyPresent: true, profileSwitchError: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Pool", exact: true }).click();
  await page.getByRole("button", { name: "Switch Codex to pool", exact: true }).click();

  const feedback = page.locator(".global-feedback.error");
  await expect(feedback).toContainText("The profile changed during the operation.");
  await expect(feedback).toBeHidden({ timeout: 9_000 });
});

test("runtime state refreshes when the app regains focus", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true });
  await page.goto("/");
  const stateReads = () => page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string }> }).__TAURI_TEST_INVOKES__.filter((call) => call.command === "get_local_runtime_state").length);
  const before = await stateReads();
  await page.evaluate(() => window.dispatchEvent(new Event("focus")));
  await expect.poll(stateReads).toBeGreaterThan(before);
});

test("row launch previews session repair and all reported quota windows stay visible", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();

  await expect(page.getByRole("button", { name: "Launch selected" })).toHaveCount(0);
  await expect(page.locator(".quota-display-menu")).toHaveCount(0);
  await expect(page.locator(".account-list .quota-meter")).toHaveCount(2);
  const launch = page.getByRole("button", { name: "Launch in Codex" });
  await expect(launch).toBeEnabled();
  await launch.click();
  const repair = page.getByRole("dialog", { name: "Keep Codex sessions visible" });
  await expect(repair).toBeVisible();
  await expect(repair.locator("dd")).toHaveText(["2", "2", "1"]);
  await repair.getByRole("button", { name: "Launch without repair" }).click();
  await expect(page.getByText("Client launched.")).toBeVisible();

  await page.getByRole("button", { name: "Settings", exact: true }).click();
  await page.getByRole("button", { name: "Appearance", exact: true }).click();
  await expect(page.getByText("Visible quota windows")).toHaveCount(0);

  const call = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: Record<string, unknown> }> }).__TAURI_TEST_INVOKES__.findLast((item) => item.command === "launch_codex_account"));
  expect(call?.args).toEqual({ accountId: "account_synthetic" });
  const profileCommands = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string }> }).__TAURI_TEST_INVOKES__.map((item) => item.command).filter((command) => ["launch_codex_account", "preview_codex_history_repair", "apply_codex_history_repair", "launch_managed_codex_profile"].includes(command)));
  expect(profileCommands).toEqual(["launch_codex_account", "preview_codex_history_repair", "launch_managed_codex_profile"]);
});

test("repair and launch applies the reviewed session changes before starting Codex", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await page.getByRole("button", { name: "Launch in Codex" }).click();

  const repair = page.getByRole("dialog", { name: "Keep Codex sessions visible" });
  await repair.getByRole("button", { name: "Repair and launch" }).click();
  await expect(page.getByText("Client launched.")).toBeVisible();

  const commands = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string }> }).__TAURI_TEST_INVOKES__.map((item) => item.command).filter((command) => ["launch_codex_account", "preview_codex_history_repair", "apply_codex_history_repair", "launch_managed_codex_profile"].includes(command)));
  expect(commands).toEqual(["launch_codex_account", "preview_codex_history_repair", "apply_codex_history_repair", "launch_managed_codex_profile"]);
});

test("row launch still starts Codex when optional history preview fails", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, historyRepairError: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await page.getByRole("button", { name: "Launch in Codex" }).click();

  await expect(page.getByText("Client launched.")).toBeVisible();
  await expect(page.getByRole("dialog", { name: "Keep Codex sessions visible" })).toHaveCount(0);
  const commands = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string }> }).__TAURI_TEST_INVOKES__.map((item) => item.command).filter((command) => ["launch_codex_account", "preview_codex_history_repair", "launch_managed_codex_profile"].includes(command)));
  expect(commands).toEqual(["launch_codex_account", "preview_codex_history_repair", "launch_managed_codex_profile"]);
});

test("remote account export uses the capability-gated server command", async ({ page }) => {
  await installTauriMock(page, { mode: "remote", locale: "en", populated: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await page.locator(".account-bulk-menu summary").click();
  await page.getByRole("menuitem", { name: "Export all" }).click();
  const dialog = page.getByRole("dialog", { name: "Export accounts" });
  await expect(dialog.getByRole("radio", { name: "sub2api" })).toHaveAttribute("aria-checked", "true");
  await dialog.getByLabel(/I understand that anyone/).check();
  await dialog.getByRole("button", { name: "Download JSON" }).click();

  const call = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: Record<string, unknown> }> }).__TAURI_TEST_INVOKES__.findLast((item) => item.command === "export_remote_accounts"));
  expect(call?.args.input).toEqual({ accountIds: ["account_synthetic"], format: "sub2api", destination: "download" });
});

test("common, account, and bulk proxy controls keep saved addresses hidden", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true });
  await page.goto("/");

  await page.getByRole("button", { name: "Gateway", exact: true }).click();
  const commonProxy = "common-user:common-pass@us-common.example:8080";
  await page.getByLabel("HTTP(S) proxy").fill(commonProxy);
  await page.locator(".proxy-settings").getByRole("button", { name: "Save" }).click();
  await expect(page.getByLabel("HTTP(S) proxy")).toHaveValue("");
  await expect(page.getByText(commonProxy)).toHaveCount(0);
  await page.getByLabel("Require a proxy for OAuth accounts").check();
  await page.locator(".proxy-settings").getByRole("button", { name: "Clear common proxy" }).click();

  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await page.getByRole("button", { name: "Direct access blocked", exact: true }).click();
  const accountProxy = "account-user:account-pass@us-account.example:8081";
  const accountDialog = page.getByRole("dialog", { name: "Proxy for Personal Plus" });
  await accountDialog.getByLabel("HTTP(S) proxy").fill(accountProxy);
  await accountDialog.getByRole("button", { name: "Save" }).click();
  await expect(page.getByText(accountProxy)).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Per-account", exact: true })).toBeVisible();

  await page.locator(".account-bulk-menu summary").click();
  await page.getByRole("menuitem", { name: "Assign proxies" }).click();
  const bulkDialog = page.getByRole("dialog", { name: "Assign account proxies" });
  const bulkList = "bulk-user:bulk-pass@us-01.example:9001\nspare-user:spare-pass@us-02.example:9002";
  await bulkDialog.getByLabel("Proxy list").fill(bulkList);
  await bulkDialog.getByRole("button", { name: "Assign", exact: true }).click();
  await expect(bulkDialog.getByRole("status")).toContainText("Assigned 1 account(s); 1 address(es) were not used.");
  await expect(bulkDialog.getByLabel("Proxy list")).toHaveValue("");
  await expect(page.getByText("bulk-pass")).toHaveCount(0);

  const calls = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: Record<string, unknown> }> }).__TAURI_TEST_INVOKES__);
  expect(calls.filter((call) => call.command === "set_local_common_proxy").map((call) => call.args)).toEqual([{ input: { proxyUrl: commonProxy } }, { input: { proxyUrl: null } }]);
  expect(calls.findLast((call) => call.command === "set_local_account_proxy_required")?.args).toEqual({ input: { required: true } });
  expect(calls.findLast((call) => call.command === "set_local_account_proxy")?.args).toEqual({ input: { accountId: "account_synthetic", proxyUrl: accountProxy } });
  expect(calls.findLast((call) => call.command === "assign_local_account_proxies")?.args).toEqual({ input: { accountIds: ["account_synthetic"], proxyUrls: bulkList.split("\n") } });
});

test("remote proxy controls use the capability-gated management actions", async ({ page }) => {
  await installTauriMock(page, { mode: "remote", locale: "en", populated: true });
  await page.goto("/");

  await page.getByRole("button", { name: "Gateway", exact: true }).click();
  await page.getByLabel("HTTP(S) proxy").fill("remote-common:secret@us-remote.example:8080");
  await page.locator(".proxy-settings").getByRole("button", { name: "Save" }).click();
  await page.getByLabel("Require a proxy for OAuth accounts").check();

  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await page.getByRole("button", { name: "Common", exact: true }).click();
  const accountDialog = page.getByRole("dialog", { name: "Proxy for Personal Plus" });
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
  expect(actions).toEqual(["set_common_proxy", "set_account_proxy_required", "set_account_proxy", "assign_account_proxies"]);
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

test("remote bulk import previews multiple files and confirms selected rows", async ({ page }) => {
  await installTauriMock(page, { mode: "remote", locale: "en", populated: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await page.getByRole("button", { name: "Import", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "Import accounts" });
  await dialog.getByRole("button", { name: "Choose JSON files" }).click();
  const imported = dialog.getByLabel("Select Imported account for import");
  const secondImported = dialog.getByLabel("Select Second imported account for import");
  const existing = dialog.getByLabel("Select Existing account for import");
  await expect(imported).toBeChecked();
  await expect(secondImported).toBeChecked();
  await expect(existing).not.toBeChecked();
  await dialog.getByRole("button", { name: "Import 2 account(s)" }).click();
  await expect(dialog).toBeHidden();
  await page.getByRole("tab", { name: "Sources" }).click();
  await page.getByRole("row").filter({ hasText: "Example compatible API" }).getByRole("button", { name: "Test" }).click();

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
      },
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

  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await expect(page.getByRole("button", { name: "Import", exact: true })).toHaveCount(1);
  await page.locator(".account-bulk-menu summary").click();
  await expect(page.getByRole("menuitem", { name: "Assign proxies" })).toBeDisabled();
  await expect(page.getByRole("menuitem", { name: "Export all" })).toBeDisabled();
  await page.locator(".account-bulk-menu summary").click();
  await page.locator(".account-card .account-row-menu summary").click();
  await expect(page.getByRole("menuitem", { name: "Export" })).toBeDisabled();
  await expect(page.getByRole("button", { name: "Show full identity" })).toHaveCount(0);

  await page.getByRole("button", { name: "Gateway", exact: true }).click();
  await expect(page.locator(".proxy-settings")).toContainText("Unsupported");
  await expect(page.locator(".proxy-settings").getByRole("button", { name: "Save" })).toBeDisabled();
  await expect(page.getByRole("button", { name: "Restart endpoint" })).toBeDisabled();
  await page.getByRole("tab", { name: "Diagnostics" }).click();
  await expect(page.locator(".diagnostics-list > section").filter({ hasText: "Streaming test" }).getByRole("button", { name: "Run" })).toBeDisabled();

  await page.getByRole("button", { name: "Pool", exact: true }).click();
  await expect(page.getByRole("tab", { name: "Keys" })).toHaveCount(0);
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

test("Ready API overview shows hosted connection facts instead of local pool counts", async ({ page }) => {
  await installTauriMock(page, { locale: "en", mode: "zenith", theme: "light", populated: true });
  await page.goto("/");
  const facts = page.locator(".overview-split section").nth(1);
  await expect(facts.getByRole("heading", { name: "API connection" })).toBeVisible();
  await expect(facts).toContainText("Requests");
  await expect(facts).toContainText("Balance");
  await expect(facts).not.toContainText("Automations");
});
