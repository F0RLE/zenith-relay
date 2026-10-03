import { expect, test } from "../../bun-playwright";
import { emitTauriEvent, installTauriMock } from "../tauri-mock";

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

test("an exhausted five-hour quota window does not hide the weekly window", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, quotaAvailable: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();

  const account = page.locator(".account-card").filter({ hasText: "Personal Plus" });
  const meters = account.locator(".quota-meter-heading > strong");
  await expect(meters).toHaveCount(2);
  await expect(meters.nth(0)).toHaveText("72%");
  await expect(meters.nth(1)).toHaveText("64%");

  await page.evaluate(() => {
    const internals = (window as unknown as { __TAURI_INTERNALS__: { invoke: (command: string, args?: unknown, options?: unknown) => Promise<unknown> } }).__TAURI_INTERNALS__;
    const invoke = internals.invoke.bind(internals);
    internals.invoke = async (command, args, options) => {
      const result = await invoke(command, args, options);
      if (command !== "get_local_runtime_state") return result;
      const snapshot = structuredClone(result) as { accounts: Array<{ quota: { primary: { availableBasisPoints: number } | null; secondary: { availableBasisPoints: number } | null } }> };
      const quota = snapshot.accounts[0]?.quota;
      if (quota?.primary) quota.primary.availableBasisPoints = 0;
      if (quota?.secondary) quota.secondary.availableBasisPoints = 6400;
      return snapshot;
    };
  });
  const stateReads = () => page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string }> }).__TAURI_TEST_INVOKES__.filter((call) => call.command === "get_local_runtime_state").length);
  const before = await stateReads();

  await emitTauriEvent(page, "zenith-state-changed", null);

  await expect.poll(stateReads).toBeGreaterThan(before);
  await expect(meters.nth(0)).toHaveText("0%");
  await expect(meters.nth(1)).toHaveText("64%");
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
  await page.locator(".proxy-storage-account-count").first().hover();
  await expect(page.getByRole("tooltip")).toHaveText("Business Workspace, Personal Plus");
  await expect(page.locator(".proxy-storage-account-count").first()).not.toHaveAttribute("title");

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
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, importPreviewDeferred: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Usage", exact: true }).click();
  await expect.poll(() => page.evaluate(() => {
    const calls = (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string }> }).__TAURI_TEST_INVOKES__;
    return calls.filter((call) => call.command === "plugin:event|listen").length;
  })).toBeGreaterThanOrEqual(4);
  await expect(page.locator("#splash-screen")).toHaveCount(0);
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
  await page.evaluate(() => window.dispatchEvent(new Event("relay-test-finish-import-preview")));
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

test("empty Choose API mode opens the compact source picker", async ({ page }) => {
  await installTauriMock(page, { mode: "zenith", locale: "en", populated: false, readyConnected: false });
  await page.goto("/");
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
  await expect(dialog.getByRole("button", { name: "Save", exact: true })).toHaveCount(1);

  await dialog.getByRole("radio", { name: /OpenRouter/ }).click();
  await expect(dialog.locator(".source-add-adapters")).toHaveCount(0);
  await expect(dialog.locator(".source-model-mode")).toHaveCount(0);
  await expect(dialog.getByLabel("API address", { exact: true })).toHaveValue("https://openrouter.ai/api/v1");
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
    return { inputOutline: getComputedStyle(input).outlineStyle, fieldOutline: getComputedStyle(field).outlineStyle, fieldShadow: getComputedStyle(field).boxShadow };
  })).toEqual({ inputOutline: "none", fieldOutline: "none", fieldShadow: "none" });

  await dialog.getByRole("radio", { name: /Zenith API/ }).click();
  await dialog.getByLabel("Upstream API key").fill("test-source-key");
  await dialog.getByRole("button", { name: "Save", exact: true }).click();
  await expect(page.getByRole("dialog", { name: "Add source" })).toBeHidden();
  await expect(page.getByText("Zenith API", { exact: true })).toBeVisible();
  await expect(page.getByRole("button", { name: "Mode: Choose API", exact: true })).toBeVisible();
  await expect(page.getByLabel("Launch", { exact: true })).toBeEnabled();
  const calls = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: Record<string, unknown> }> }).__TAURI_TEST_INVOKES__);
  expect(calls.find((call) => call.command === "create_local_source")?.args.input).toMatchObject({
    name: "Zenith API",
    baseUrl: "https://api.zenithmarket.dev/v1",
    wireApi: "responses",
    protocolBindings: [],
    apiKey: "test-source-key",
  });
  expect(calls.map((call) => call.command)).not.toContain("save_key");
  expect(calls.map((call) => call.command)).not.toContain("set_local_pool_membership");
});

