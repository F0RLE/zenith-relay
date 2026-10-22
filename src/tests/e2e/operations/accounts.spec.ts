import { expect, test } from "../../bun-playwright";
import { emitTauriEvent, installTauriMock } from "../tauri-mock";
import { chooseOption, settleConfirmation } from "./helpers";

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

test("diagnostic debug mode is opt-in and persisted by the native settings command", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Settings", exact: true }).click();
  const debug = page.getByLabel("Debug mode");
  const poolData = page.locator(".settings-group").filter({ hasText: "Pool data" });
  const diagnostics = page.locator(".settings-group").filter({ has: page.getByRole("heading", { name: "Diagnostics", exact: true }) });
  await expect(poolData.locator(".settings-debug-section")).toHaveCount(1);
  await expect(diagnostics).toHaveCount(0);
  await expect(poolData.locator(".settings-control-row").last()).toHaveClass(/settings-danger-row/);
  await expect(debug).not.toBeChecked();
  await debug.check();
  await expect(debug).toBeChecked();
  await expect(poolData.getByRole("button", { name: "Open operations" })).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Open operations", exact: true })).toHaveCount(1);
  await expect(diagnostics.getByRole("button", { name: "Open operations" })).toBeVisible();
  await expect(diagnostics).toHaveCount(1);
  await expect(page.locator(".settings-group").last()).toContainText("Diagnostics");
  await expect.poll(() => page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: Record<string, unknown> }> }).__TAURI_TEST_INVOKES__.findLast((call) => call.command === "set_diagnostic_debug_mode")?.args)).toEqual({ enabled: true });
  await debug.uncheck();
  await expect(debug).not.toBeChecked();
  await expect(diagnostics).toHaveCount(0);
  await expect.poll(() => page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: Record<string, unknown> }> }).__TAURI_TEST_INVOKES__.filter((call) => call.command === "set_diagnostic_debug_mode").length)).toBe(2);
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

test("ChatGPT recovery restores a selected manual snapshot", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, profileSnapshots: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Recovery", exact: true }).click();

  await expect(page.getByRole("button", { name: "Restore Before switch" })).toBeVisible();
  await page.getByRole("button", { name: "Restore Before switch" }).click();
  const restoreDialog = page.getByRole("dialog", { name: "Restore snapshot" });
  await restoreDialog.getByRole("button", { name: "Yes" }).click();
  await expect(restoreDialog).toHaveCount(0);
  const commands = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string }> }).__TAURI_TEST_INVOKES__.map((call) => call.command));
  expect(commands).toContain("restore_full_codex_profile_snapshot");
});

test("Overview launches OpenCode without attaching or restarting it", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Launch application" }).click();
  const dialog = page.getByRole("dialog", { name: "Which application do you want to launch?" });
  await expect(dialog.getByRole("checkbox")).toHaveCount(0);
  await dialog.getByRole("button", { name: "OpenCode", exact: true }).click();

  await expect(page.locator(".global-feedback.success")).toContainText("Client launched.");
  const commands = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string }> }).__TAURI_TEST_INVOKES__.map((call) => call.command));
  expect(commands.filter((command) => command === "launch_application")).toHaveLength(1);
  expect(commands).not.toContain("connect_opencode_to_local_gateway");
  expect(commands).not.toContain("restart_opencode_app");
});

test("confirmed local reset delegates protected restoration to Rust", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Settings", exact: true }).click();
  await page.locator(".settings-group").filter({ hasText: "Pool data" }).getByRole("button", { name: "Reset" }).click();
  await settleConfirmation(page);

  await expect.poll(() => page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string }> }).__TAURI_TEST_INVOKES__.some((call) => call.command === "reset_local_pool_data"))).toBe(true);
});

test("local pool reset is hidden outside Computer mode", async ({ page }) => {
  await installTauriMock(page, { mode: "hosted", locale: "en", populated: true });
  await page.goto("/");
  await expect(page.getByText("In development", { exact: true })).toBeVisible();
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
  await expect(page.getByRole("button", { name: "Request details: req_synthetic_local", exact: true })).toBeVisible();
});

