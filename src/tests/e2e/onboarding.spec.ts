import { expect, test, type Page } from "../bun-playwright";
import { emitTauriEvent, installTauriMock } from "./tauri-mock";
import { chooseOption } from "./operations/helpers";

async function expectSetupFrame(page: Page) {
  await expect(page.locator("#splash-screen")).toHaveCount(0);
  await expect(page.locator('.setup-progress [aria-current="step"]')).toHaveCount(1);
  expect(await page.locator(".setup-workspace, .setup-footer").evaluateAll((items) => items.every((item) => {
    const box = item.getBoundingClientRect();
    return box.left >= 0 && box.right <= innerWidth && box.top >= 0 && box.bottom <= innerHeight && item.scrollWidth <= item.clientWidth;
  }))).toBe(true);
  expect(await page.locator(".setup-body").evaluate((body) => body.scrollWidth <= body.clientWidth)).toBe(true);
  if (await page.locator(".setup-heading").count()) {
    await expect(page.locator(".setup-heading")).toHaveCSS("text-align", "left");
  }
}

for (const kind of ["codex", "siwc", "excel_bps"] as const) {
  test(`quick setup signs in through ${kind} and only admits pool clients`, async ({ page }) => {
    await installTauriMock(page, { onboarding: false, locale: "en", populated: true, accountOAuthClientKind: kind });
    await page.goto("/");
    await page.getByRole("button", { name: "Get started" }).click();
    await page.getByRole("button", { name: "Continue" }).click();
    await expect(page.getByRole("button", { name: "Sign-in method: Codex", exact: true })).toBeVisible();
    if (kind !== "codex") await chooseOption(page, page, "Sign-in method", kind);
    await page.getByRole("button", { name: /^Sign in(?: |$)/ }).click();
    await expect.poll(() => page.evaluate(() => (window as unknown as {
      __TAURI_TEST_INVOKES__: Array<{ command: string; args: { clientKind?: string } }>;
    }).__TAURI_TEST_INVOKES__.findLast((call) => call.command === "start_codex_oauth")?.args.clientKind)).toBe(kind);
    await emitTauriEvent(page, "relay-oauth-status", { loginId: "oauth_synthetic", status: "callback_received" });
    await expect(page.getByRole("button", { name: "Continue", exact: true })).toBeEnabled();
    await expect(page.getByRole("dialog", { name: "Add a regular account to the pool?", exact: true })).toHaveCount(0);
    const membership = await page.evaluate(() => (window as unknown as {
      __TAURI_TEST_INVOKES__: Array<{ command: string; args: Record<string, unknown> }>;
    }).__TAURI_TEST_INVOKES__.findLast((call) => call.command === "set_local_pool_membership"));
    if (kind === "codex") {
      expect(membership).toBeUndefined();
      await expect(page.locator(".setup-pool-ready")).toHaveCount(0);
    } else {
      expect(membership?.args.input).toEqual({ accountIds: ["account_synthetic"], sourceIds: [], inPool: true });
      await expect(page.locator(".setup-pool-ready")).toBeVisible();
    }
  });
}

test("quick setup chooses where the shared pool runs", async ({ page }) => {
  await installTauriMock(page, { onboarding: false, locale: "en", populated: true });
  await page.goto("/");
  await expect(page.getByRole("heading", { name: "Zenith Relay" })).toBeVisible();
  await page.getByRole("button", { name: "Get started" }).click();
  await expect(page.getByRole("heading", { name: "Where should Zenith Relay run?" })).toBeVisible();
  await expect(page.locator(".mode-options button")).toHaveCount(2);
  await expect(page.getByRole("button", { name: /Choose API/ })).toHaveCount(0);
  await page.getByRole("button", { name: /Relay Hosted/ }).click();
  await page.getByRole("button", { name: "Continue" }).click();
  await expect(page.getByText("In development", { exact: true })).toBeVisible();
  const calls = await page.evaluate(() => (window as unknown as {
    __TAURI_TEST_INVOKES__: Array<{ command: string }>;
  }).__TAURI_TEST_INVOKES__);
  expect(calls.some(({ command }) => command === "connect_remote_server" || command === "start_local_gateway")).toBe(false);
  await page.screenshot({ path: "output/playwright/onboarding-hosted-1160x760.png" });
});