test("provider presets leave automatic protocol discovery to the connector", async ({ page }) => {
  await installTauriMock(page, { mode: "zenith", locale: "en", populated: false, readyConnected: false });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await page.getByRole("button", { name: "Add source", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "Add source" });

  await dialog.getByRole("radio", { name: /OpenAI/ }).click();
  await expect(dialog.locator(".source-route-simple-options")).toHaveCount(0);

  await dialog.getByLabel("Upstream API key").fill("sk-synthetic-ready-key");
  await dialog.getByRole("button", { name: "Save", exact: true }).click();
  await expect(page.getByRole("dialog", { name: "Add source" })).toBeHidden();
  await expect(page.getByText("OpenAI", { exact: true })).toBeVisible();
  const calls = await page.evaluate(() => (window as unknown as {
    __TAURI_TEST_INVOKES__: Array<{ command: string; args: Record<string, unknown> }>;
  }).__TAURI_TEST_INVOKES__);
  expect(calls.find((call) => call.command === "create_local_source")?.args.input).toMatchObject({
    name: "OpenAI",
    baseUrl: "https://api.openai.com/v1",
    wireApi: "responses",
    protocolBindings: [],
    apiKey: "sk-synthetic-ready-key",
  });
});

test("source setup leaves model catalog discovery to the connector", async ({ page }) => {
  await installTauriMock(page, { mode: "zenith", locale: "en", populated: false, readyConnected: false });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await page.getByRole("button", { name: "Add source", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "Add source" });

  await dialog.getByRole("radio", { name: /Custom API/ }).click();
  await expect(dialog.locator(".source-model-mode")).toHaveCount(0);
  await expect(dialog.getByLabel("Model identifier")).toHaveCount(0);
  await expect(dialog.locator(".source-add-adapters")).toHaveCount(0);
});

test("source creation errors stay in one topmost dialog", async ({ page }) => {
  await installTauriMock(page, { mode: "zenith", locale: "en", populated: false, readyConnected: false, sourceCreateError: "source_test_failed" });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await page.getByRole("button", { name: "Add source", exact: true }).click();
  const sourceDialog = page.getByRole("dialog", { name: "Add source" });
  await sourceDialog.getByRole("radio", { name: /OpenAI/ }).click();
  await sourceDialog.getByLabel("Upstream API key").fill("sk-synthetic-invalid");
  await sourceDialog.getByRole("button", { name: "Save", exact: true }).click();

  const errorDialog = page.getByRole("dialog", { name: "Error details" });
  await expect(errorDialog).toBeVisible();
  await expect(sourceDialog).toBeVisible();
  await expect(page.locator(".global-feedback")).toHaveCount(0);
  const layers = await page.evaluate(() => [...document.querySelectorAll<HTMLElement>(".relay-modal-backdrop")].map((element) => Number.parseInt(getComputedStyle(element).zIndex, 10)));
  expect(layers).toContain(100);
  expect(layers).toContain(400);
  expect(Math.max(...layers)).toBe(400);
  await expect(errorDialog.locator("pre")).toContainText("Synthetic upstream model discovery failed");

  await errorDialog.locator("footer").getByRole("button", { name: "Close", exact: true }).click();
  await expect(errorDialog).toBeHidden();
  await expect(sourceDialog).toBeVisible();
  await expect(sourceDialog.getByLabel("Upstream API key")).toHaveValue("sk-synthetic-invalid");
});