test("usage pagination follows the errors table", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, usageFailure: true, usageTotalPages: 3 });
  await page.goto("/");
  await page.getByRole("button", { name: "Usage", exact: true }).click();
  await page.getByRole("tab", { name: "Errors", exact: true }).click();

  const pagination = page.getByRole("navigation", { name: "Usage pages" });
  await expect(pagination.locator('[aria-current="page"]')).toHaveText("1");
  await expect(pagination.locator(".usage-pagination-page")).toHaveText(["1", "2", "3"]);
  await expect(pagination.getByRole("textbox")).toHaveCount(0);
  await expect(page.locator(".relay-table-wrap + .usage-pagination")).toBeVisible();
  await pagination.getByRole("button", { name: "Page 2", exact: true }).click();
  await expect(pagination.locator('[aria-current="page"]')).toHaveText("2");
});

for (const mode of ["local"] as const) {
  test(`usage can jump directly to a page in ${mode} mode`, async ({ page }) => {
    await installTauriMock(page, { mode, locale: "en", populated: true, usageFailure: true, usageTotalPages: 384 });
    await page.goto("/");
    await page.getByRole("button", { name: "Usage", exact: true }).click();

    const pagination = page.getByRole("navigation", { name: "Usage pages" });
    const input = pagination.getByRole("textbox", { name: "Page" });
    const command = "get_local_usage_page";
    const requestedPages = () => page.evaluate((commandName) => (window as unknown as {
      __TAURI_TEST_INVOKES__: Array<{ command: string; args: { input?: { page?: number } } }>;
    }).__TAURI_TEST_INVOKES__.filter((call) => call.command === commandName).map((call) => call.args.input?.page), command);

    await expect(pagination.locator(".usage-pagination-page")).toHaveText(["1", "2", "3", "384"]);
    await expect(pagination.getByRole("button", { name: "Back" })).toBeDisabled();
    await pagination.getByRole("button", { name: "Jump to page", exact: true }).click();
    await expect(input).toBeFocused();
    await expect(input).toHaveValue("1");
    const beforeTyping = await requestedPages();
    await input.fill("25");
    expect(await requestedPages()).toEqual(beforeTyping);
    await input.press("Enter");
    await expect(pagination.locator('[aria-current="page"]')).toHaveText("25");
    await expect(pagination.locator(".usage-pagination-page")).toHaveText(["1", "25", "384"]);
    await expect.poll(async () => (await requestedPages()).at(-1)).toBe(25);

    await pagination.getByRole("button", { name: "Page 25", exact: true }).click();
    await input.fill("385");
    await input.press("Enter");
    await expect(pagination.getByRole("alert")).toContainText("1 to 384");
    expect((await requestedPages()).at(-1)).toBe(25);
    await input.fill("384");
    await pagination.getByRole("button", { name: "Go" }).click();
    await expect(pagination.locator('[aria-current="page"]')).toHaveText("384");
    await expect(pagination.locator(".usage-pagination-page")).toHaveText(["1", "382", "383", "384"]);
    await expect.poll(async () => (await requestedPages()).at(-1)).toBe(384);
    await expect(pagination.getByRole("button", { name: "Continue" })).toBeDisabled();

    await pagination.getByRole("button", { name: "Back" }).click();
    await expect(pagination.locator('[aria-current="page"]')).toHaveText("383");
    await expect.poll(async () => (await requestedPages()).at(-1)).toBe(383);

    await pagination.getByRole("button", { name: "Page 383", exact: true }).click();
    await input.fill("20");
    await input.press("Escape");
    await expect(pagination.getByRole("button", { name: "Page 383", exact: true })).toBeFocused();
    expect((await requestedPages()).at(-1)).toBe(383);

    await page.getByRole("tab", { name: "Errors", exact: true }).click();
    await expect(pagination.locator('[aria-current="page"]')).toHaveText("1");
    await expect.poll(async () => (await requestedPages()).at(-1)).toBe(1);
  });
}