test("skipping repeated setup migrates the removed API-only mode to Computer", async ({ page }) => {
  await installTauriMock(page, { onboarding: false, locale: "en", populated: true, storedMode: "zenith" });
  await page.goto("/");
  await page.getByRole("button", { name: "Skip setup" }).click();
  await expect(page.getByRole("button", { name: "Mode: Computer" })).toBeVisible();
  await expect.poll(() => page.evaluate(() => localStorage.getItem("relay.mode"))).toBe("local");
  await page.getByRole("button", { name: "Mode: Computer" }).click();
  await expect(page.getByRole("menuitemradio")).toHaveText(["Computer", "Relay Hosted"]);
  await expect(page.getByRole("menuitemradio", { name: "Computer", exact: true })).toHaveAttribute("aria-checked", "true");
});

test("local quick setup verifies runtime and applies ChatGPT only after explicit choices", async ({ page }) => {
  await installTauriMock(page, { onboarding: false, locale: "en", populated: true, accountOAuthClientKind: "excel_bps" });
  await page.goto("/");
  await page.getByRole("button", { name: "Get started" }).click();
  await page.getByRole("button", { name: "Continue" }).click();
  await chooseOption(page, page, "Sign-in method", "excel_bps");
  await page.setViewportSize({ width: 840, height: 560 });
  await page.getByRole("button", { name: "Sign in" }).click();
  await expect(page.getByText("Waiting for sign-in", { exact: true })).toBeVisible();
  await expect(page.locator(".setup-connect-options")).toHaveCount(0);
  await expect(page.locator(".setup-oauth-pending")).toBeVisible();
  await expect(page.getByRole("button", { name: "Open sign-in window" })).toBeVisible();
  await expect(page.getByRole("button", { name: "Cancel" })).toBeVisible();
  await expect(page.getByRole("button", { name: "Continue" })).toBeDisabled();
  await expect(page.locator("#splash-screen")).toHaveCount(0);
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= document.documentElement.clientWidth)).toBe(true);
  expect(await page.locator(".setup-oauth-pending").evaluate((element) => {
    const rect = element.getBoundingClientRect();
    return rect.left >= 0 && rect.right <= innerWidth && element.scrollWidth <= element.clientWidth;
  })).toBe(true);
  await page.screenshot({ path: "output/playwright/onboarding-oauth-pending-840x560.png" });
  await emitTauriEvent(page, "relay-oauth-status", { loginId: "oauth_synthetic", status: "callback_received" });
  await expect.poll(() => page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string }> }).__TAURI_TEST_INVOKES__.some((call) => call.command === "complete_codex_oauth"))).toBe(true);
  await expect(page.getByRole("button", { name: "Continue" })).toBeEnabled();
  await page.getByRole("button", { name: "Continue" }).click();
  await expect(page.getByRole("heading", { name: "What should use your pool?" })).toBeVisible();
  await page.getByRole("button", { name: "ChatGPT" }).click();
  await page.getByRole("button", { name: "Continue" }).click();
  await expect(page.getByRole("heading", { name: "Relay is ready" })).toBeVisible();
  await expect(page.getByText("http://127.0.0.1:14998/v1")).toHaveCount(0);
  const calls = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: Record<string, unknown> }> }).__TAURI_TEST_INVOKES__);
  expect(calls.map((call) => call.command)).toEqual(expect.arrayContaining(["complete_codex_oauth", "set_local_pool_membership", "get_local_runtime_state", "attach_codex_to_local_gateway"]));
  expect(calls.find((call) => call.command === "set_local_pool_membership")?.args).toEqual({ input: { accountIds: ["account_synthetic"], sourceIds: [], inPool: true } });
  expect(calls.findLast((call) => call.command === "attach_codex_to_local_gateway")?.args).toEqual({ boundOauthAccountId: null });
  expect(calls.map((call) => call.command)).not.toContain("launch_managed_codex_profile");
});