test("source editor preserves legacy routes while routing stays automatic", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await page.getByRole("tab", { name: "Sources", exact: true }).click();
  await page.getByRole("button", { name: "Edit", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "Edit source" });
  await expect(dialog.locator(".source-add-adapters")).toHaveCount(0);
  expect(await dialog.evaluate((element) => element.scrollWidth <= element.clientWidth)).toBe(true);
  await dialog.getByRole("button", { name: "Save", exact: true }).click();
  const bindings = await page.evaluate(() => (window as unknown as {
    __TAURI_TEST_INVOKES__: Array<{ command: string; args: { input?: { protocolBindings?: unknown[] } } }>;
  }).__TAURI_TEST_INVOKES__.findLast((call) => call.command === "update_local_source")?.args.input?.protocolBindings);
  expect(bindings).toEqual(expect.arrayContaining([
    expect.objectContaining({ wireApi: "responses", adapter: "native" }),
  ]));
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
  const launch = sourceRow.getByRole("button", { name: "Launch", exact: true });
  await launch.click();
  const dialog = page.getByRole("dialog", { name: "Where do you want to launch this source?" });
  await expect(dialog.getByRole("button", { name: "ChatGPT", exact: true })).toBeDisabled();
  await expect(dialog.getByRole("button", { name: "OpenCode", exact: true })).toBeEnabled();
});

test("Choose API mode manages and launches saved sources without balance controls", async ({ page }) => {
  await installTauriMock(page, { mode: "zenith", locale: "en", populated: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await expect(page.getByText("Example compatible API", { exact: true })).toBeVisible();
  await expect(page.getByText("Balance", { exact: true })).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Top up", exact: true })).toHaveCount(0);
  await page.getByLabel("Launch", { exact: true }).click();
  await page.getByRole("dialog", { name: "Where do you want to launch this source?" }).getByRole("button", { name: "ChatGPT", exact: true }).click();
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

test("Choose API overview shows a retained observation and its failed-refresh reason", async ({ page }) => {
  await installTauriMock(page, {
    mode: "zenith", locale: "en", populated: true,
    sourceStatsById: {
      source_synthetic: { provider: "zenith", balanceMicroUsd: 42_500_000, spentMicroUsd: null,
        requests: null, totalTokens: null, status: "available", asOfMs: Date.UTC(2026, 8, 23, 12),
        stale: true, refreshError: "rate_limited" },
    },
  });
  await page.goto("/");
  await expect(page.locator(".direct-api-metrics")).toContainText("$42.50");
  await expect(page.getByText("Not refreshed", { exact: true })).toBeVisible();
  await expect(page.getByText(/Last checked:/)).toHaveCount(0);
  await expect(page.locator(".source-stats-caption[data-warning='true']")).toHaveAttribute("data-relay-tooltip", "Try again later");
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
  })).toEqual({ sourceId: "source_synthetic", force: true });
});

test("replacing a source key with the same URL retires visible statistics", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Pool", exact: true }).click();
  const source = page.locator('.pool-member-card[data-member-kind="source"]').first();
  await expect(source.locator(".pool-source-stats")).toContainText("$42.50");
  await page.evaluate(() => {
    const internals = (window as unknown as { __TAURI_INTERNALS__: { invoke: (command: string, args?: unknown, options?: unknown) => Promise<unknown> } }).__TAURI_INTERNALS__;
    const original = internals.invoke.bind(internals);
    internals.invoke = async (command, args, options) => {
      if (command === "get_local_runtime_state") {
        const snapshot = await original(command, args, options) as { sources: Array<{ refreshRevision: number }> };
        snapshot.sources[0]!.refreshRevision = 2;
        return snapshot;
      }
      if (command === "get_local_source_stats") {
        await new Promise((resolve) => setTimeout(resolve, 200));
        return { provider: "zenith", balanceMicroUsd: 17_000_000, spentMicroUsd: null, requests: null, totalTokens: null };
      }
      return original(command, args, options);
    };
  });
  await emitTauriEvent(page, "zenith-state-changed", null);
  await expect(source.locator(".pool-source-stats")).toContainText("$17.00");
  await expect(source.locator(".pool-source-stats")).not.toContainText("$42.50");
});