test("dense status rows use accessible icons without repeated labels", async ({ page }) => {
  await installTauriMock(page, {
    mode: "local",
    locale: "en",
    populated: true,
    profileSnapshots: true,
  });
  await page.goto("/");

  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await page.getByRole("tab", { name: "Sources" }).click();
  await expect(page.locator(".source-card").first().locator(".relay-status-icon")).toHaveAttribute("aria-label", "Ready for requests");

  await page.getByRole("button", { name: "Pool", exact: true }).click();
  await page.getByRole("tab", { name: "Model Rules" }).click();
  // Availability is expressed by the power toggle, not a separate status column.
  const firstModelRow = page.locator(".model-rules tbody tr[data-model-id]").first();
  await expect(firstModelRow.locator(".relay-status-icon")).toHaveCount(0);
  await expect(firstModelRow.locator(".model-toggle")).toBeChecked();

  await page.getByRole("button", { name: "Usage", exact: true }).click();
  const requestStatus = page.locator(".usage-request-table tbody tr td").nth(1);
  await expect(requestStatus.locator(".relay-status-icon")).toHaveAttribute("aria-label", "Success");
  await expect(requestStatus).toHaveText("Success");

  await page.getByRole("button", { name: "Recovery", exact: true }).click();
  await expect(page.getByRole("button", { name: "Restore Before switch" })).toBeVisible();
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

for (const [kind, badge] of [["codex", "Codex"], ["siwc", "SIWC"], ["excel_bps", "Excel / BPS"]] as const) {
  test(`connection and pool cards distinguish the ${kind} issuing client`, async ({ page }) => {
    await installTauriMock(page, { mode: "local", locale: "en", populated: true, accountOAuthClientKind: kind });
    await page.goto("/");
    await page.getByRole("button", { name: "Connections", exact: true }).click();
    const account = page.locator(".account-card").first();
    await expect(account.locator(".account-route-selector > button")).toHaveText(badge);
    await expect(account.locator(".account-route-selector > button")).toHaveAttribute("data-value", kind);
    await expect(account.locator(".account-plan-badge")).toHaveText("Plus");

    await page.getByRole("button", { name: "Pool", exact: true }).click();
    const member = page.locator('.pool-member-card[data-member-kind="account"]').first();
    await expect(member.locator(".account-route-selector > button")).toHaveText(badge);
    await expect(member.locator(".account-route-selector > button")).toHaveAttribute("data-value", kind);
    await expect(member.locator(".account-plan-badge")).toHaveText("Plus");
  });
}

for (const locale of ["en", "ru"] as const) {
  test(`SIWC missing metadata is explicit in Connections and Pool (${locale})`, async ({ page }) => {
    await installTauriMock(page, {
      mode: "local", locale, populated: true, accountOAuthClientKind: "siwc",
      accountPlanType: null, accountQuotaMissing: true, quotaRefreshStatus: "pending",
    });
    await page.goto("/");
    await page.getByRole("button", { name: locale === "ru" ? "Подключения" : "Connections", exact: true }).click();
    const card = page.locator(".account-card").first();
    await expect(card.locator(".account-plan-badge")).toHaveText(locale === "ru" ? "Тариф не передан" : "Plan not provided");
    await expect(card.locator(".account-quota-refresh-state")).toHaveText(locale === "ru" ? "Квота не передана" : "Quota not provided");
    await expect(card.locator(".quota-meter")).toHaveCount(0);
    await page.getByRole("button", { name: locale === "ru" ? "Пул" : "Pool", exact: true }).click();
    const member = page.locator('.pool-member-card[data-member-kind="account"]').first();
    await expect(member.locator(".account-plan-badge")).toHaveText(locale === "ru" ? "Тариф не передан" : "Plan not provided");
    await expect(member.locator(".account-quota-refresh-state")).toHaveText(locale === "ru" ? "Квота не передана" : "Quota not provided");
  });

  test(`SIWC exports only its registration-preserving format (${locale})`, async ({ page }) => {
    await installTauriMock(page, { mode: "local", locale, populated: true, accountOAuthClientKind: "siwc" });
    await page.goto("/");
    await page.getByRole("button", { name: locale === "en" ? "Connections" : "Подключения", exact: true }).click();
    await page.locator(".account-bulk-menu summary").click();
    await page.getByRole("menuitem", { name: locale === "en" ? "Export all" : "Экспортировать все", exact: true }).click();
    const dialog = page.getByRole("dialog", { name: locale === "en" ? "Export accounts" : "Экспорт учётных записей" });
    await expect(dialog.getByRole("alert")).toContainText(locale === "en" ? "After import, sign in again" : "После импорта войдите повторно");
    await expect(dialog.getByRole("radio")).toHaveCount(1);
    await expect(dialog.getByRole("radio", { name: "Zenith", exact: true })).toHaveAttribute("aria-checked", "true");
    await dialog.getByRole("button", { name: locale === "en" ? "Download JSON" : "Скачать JSON", exact: true }).click();
    const exports = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: { input: unknown } }> }).__TAURI_TEST_INVOKES__.filter((call) => call.command === "export_local_accounts"));
    expect(exports).toHaveLength(1);
    expect(exports[0].args.input).toEqual({ accountIds: ["account_synthetic"], format: "zenith", destination: "download" });
  });
}

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
  await dialog.getByText("Seller page", { exact: true }).hover();
  await expect(page.getByRole("tooltip")).toHaveText("https://example.invalid");
  await expect(dialog.getByText("Seller page", { exact: true })).not.toHaveAttribute("title");
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
    "Refresh",
    "Launch in ChatGPT",
  ]);
  await page.locator(".account-card .account-row-menu summary").click();
  const menu = page.getByRole("menu");
  await expect(menu.getByRole("menuitem", { name: "Proxy: Common", exact: true })).toBeVisible();
  await expect(menu.getByRole("menuitem", { name: "Export" })).toBeVisible();
  await expect(menu.getByRole("menuitem", { name: "Notes", exact: true })).toBeVisible();
  await expect(menu.getByRole("menuitem")).toHaveCount(6);
  await expect(menu.getByRole("menuitem", { name: "Disable" })).toBeVisible();
  await expect(menu.getByRole("menuitem", { name: "Delete", exact: true })).toBeVisible();
  await expect(menu.getByRole("menuitem", { name: "Delete permanently", exact: true })).toHaveCount(0);
  await menu.getByRole("menuitem", { name: "Delete", exact: true }).click({ button: "right" });
  await page.getByRole("menu", { name: "Context menu", exact: true }).getByRole("menuitem", { name: "Delete permanently", exact: true }).click();
  await expect(page.getByRole("dialog", { name: "Delete permanently", exact: true })).toBeVisible();
});