test("local quick setup offers Claude Desktop second and connects it without launching", async ({ page }) => {
  await installTauriMock(page, { onboarding: false, locale: "en", populated: true });
  await page.goto("/");
  await expect(page.locator(".intro-flow-harnesses .intro-flow-node")).toHaveText(["ChatGPT", "Claude Desktop", "OpenCode"]);
  await page.getByRole("button", { name: "Get started" }).click();
  await page.getByRole("button", { name: "Continue" }).click();
  await page.getByRole("button", { name: "Continue" }).click();
  await expect(page.locator(".client-options button")).toHaveText([
    "ChatGPTChats and coding tasks in the ChatGPT application.",
    "Claude DesktopUse local pool models in Claude Desktop.",
    "OpenCodeA coding assistant in your terminal.",
    "Configure laterChoose and connect an application after setup.",
  ]);
  await page.getByRole("button", { name: "Claude Desktop", exact: true }).click();
  await page.getByRole("button", { name: "Continue" }).click();
  await expect(page.getByRole("heading", { name: "Relay is ready" })).toBeVisible();
  const commands = await page.evaluate(() => (window as unknown as {
    __TAURI_TEST_INVOKES__: Array<{ command: string }>;
  }).__TAURI_TEST_INVOKES__.map((call) => call.command));
  expect(commands).toContain("connect_claude_desktop");
  expect(commands).not.toContain("launch_claude_desktop");
  expect(commands).not.toContain("connect_opencode_to_local_gateway");
});


test("local quick setup connects OpenCode to the pool without launching a client", async ({ page }) => {
  await installTauriMock(page, { onboarding: false, locale: "en", populated: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Get started" }).click();
  await page.getByRole("button", { name: "Continue" }).click();
  await page.getByRole("button", { name: "Continue" }).click();
  await expect(page.getByRole("heading", { name: "What should use your pool?" })).toBeVisible();
  await page.getByRole("button", { name: "OpenCode", exact: true }).click();
  await page.getByRole("button", { name: "Continue" }).click();
  await expect(page.getByRole("heading", { name: "Relay is ready" })).toBeVisible();
  await expect.poll(() => page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string }> }).__TAURI_TEST_INVOKES__.some((call) => call.command === "connect_opencode_to_local_gateway"))).toBe(true);
  const commands = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string }> }).__TAURI_TEST_INVOKES__.map((call) => call.command));
  expect(commands).not.toContain("restart_opencode_app");
  expect(commands).not.toContain("launch_managed_codex_profile");
});

test("local quick setup imports through the unified dialog and selects pool membership", async ({ page }) => {
  await installTauriMock(page, { onboarding: false, locale: "en", populated: true, accountOAuthClientKind: "excel_bps" });
  await page.goto("/");
  await page.getByRole("button", { name: "Get started" }).click();
  await page.getByRole("button", { name: "Continue" }).click();
  await page.getByRole("button", { name: /Import accounts/ }).click();
  const dialog = page.getByRole("dialog", { name: "Import accounts" });
  await dialog.getByRole("button", { name: /Choose account files/ }).click();
  await expect(dialog.getByLabel("Add selected to pool after import")).toBeChecked();
  await dialog.getByRole("button", { name: /Import 2 account/ }).click();
  await expect(page.getByRole("dialog", { name: "Add a regular account to the pool?", exact: true })).toHaveCount(0);
  await expect(dialog).not.toBeVisible();
  const call = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: Record<string, unknown> }> }).__TAURI_TEST_INVOKES__.findLast((item) => item.command === "confirm_local_account_import"));
  expect(call?.args.input).toMatchObject({ addToPool: true });
});