test("excluded connection exposes an add-to-pool action", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, poolMembers: false, gatewayRunning: false, accountOAuthClientKind: "excel_bps" });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();

  const actions = page.locator(".account-card").first().locator(".account-card-actions");
  await expect(actions.getByRole("button", { name: "Add to pool", exact: true })).toBeVisible();
  await expect(actions.getByRole("button", { name: "Excluded", exact: true })).toHaveCount(0);
});

test("Russian excluded connection exposes an add-to-pool action", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "ru", populated: true, poolMembers: false, gatewayRunning: false, accountOAuthClientKind: "excel_bps" });
  await page.goto("/");
  await page.getByRole("button", { name: "Подключения", exact: true }).click();

  const actions = page.locator(".account-card").first().locator(".account-card-actions");
  await expect(actions.getByRole("button", { name: "Добавить в пул", exact: true })).toBeVisible();
  await expect(actions.getByRole("button", { name: "Исключены", exact: true })).toHaveCount(0);
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
  expect(errorJson).toMatchObject({ code: "auth_invalid_grant", message: "Signed out or account changed", connection_kind: "codex", health: "healthy", auth_state: "requires_reauth", subscription_status: "active", observed_at: null });
  expect(errorJson).not.toHaveProperty("account");
  await expect(errorDialog).not.toContainText("Personal Plus");
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

test("connections group availability while the automatic pool ranks fresh headroom", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, accountCount: 6, sourceCount: 3, usageAccountIndex: 3 });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  const labels = page.locator(".account-card .account-identity > strong");

  await expect(labels).toHaveText(["Pro account", "Business Workspace", "Backup account", "Quota pending", "Free reserve", "Personal Plus"]);
  const connectionOrder = await labels.allTextContents();
  await expect(page.getByRole("button", { name: /Sort accounts/ })).toHaveCount(0);
  await expect(page.getByRole("button", { name: "List view" })).toHaveCount(0);
  await expect(page.locator(".account-priority")).toHaveCount(0);

  await page.getByRole("button", { name: "Pool", exact: true }).click();
  const poolAccounts = page.locator('.pool-member-card[data-member-kind="account"] .pool-member-name');
  await expect(poolAccounts).toHaveCount(connectionOrder.length);
  const poolAccountOrder = await poolAccounts.allTextContents();
  // The scheduler's common next candidate wins; stale quota cannot promote Backup.
  expect(poolAccountOrder).toEqual(["Pro account", "Free reserve", "Business Workspace", "Backup account", "Quota pending", "Personal Plus"]);

  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await page.getByRole("tab", { name: "Sources", exact: true }).click();
  const sourceRows = page.locator(".source-card");
  await expect(sourceRows).toHaveCount(3);
  const connectionSourceOrder = await sourceRows.locator(".account-identity > strong").allTextContents();

  await page.getByRole("button", { name: "Pool", exact: true }).click();
  const poolSourceOrder = await page.locator('.pool-member-card[data-member-kind="source"] .pool-member-name').allTextContents();
  expect(poolSourceOrder).toEqual(connectionSourceOrder);
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
  const indicator = member.locator('.pool-member-kind-icon[data-status="error"]');
  await expect(member).not.toContainText("auth_invalidated_refresh_token");
  await expect(indicator).toHaveAttribute("aria-label", "Sign-in revoked");
  await indicator.hover();
  await expect(page.getByRole("tooltip")).toHaveText("Sign-in revoked");
  await expect(member.getByText("Waiting for quota", { exact: true })).toHaveCount(0);
});

test("runtime retry hints do not turn available accounts into quota pauses", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, accountHealth: "degraded", quotaAvailable: true, accountCooldown: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();

  const connection = page.locator(".account-card").filter({ hasText: "Personal Plus" });
  await expect(connection.locator(".relay-status-icon")).toHaveAttribute("aria-label", "In rotation");
  await expect(connection.getByText("Waiting for quota", { exact: true })).toHaveCount(0);

  await page.getByRole("button", { name: "Pool", exact: true }).click();
  const member = page.locator('[data-member-label="Personal Plus"]');
  await expect(member.locator('.pool-member-kind-icon')).toHaveAttribute("aria-label", /^Retry after .+ · In rotation$/);
  await expect(member.locator('.pool-member-kind-icon')).toHaveAttribute("data-status", "ready");
  await expect(member.getByText("Waiting for quota", { exact: true })).toHaveCount(0);
  await expect(member.locator(".pool-member-kind-icon")).not.toHaveAttribute("title", /^Retry after /);
});

test("connections and pool keep model cooldown in the status icon without duplicate card text", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, quotaAvailable: true, accountModelCooldown: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();

  const connection = page.locator(".account-card").filter({ hasText: "Personal Plus" });
  await expect(connection.locator(".account-runtime-line")).toHaveCount(0);
  const connectionStatus = connection.locator('.relay-status-icon[aria-label^="In rotation"]');
  await expect(connectionStatus).toHaveAttribute("aria-label", /gpt-5\.4: retry after/);
  await connectionStatus.hover();
  await expect(page.getByRole("tooltip")).toContainText("gpt-5.4: retry after");

  await page.getByRole("button", { name: "Pool", exact: true }).click();
  const member = page.locator('[data-member-label="Personal Plus"]');
  await expect(member.locator(".account-runtime-line")).toHaveCount(0);
  const poolStatus = member.locator(".pool-member-kind-icon");
  await expect(poolStatus).toHaveAttribute("aria-label", /gpt-5\.4: retry after/);
  await expect(poolStatus).not.toHaveAttribute("data-status", "error");
  await poolStatus.hover();
  await expect(page.getByRole("tooltip")).toContainText("gpt-5.4: retry after");
});