test("local quick setup imports account files and allows adding an API before continuing", async ({ page }) => {
  await installTauriMock(page, { onboarding: false, locale: "en", populated: true, accountOAuthClientKind: "excel_bps", gatewayRunning: false, importConfirmDelayMs: 1_000 });
  await page.goto("/");
  await page.getByRole("button", { name: "Get started" }).click();
  await page.getByRole("button", { name: "Continue" }).click();
  await page.getByRole("button", { name: /Import accounts/ }).click();
  const dialog = page.getByRole("dialog", { name: "Import accounts" });
  await dialog.getByRole("button", { name: /Choose account files/ }).click();
  await dialog.getByRole("button", { name: /Import 2 account/ }).click();
  await expect(dialog.getByRole("status")).toContainText("Importing");
  await expect(dialog).toBeHidden();
  const ready = page.locator(".setup-pool-ready");
  await expect(ready).toBeVisible();
  await expect(page.getByRole("button", { name: /^Add API source/ })).toBeVisible();
  await expect(page.getByRole("button", { name: "Continue" })).toBeEnabled();
  await expectSetupFrame(page);
  await page.screenshot({ path: "output/playwright/onboarding-import-success-1160x760.png" });
  await page.setViewportSize({ width: 840, height: 560 });
  await expect(ready).toBeVisible();
  await expectSetupFrame(page);
  await page.screenshot({ path: "output/playwright/onboarding-import-success-840x560.png" });
  const calls = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: Record<string, unknown> }> }).__TAURI_TEST_INVOKES__);
  expect(calls.map((call) => call.command)).toEqual(expect.arrayContaining(["preview_local_account_import_files", "confirm_local_account_import"]));
  expect(calls.map((call) => call.command)).not.toContain("preview_current_codex_account_import");
  expect(calls.find((call) => call.command === "confirm_local_account_import")?.args.input).toMatchObject({
    sessionId: "11111111-2222-4333-8444-555555555555",
    addToPool: true,
  });
  await page.getByRole("button", { name: "Continue" }).click();
  await expect(page.getByRole("heading", { name: "What should use your pool?" })).toBeVisible();
  await expect.poll(() => page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string }> }).__TAURI_TEST_INVOKES__.some((call) => call.command === "start_local_gateway"))).toBe(true);
});

test("failed account import keeps setup on the connection step and permits retry", async ({ page }) => {
  await installTauriMock(page, { onboarding: false, locale: "en", populated: true, importResult: "item_failure" });
  await page.goto("/");
  await page.getByRole("button", { name: "Get started" }).click();
  await page.getByRole("button", { name: "Continue" }).click();
  await page.getByRole("button", { name: /Import accounts/ }).click();
  const dialog = page.getByRole("dialog", { name: "Import accounts" });
  await dialog.getByRole("button", { name: /Choose account files/ }).click();
  await dialog.getByRole("button", { name: /Import 2 account/ }).click();
  await expect(dialog.getByRole("alert")).toContainText("Some accounts were not imported");
  await expect(dialog.getByRole("button", { name: "Retry failed" })).toBeVisible();
  await page.screenshot({ path: "output/playwright/onboarding-import-error-1160x760.png" });
  await expect(page.getByRole("heading", { name: "What should use your pool?" })).toHaveCount(0);
  await dialog.getByRole("button", { name: "Close", exact: true }).last().click();
  await expect(dialog).toBeHidden();
  await expectSetupFrame(page);
  const calls = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string }> }).__TAURI_TEST_INVOKES__);
  expect(calls.map((call) => call.command)).toContain("cancel_local_account_import");
});

test("closing the account preview cancels its session before leaving setup", async ({ page }) => {
  await installTauriMock(page, { onboarding: false, locale: "en", populated: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Get started" }).click();
  await page.getByRole("button", { name: "Continue" }).click();
  await page.getByRole("button", { name: /Import accounts/ }).click();
  const dialog = page.getByRole("dialog", { name: "Import accounts" });
  await dialog.getByRole("button", { name: /Choose account files/ }).click();
  await expect(dialog.getByRole("button", { name: /Import 2 account/ })).toBeVisible();
  await dialog.getByRole("button", { name: "Cancel", exact: true }).click();
  await expect(dialog).toBeHidden();
  await page.getByRole("button", { name: "Back" }).click();
  await expect(page.getByRole("heading", { name: "Where should Zenith Relay run?" })).toBeVisible();
  await expect.poll(() => page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string }> }).__TAURI_TEST_INVOKES__.some((call) => call.command === "cancel_local_account_import"))).toBe(true);
  const calls = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string }> }).__TAURI_TEST_INVOKES__);
  expect(calls.map((call) => call.command)).toEqual(expect.arrayContaining(["preview_local_account_import_files", "cancel_local_account_import"]));
  expect(calls.map((call) => call.command)).not.toContain("confirm_local_account_import");
});