test("pool source errors use one status indicator without duplicate card text", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, sourceErrorCode: "upstream model discovery failed" });
  await page.goto("/");
  await page.getByRole("button", { name: "Pool", exact: true }).click();

  const source = page.locator('.pool-member-card[data-member-kind="source"]');
  await expect(source.locator('.pool-member-kind-icon[data-status="error"]')).toBeVisible();
  await expect(source.locator(".account-runtime-line")).toHaveCount(0);
  await expect(source).not.toHaveAttribute("title", /upstream model discovery failed/);
  await expect(source.locator('.pool-member-kind-icon[data-status="error"]')).toHaveAttribute("aria-label", "Error: upstream model discovery failed");
  await source.locator(".pool-member-kind-icon").hover();
  await expect(page.getByRole("tooltip")).toHaveCount(0);
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

  const refreshAll = page.locator(".account-command-actions").getByRole("button", { name: "Refresh", exact: true });
  await expect(refreshAll).toBeVisible();
  await refreshAll.click();
  await expect(page.getByText("Updated: 1 · Errors: 0", { exact: true })).toBeVisible();
  await expect(page.getByRole("button", { name: "Refresh and delete non-working accounts" })).toHaveCount(0);
  await page.locator(".account-bulk-menu summary").click();
  await expect(page.getByRole("menuitem", { name: "Refresh", exact: true })).toHaveCount(0);
  await expect(page.getByRole("menuitem", { name: "Refresh and delete non-working accounts" })).toHaveCount(0);
});

test("accounts without quota show the automatic refresh state", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, accountCount: 5 });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  const pending = page.locator(".account-card").filter({ hasText: "Quota pending" });
  await expect(pending.locator(".account-quota-refresh-state")).toContainText("Waiting for check");
});

test("account cards show subscription dates only when available", async ({ page }) => {
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
  await expect(cards.filter({ hasText: "Backup account" }).locator(".account-subscription-line")).toHaveCount(0);

  await page.getByRole("button", { name: "Pool", exact: true }).click();
  const poolPersonal = page.locator('.pool-member-card[data-member-label="Personal Plus"]');
  const poolBackup = page.locator('.pool-member-card[data-member-label="Backup account"]');
  await expect(poolPersonal.locator(".account-subscription-line")).toContainText(/\d{2}\/\d{2}\/\d{4}/);
  await expect(poolPersonal.locator(".account-subscription-countdown")).toHaveText(/^\d+ d \d+ h \d+ min$/);
  await expect(poolBackup.locator(".account-subscription-line")).toHaveCount(0);
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
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, accountCount: 3, accountOAuthClientKind: "siwc" });
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
  await expect(page.getByRole("dialog", { name: "Add a regular account to the pool?", exact: true })).toHaveCount(0);
  await expect(card).toBeHidden();

  await expect.poll(() => page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string }> }).__TAURI_TEST_INVOKES__.filter((call) => call.command === "set_local_pool_membership").length)).toBe(2);
  const calls = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: Record<string, unknown> }> }).__TAURI_TEST_INVOKES__);
  expect(calls.filter((call) => call.command === "set_local_pool_membership").map((call) => call.args)).toEqual([
    { input: { accountIds: ["account_synthetic_3"], sourceIds: [], inPool: false } },
    { input: { accountIds: ["account_synthetic_3"], sourceIds: [], inPool: true } },
  ]);
  expect(calls.some((call) => call.command === "delete_local_account")).toBe(false);
});

test("SIWC account joins the pool directly from Connections without a warning", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, poolMembers: false, accountOAuthClientKind: "siwc" });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  const card = page.locator(".account-card").first();
  await card.getByRole("button", { name: "Add to pool", exact: true }).click();
  await expect(card.getByRole("button", { name: "Remove from pool", exact: true })).toBeVisible();
  await expect(page.getByRole("dialog")).toHaveCount(0);
  const calls = await page.evaluate(() => (window as unknown as {
    __TAURI_TEST_INVOKES__: Array<{ command: string; args: Record<string, unknown> }>;
  }).__TAURI_TEST_INVOKES__.filter((call) => call.command === "set_local_pool_membership"));
  expect(calls.map((call) => call.args)).toEqual([
    { input: { accountIds: ["account_synthetic"], sourceIds: [], inPool: true } },
  ]);
});

test("Free BPS accounts have no pool addition action while paid BPS accounts do", async ({ page }) => {
  await installTauriMock(page, {
    mode: "local", locale: "en", populated: true, accountCount: 3,
    accountOAuthClientKind: "excel_bps", poolMembers: false,
  });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  const paid = page.locator(".account-card").filter({ hasText: "Personal Plus" });
  const free = page.locator(".account-card").filter({ hasText: "Backup account" });
  await expect(paid.getByRole("button", { name: "Add to pool", exact: true })).toBeVisible();
  await expect(free.getByRole("button", { name: "Add to pool", exact: true })).toHaveCount(0);
  await expect(free.getByRole("button", { name: "Remove from pool", exact: true })).toHaveCount(0);
});

for (const locale of ["ru", "en"] as const) {
  test(`Codex keeps account switching but cannot join the pool (${locale})`, async ({ page }, testInfo) => {
    await installTauriMock(page, {
      mode: "local", locale, populated: true, accountOAuthClientKind: "codex", poolMembers: false,
    });
    await page.goto("/");
    await page.getByRole("button", { name: locale === "ru" ? "Подключения" : "Connections", exact: true }).click();
    await page.setViewportSize({ width: locale === "ru" ? 390 : 1160, height: 800 });
    const card = page.locator(".account-card").first();
    await expect(card.getByRole("button", { name: locale === "ru" ? "Запустить в ChatGPT" : "Launch in ChatGPT", exact: true })).toBeEnabled();
    await expect(card.getByRole("button", { name: locale === "ru" ? "Добавить в пул" : "Add to pool", exact: true })).toHaveCount(0);
    await expect(card.locator(".account-connection-purpose")).toHaveCount(0);
    await page.getByLabel(locale === "ru" ? "Выбрать все учётные записи" : "Select all accounts").check();
    await expect(page.getByRole("button", { name: locale === "ru" ? "Добавить выбранные в пул" : "Add selected to pool", exact: true })).toHaveCount(0);
    await page.screenshot({ path: testInfo.outputPath("codex-connections.png") });
  });
  for (const [kind, client] of [["excel_bps", "BPS"], ["siwc", "SIWC"]] as const) {
    test(`${client} accounts hide app launch without a pool-only caption (${locale})`, async ({ page }, testInfo) => {
      await installTauriMock(page, {
        mode: "local", locale, populated: true, accountOAuthClientKind: kind,
      });
      await page.goto("/");
      await page.getByRole("button", { name: locale === "ru" ? "Подключения" : "Connections", exact: true }).click();
      const launch = page.locator(".account-card").first().getByRole("button", {
        name: locale === "ru" ? "Запустить в ChatGPT" : "Launch in ChatGPT", exact: true,
      });
      await expect(launch).toHaveCount(0);
      await expect(page.locator(".account-card").first().locator(".account-connection-purpose")).toHaveCount(0);
      await page.screenshot({ path: testInfo.outputPath("pool-only-connections.png") });
    });
  }
}

test("bulk account actions stay compact and delete the selected records", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, accountCount: 3 });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await page.getByLabel("Select all accounts").check();

  const actions = page.locator(".account-command-bar > div:last-child");
  await expect(actions.locator(".relay-button")).toHaveCount(0);
  await expect(actions.locator(".relay-icon-button")).toHaveCount(4);
  await expect(actions.getByRole("button", { name: "Move to server", exact: true })).toHaveCount(0);
  await expect(actions.getByRole("button", { name: "Add selected to pool" })).toHaveCount(0);
  await expect(actions.getByRole("button", { name: "Remove selected from pool" })).toBeVisible();
  await expect(actions.getByRole("button", { name: "Export selected (3)" })).toBeVisible();

  await actions.getByRole("button", { name: "Delete selected accounts" }).click();
  await expect(page.getByText("No accounts", { exact: true })).toBeVisible();

  const deleted = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: { accountIds?: string[] } }> }).__TAURI_TEST_INVOKES__.findLast((call) => call.command === "trash_local_accounts")?.args.accountIds);
  expect([...(deleted ?? [])].sort()).toEqual(["account_synthetic", "account_synthetic_2", "account_synthetic_3"].sort());
});