test("setup offers explicit connections without importing the active ChatGPT profile", async ({ page }) => {
  await installTauriMock(page, { onboarding: false, locale: "ru", theme: "dark", populated: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Приступить" }).click();
  await page.getByRole("button", { name: "Продолжить" }).click();
  await expect(page.getByRole("button", { name: /Импортировать текущий профиль/ })).toHaveCount(0);
  await expect(page.locator(".setup-connect-options button")).toHaveCount(3);
  await expect(page.locator(".setup-connected")).toHaveCount(0);
  const commands = await page.evaluate(() => (window as unknown as {
    __TAURI_TEST_INVOKES__: Array<{ command: string }>;
  }).__TAURI_TEST_INVOKES__.map(call => call.command));
  expect(commands.filter(command => [
    "current_chatgpt_profile_available", "preview_current_codex_account_import",
  ].includes(command))).toEqual([]);
  await page.screenshot({ path: "output/playwright/onboarding-step-2-no-profile-ru-dark-1160x760.png" });
});

test("all onboarding steps keep progress and actions in one bounded workspace", async ({ page }) => {
  await installTauriMock(page, { onboarding: false, locale: "ru", theme: "dark", populated: true, accountOAuthClientKind: "excel_bps" });
  await page.setViewportSize({ width: 1160, height: 760 });
  await page.goto("/");
  await page.getByRole("button", { name: "Приступить" }).click();

  await expectSetupFrame(page);
  await page.screenshot({ path: "output/playwright/onboarding-step-1-mode-ru-dark-1160x760.png" });
  await page.getByRole("button", { name: "Продолжить" }).click();
  await expect(page.getByRole("button", { name: /Импортировать учётные записи/ })).toBeVisible();
  await expectSetupFrame(page);
  await page.screenshot({ path: "output/playwright/onboarding-step-2-connection-ru-dark-1160x760.png" });

  await page.getByRole("button", { name: /Импортировать учётные записи/ }).click();
  const dialog = page.getByRole("dialog", { name: "Импортировать учётные записи" });
  await dialog.getByRole("button", { name: /Выбрать файлы/ }).click();
  await dialog.getByRole("button", { name: "Импортировать: 2" }).click();
  await expect(dialog).toBeHidden();
  await expect(page.locator(".setup-pool-ready")).toBeVisible();
  await page.getByRole("button", { name: "Продолжить" }).click();
  await expect(page.locator(".client-options")).toBeVisible();
  await expectSetupFrame(page);
  await page.screenshot({ path: "output/playwright/onboarding-step-3-client-ru-dark-1160x760.png" });
  await page.getByRole("button", { name: "Продолжить" }).click();
  await expectSetupFrame(page);
  await page.screenshot({ path: "output/playwright/onboarding-step-4-ready-ru-dark-1160x760.png" });
});

test("local quick setup combines an account and an OpenRouter source in the pool", async ({ page }) => {
  await installTauriMock(page, { onboarding: false, locale: "en", mode: "local", populated: true, accountOAuthClientKind: "excel_bps" });
  await page.goto("/");
  await page.getByRole("button", { name: "Get started" }).click();
  await page.getByRole("button", { name: "Continue" }).click();
  await page.getByRole("button", { name: /Import accounts/ }).click();
  const importDialog = page.getByRole("dialog", { name: "Import accounts" });
  await importDialog.getByRole("button", { name: /Choose account files/ }).click();
  await importDialog.getByRole("button", { name: /Import 2 account/ }).click();
  await expect(importDialog).toBeHidden();
  await expect(page.locator(".setup-pool-ready")).toBeVisible();
  await page.getByRole("button", { name: /^Add API source/ }).click();
  const dialog = page.getByRole("dialog", { name: "Add API source" });
  await dialog.getByRole("radio", { name: /OpenRouter/ }).click();
  await dialog.getByLabel("Upstream API key", { exact: true }).fill("sk-or-synthetic");
  await dialog.getByRole("button", { name: "Add source", exact: true }).click();
  await expect(dialog).toHaveCount(0);
  await expect(page.locator(".setup-pool-ready")).toBeVisible();
  await page.getByRole("button", { name: "Continue" }).click();
  await expect(page.getByRole("heading", { name: "What should use your pool?" })).toBeVisible();
  await page.getByRole("button", { name: "ChatGPT", exact: true }).click();
  await page.getByRole("button", { name: "Continue" }).click();
  const calls = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: Record<string, unknown> }> }).__TAURI_TEST_INVOKES__);
  expect(calls.find((call) => call.command === "create_local_source")?.args.input).toMatchObject({ name: "OpenRouter", baseUrl: "https://openrouter.ai/api/v1", wireApi: "chat_completions" });
  expect(calls.find((call) => call.command === "confirm_local_account_import")?.args.input).toMatchObject({ addToPool: true });
  expect(calls.find((call) => call.command === "set_local_pool_membership")?.args.input).toEqual({ accountIds: [], sourceIds: ["source_created_2"], inPool: true });
  expect(calls.map((call) => call.command)).toContain("attach_codex_to_local_gateway");
  expect(calls.map((call) => call.command)).not.toContain("launch_codex_source");
  await page.getByRole("button", { name: "Open application" }).click();
  await expect.poll(() => page.evaluate(() => localStorage.getItem("relay.mode"))).toBe("local");
});