test("legacy personal-server accounts require confirmed offline recovery and remain disabled", async ({ page }) => {
  await installTauriMock(page, {
    mode: "local", locale: "en", populated: true, accountCount: 2,
    legacyOwnedAccountIds: ["account_synthetic", "account_synthetic_2"],
  });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await expect(page.locator(".account-card")).toHaveCount(2);
  await expect(page.locator('.account-card input[role="switch"]:checked')).toHaveCount(0);
  const card = page.locator(".account-card").filter({ hasText: "Personal Plus" });
  await card.locator(".account-row-menu summary").click();
  await expect(page.getByRole("menuitem", { name: "Return to this computer" })).toHaveCount(0);
  await page.getByRole("menuitem", { name: "Recover local account" }).click();
  const dialog = page.getByRole("dialog", { name: "Recover local account" });
  await expect(dialog).toContainText("First stop its use on the old server");
  await expect(dialog).toContainText("stays disabled and outside the pool");
  await dialog.getByRole("button", { name: "Cancel", exact: true }).click();
  expect(await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string }> }).__TAURI_TEST_INVOKES__.some((item) => item.command === "recover_local_account"))).toBe(false);
  await card.locator(".account-row-menu summary").click();
  await page.getByRole("menuitem", { name: "Recover local account" }).click();
  await dialog.getByRole("button", { name: "Recover", exact: true }).click();
  await expect(dialog).toHaveCount(0);
  await expect(card.locator('input[role="switch"]:checked')).toHaveCount(0);
  const calls = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: unknown }> }).__TAURI_TEST_INVOKES__);
  expect(calls.findLast((item) => item.command === "recover_local_account")?.args).toEqual({ accountId: "account_synthetic", confirmExternalOwnerStopped: true });
  expect(calls.filter(({ command }) => /remote_server|to_remote|remote_account/.test(command))).toEqual([]);
});

test("selecting local accounts cannot start a retired server transfer or switch profiles", async ({ page }) => {
  await installTauriMock(page, {
    mode: "local",
    locale: "en",
    populated: true,
  });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await page.getByLabel("Select all accounts").check();
  await expect(page.getByRole("button", { name: "Move to server", exact: true })).toHaveCount(0);
  await expect(page.getByText("On server", { exact: true })).toHaveCount(0);
  const commands = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string }> }).__TAURI_TEST_INVOKES__.map((item) => item.command));
  expect(commands.filter((command) => ["attach_codex_to_remote_gateway", "move_local_accounts_to_remote", "restore_codex_profile", "restore_full_codex_profile_snapshot"].includes(command))).toEqual([]);
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

  const deleted = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: { accountIds?: string[] } }> }).__TAURI_TEST_INVOKES__.filter((call) => call.command === "trash_local_accounts").flatMap((call) => call.args.accountIds ?? []));
  expect(deleted).toEqual(["account_synthetic_3"]);
});

test("icon actions explain themselves and scrollbars follow the active theme", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "ru", theme: "light", populated: true, accountCount: 3 });
  await page.setViewportSize({ width: 840, height: 560 });
  await page.goto("/");
  await page.getByRole("button", { name: "Подключения", exact: true }).click();

  const refreshAll = page.locator(".account-command-actions").getByRole("button", { name: "Обновить", exact: true });
  await refreshAll.hover();
  const tooltip = page.getByRole("tooltip");
  await expect(tooltip).toHaveText("Обновить");
  const box = await tooltip.boundingBox();
  expect(box).not.toBeNull();
  expect(box!.x).toBeGreaterThanOrEqual(8);
  expect(box!.x + box!.width).toBeLessThanOrEqual(832);
  expect(await tooltip.evaluate((element, button) => {
    const rect = element.getBoundingClientRect();
    const anchor = button.getBoundingClientRect();
    const arrow = Number.parseFloat(getComputedStyle(element).getPropertyValue("--relay-tooltip-arrow-left"));
    return Math.abs(rect.left + arrow - (anchor.left + anchor.width / 2)) <= 1;
  }, await refreshAll.elementHandle())).toBe(true);

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
  expect(light.thumb).toBe("#bacbbd");
  expect(dark.thumb).toBe("#555550");
  expect(light.hover).not.toBe(dark.hover);
  expect(light.scrollbar).not.toBe(dark.scrollbar);
});

test("pool summary shows routing states and current errors", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, accountCount: 3 });
  await page.goto("/");
  await page.getByRole("button", { name: "Pool", exact: true }).click();
  const summary = page.locator(".pool-summary");
  await expect(summary.locator("div")).toHaveCount(4);
  await expect(summary.locator("strong")).toHaveText(["3", "1", "1", "0"]);
  await expect(summary.locator("span")).toHaveText(["In rotation", "Waiting", "With errors", "Disabled"]);
});