test("repeated quick setup adds a custom API to the local pool from a previous direct mode", async ({ page }) => {
  await installTauriMock(page, { onboarding: false, locale: "en", populated: true, storedMode: "zenith" });
  await page.goto("/");
  await page.getByRole("button", { name: "Get started" }).click();
  await expect(page.getByRole("button", { name: /Computer/ })).toHaveAttribute("aria-pressed", "true");
  await page.getByRole("button", { name: "Continue" }).click();
  await page.getByRole("button", { name: "Add API source" }).click();
  const dialog = page.getByRole("dialog", { name: "Add API source" });
  await dialog.getByRole("radio", { name: /Custom API/ }).click();
  await dialog.getByLabel("Upstream API key", { exact: true }).fill("synthetic-custom-api-key");
  await expect(dialog.getByRole("button", { name: "Add source", exact: true })).toBeDisabled();
  await dialog.getByLabel("API address").fill("https://api.example.invalid/v1");
  await dialog.getByLabel("Name").fill("My API");
  await dialog.getByRole("button", { name: "Add source", exact: true }).click();
  await expect(dialog).toHaveCount(0);
  const calls = await page.evaluate(() => (window as unknown as {
    __TAURI_TEST_INVOKES__: Array<{ command: string; args: Record<string, unknown> }>;
  }).__TAURI_TEST_INVOKES__);
  expect(calls.find((call) => call.command === "create_local_source")?.args.input).toMatchObject({ name: "My API", baseUrl: "https://api.example.invalid/v1", apiKey: "synthetic-custom-api-key", protocolBindings: [] });
  expect(calls.find((call) => call.command === "set_local_pool_membership")?.args.input).toMatchObject({ sourceIds: ["source_created_2"], inPool: true });
  expect(calls.map((call) => call.command)).not.toContain("execute_remote_server_action");
  await page.getByRole("button", { name: "Continue" }).click();
  await page.getByRole("button", { name: "Continue" }).click();
  await page.getByRole("button", { name: "Open application" }).click();
  await expect.poll(() => page.evaluate(() => localStorage.getItem("relay.mode"))).toBe("local");
});


test("quick setup can switch to Russian without untranslated keys", async ({ page }) => {
  await installTauriMock(page, { onboarding: false, locale: "ru", populated: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Язык: Русский" }).click();
  await expect(page.getByRole("listbox", { name: "Язык" }).getByRole("option")).toHaveCount(2);
  await page.getByRole("option", { name: "English" }).click();
  await expect(page.getByRole("button", { name: "Get started" })).toBeVisible();
  await page.getByRole("button", { name: "Language: English" }).click();
  await page.getByRole("option", { name: "Русский" }).click();
  await expect(page.getByRole("button", { name: "Приступить" })).toBeVisible();
  await expect.poll(() => page.evaluate(() => localStorage.getItem("relay.language"))).toBe("ru");
  await page.reload();
  await expect(page.getByRole("button", { name: "Приступить" })).toBeVisible();
  await expect(page.locator("body")).not.toContainText(/(?:common|onboarding|modes)\.[a-z]/);
});

for (const theme of ["light", "dark"] as const) {
  for (const viewport of [{ width: 1160, height: 760 }, { width: 840, height: 560 }, { width: 390, height: 844 }] as const) {
    test(`onboarding layout ${theme} ${viewport.width}x${viewport.height}`, async ({ page }) => {
      await installTauriMock(page, { onboarding: false, locale: "ru", theme, populated: true });
      await page.setViewportSize(viewport);
      await page.goto("/");
      await expect(page.getByRole("heading", { name: "Zenith Relay" })).toBeVisible();
      await expect(page.locator("#splash-screen")).toHaveCount(0);
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= document.documentElement.clientWidth)).toBe(true);
      expect(await page.locator(".product-intro button").evaluateAll((buttons) => buttons.every((button) => button.scrollWidth <= button.clientWidth))).toBe(true);
      await page.screenshot({ path: `output/playwright/onboarding-intro-ru-${theme}-${viewport.width}x${viewport.height}.png` });
      await page.getByRole("button", { name: "Приступить" }).click();
      expect(await page.locator(".setup-body").evaluate((body) => body.scrollWidth <= body.clientWidth)).toBe(true);
      await expectSetupFrame(page);
      expect(await page.locator(".mode-options button").evaluateAll((buttons) => buttons.every((button) => {
        const rect = button.getBoundingClientRect();
        return rect.left >= 0 && rect.right <= innerWidth && button.scrollWidth <= button.clientWidth;
      }))).toBe(true);
      await page.screenshot({ path: `output/playwright/onboarding-mode-ru-${theme}-${viewport.width}x${viewport.height}.png` });
      await page.getByRole("button", { name: "Продолжить" }).click();
      await expect(page.getByRole("button", { name: /Импортировать учётные записи/ })).toBeVisible();
      await expectSetupFrame(page);
      expect(await page.locator(".setup-connect-options button").evaluateAll((buttons) => buttons.every((button) => button.scrollWidth <= button.clientWidth))).toBe(true);
      await page.screenshot({ path: `output/playwright/onboarding-local-ru-${theme}-${viewport.width}x${viewport.height}.png` });
      await page.getByRole("button", { name: "Добавить API-источник" }).click();
      const dialog = page.getByRole("dialog", { name: "Добавить API-источник" });
      await expectSetupFrame(page);
      expect(await dialog.locator(".api-provider-options button").evaluateAll((buttons) => buttons.every((button) => button.scrollWidth <= button.clientWidth))).toBe(true);
      await page.screenshot({ path: `output/playwright/onboarding-pool-api-ru-${theme}-${viewport.width}x${viewport.height}.png` });
      await dialog.getByRole("radio", { name: "Свой API", exact: true }).click();
      await dialog.getByLabel("Ключ внешнего API", { exact: true }).fill("synthetic-preview-key");
      await dialog.getByLabel("Адрес API", { exact: true }).fill("https://api.example.invalid/v1");
      await dialog.getByLabel("Название", { exact: true }).fill("Рабочий API");
      await expect(dialog.getByRole("button", { name: "Добавить источник", exact: true })).toBeEnabled();
      await expectSetupFrame(page);
      await page.screenshot({ path: `output/playwright/onboarding-pool-api-custom-ru-${theme}-${viewport.width}x${viewport.height}.png`, animations: "disabled" });
      await dialog.getByRole("button", { name: "Добавить источник", exact: true }).click();
      await expect(dialog).toHaveCount(0);
      await page.getByRole("button", { name: "Продолжить" }).click();
      await expect(page.locator(".client-options")).toBeVisible();
      await expectSetupFrame(page);
      await page.screenshot({ path: `output/playwright/onboarding-client-ru-${theme}-${viewport.width}x${viewport.height}.png` });
      await page.getByRole("button", { name: "Продолжить" }).click();
      await expect(page.getByRole("heading", { name: "Relay готов" })).toBeVisible();
      await expectSetupFrame(page);
      await page.screenshot({ path: `output/playwright/onboarding-ready-ru-${theme}-${viewport.width}x${viewport.height}.png` });
      for (let step = 4; step > 1; step -= 1) {
        await page.getByRole("button", { name: "Назад", exact: true }).click();
      }
      await page.getByRole("button", { name: /На сервере/ }).click();
      await page.getByRole("button", { name: "Продолжить" }).click();
      await expect(page.getByText("В разработке", { exact: true })).toBeVisible();
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= document.documentElement.clientWidth)).toBe(true);
      await page.screenshot({ path: `output/playwright/onboarding-hosted-ru-${theme}-${viewport.width}x${viewport.height}.png` });
    });
  }
}
