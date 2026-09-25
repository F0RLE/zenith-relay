import { expect, test, type Page } from "../bun-playwright";
import { emitTauriEvent, installTauriMock } from "./tauri-mock";

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

test("quick setup chooses where the shared pool runs", async ({ page }) => {
  await installTauriMock(page, { onboarding: false, locale: "en", populated: true });
  await page.goto("/");
  await expect(page.getByRole("heading", { name: "Zenith Relay" })).toBeVisible();
  await page.getByRole("button", { name: "Get started" }).click();
  await expect(page.getByRole("heading", { name: "Where should Zenith Relay run?" })).toBeVisible();
  await expect(page.locator(".mode-options button")).toHaveCount(2);
  await expect(page.getByRole("button", { name: /Choose API/ })).toHaveCount(0);
  await page.getByRole("button", { name: /On your server/ }).click();
  await page.getByRole("button", { name: "Continue" }).click();
  await page.getByLabel("Server address").fill("https://relay.example.invalid");
  await page.getByLabel("Management token").fill("synthetic-management-token-000000");
  await page.getByRole("button", { name: "Continue" }).click();
  await expect(page.getByRole("heading", { name: "What should use your pool?" })).toBeVisible();
  await page.screenshot({ path: "output/playwright/onboarding-server-1160x760.png" });
});

test("skipping repeated setup preserves a previously selected direct API mode", async ({ page }) => {
  await installTauriMock(page, { onboarding: false, locale: "en", mode: "zenith", populated: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Skip setup" }).click();
  await expect(page.getByRole("button", { name: "Mode: Choose API" })).toBeVisible();
  await expect.poll(() => page.evaluate(() => localStorage.getItem("relay.mode"))).toBe("zenith");
});

test("local quick setup verifies runtime and applies ChatGPT only after explicit choices", async ({ page }) => {
  await installTauriMock(page, { onboarding: false, locale: "en", populated: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Get started" }).click();
  await page.getByRole("button", { name: "Continue" }).click();
  await page.setViewportSize({ width: 840, height: 560 });
  await page.getByRole("button", { name: "Sign in" }).click();
  await expect(page.getByText("Waiting for sign-in", { exact: true })).toBeVisible();
  await expect(page.locator(".setup-connect-options")).toHaveCount(0);
  await expect(page.locator(".setup-oauth-pending")).toBeVisible();
  await expect(page.getByRole("link", { name: "Open in browser" })).toBeVisible();
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
  await installTauriMock(page, { onboarding: false, locale: "en", populated: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Get started" }).click();
  await page.getByRole("button", { name: "Continue" }).click();
  await page.getByRole("button", { name: /Import accounts/ }).click();
  const dialog = page.getByRole("dialog", { name: "Import accounts" });
  await dialog.getByRole("button", { name: /Choose account files/ }).click();
  await expect(dialog.getByLabel("Add selected to pool after import")).toBeChecked();
  await dialog.getByRole("button", { name: /Import 2 account/ }).click();
  await expect(dialog).not.toBeVisible();
  const call = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: Record<string, unknown> }> }).__TAURI_TEST_INVOKES__.findLast((item) => item.command === "confirm_local_account_import"));
  expect(call?.args.input).toMatchObject({ addToPool: true });
});

test("local quick setup imports the current ChatGPT profile and allows adding an API before continuing", async ({ page }) => {
  await installTauriMock(page, { onboarding: false, locale: "en", populated: true, gatewayRunning: false, importConfirmDelayMs: 250 });
  await page.goto("/");
  await page.getByRole("button", { name: "Get started" }).click();
  await page.getByRole("button", { name: "Continue" }).click();
  await expect(page.getByRole("button", { name: /Import current profile/ })).toBeVisible();
  await page.getByRole("button", { name: /Import current profile/ }).click();
  const status = page.locator(".setup-current-profile-status");
  await expect(status).toContainText("Importing current profile");
  await expect(page.getByRole("dialog", { name: "Import accounts" })).toHaveCount(0);
  await expect(status).toContainText("Profile imported");
  await expect(page.getByRole("button", { name: "Add API source" })).toBeVisible();
  await expect(page.getByRole("button", { name: "Continue" })).toBeEnabled();
  expect(await status.evaluate((element) => {
    const rect = element.getBoundingClientRect();
    return rect.left >= 0 && rect.right <= innerWidth && element.scrollWidth <= element.clientWidth;
  })).toBe(true);
  await page.screenshot({ path: "output/playwright/onboarding-current-profile-success-1160x760.png" });
  await page.setViewportSize({ width: 840, height: 560 });
  await expect(status).toBeVisible();
  expect(await status.evaluate((element) => {
    const rect = element.getBoundingClientRect();
    return rect.left >= 0 && rect.right <= innerWidth && element.scrollWidth <= element.clientWidth;
  })).toBe(true);
  await page.screenshot({ path: "output/playwright/onboarding-current-profile-success-840x560.png" });
  const calls = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: Record<string, unknown> }> }).__TAURI_TEST_INVOKES__);
  expect(calls.map((call) => call.command)).toEqual(expect.arrayContaining(["preview_current_codex_account_import", "confirm_local_account_import", "get_local_runtime_state", "start_local_gateway"]));
  expect(calls.find((call) => call.command === "confirm_local_account_import")?.args.input).toMatchObject({
    sessionId: "current_codex_profile",
    addToPool: true,
  });
  await page.getByRole("button", { name: "Continue" }).click();
  await expect(page.getByRole("heading", { name: "What should use your pool?" })).toBeVisible();
});

test("current profile import keeps setup on the connection step when an item fails", async ({ page }) => {
  await installTauriMock(page, { onboarding: false, locale: "en", populated: true, importResult: "item_failure" });
  await page.goto("/");
  await page.getByRole("button", { name: "Get started" }).click();
  await page.getByRole("button", { name: "Continue" }).click();
  await page.getByRole("button", { name: /Import current profile/ }).click();
  await expect(page.locator(".setup-current-profile-status.failed")).toContainText("Could not import the current profile");
  await expect(page.getByRole("button", { name: "Retry" })).toBeVisible();
  await expectSetupFrame(page);
  await page.screenshot({ path: "output/playwright/onboarding-import-error-1160x760.png" });
  await expect(page.getByRole("heading", { name: "What should use your pool?" })).toHaveCount(0);
  const calls = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string }> }).__TAURI_TEST_INVOKES__);
  expect(calls.map((call) => call.command)).toContain("cancel_local_account_import");
});

test("leaving the local connection step cancels a delayed current-profile preview before confirmation", async ({ page }) => {
  await installTauriMock(page, { onboarding: false, locale: "en", populated: true, importPreviewDelayMs: 1_000 });
  await page.goto("/");
  await page.getByRole("button", { name: "Get started" }).click();
  await page.getByRole("button", { name: "Continue" }).click();
  await page.getByRole("button", { name: /Import current profile/ }).click();
  await expect.poll(() => page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string }> }).__TAURI_TEST_INVOKES__.some((call) => call.command === "preview_current_codex_account_import"))).toBe(true);
  await page.getByRole("button", { name: "Back" }).click();
  await expect(page.getByRole("heading", { name: "Where should Zenith Relay run?" })).toBeVisible();
  await expect.poll(() => page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string }> }).__TAURI_TEST_INVOKES__.some((call) => call.command === "cancel_local_account_import"))).toBe(true);
  const calls = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string }> }).__TAURI_TEST_INVOKES__);
  expect(calls.map((call) => call.command)).toEqual(expect.arrayContaining(["preview_current_codex_account_import", "cancel_local_account_import"]));
  expect(calls.map((call) => call.command)).not.toContain("confirm_local_account_import");
});

test("current profile action stays hidden when no usable ChatGPT profile exists", async ({ page }) => {
  await installTauriMock(page, { onboarding: false, locale: "ru", theme: "dark", populated: true, currentProfileAvailable: false });
  await page.goto("/");
  await page.getByRole("button", { name: "Приступить" }).click();
  await page.getByRole("button", { name: "Продолжить" }).click();
  await expect(page.getByRole("button", { name: /Импортировать текущий профиль/ })).toHaveCount(0);
  await expect(page.locator(".setup-connect-options button")).toHaveCount(3);
  await expect(page.locator(".setup-connected")).toHaveCount(0);
  await page.screenshot({ path: "output/playwright/onboarding-step-2-no-profile-ru-dark-1160x760.png" });
});

test("all onboarding steps keep progress and actions in one bounded workspace", async ({ page }) => {
  await installTauriMock(page, { onboarding: false, locale: "ru", theme: "dark", populated: true });
  await page.setViewportSize({ width: 1160, height: 760 });
  await page.goto("/");
  await page.getByRole("button", { name: "Приступить" }).click();

  await expectSetupFrame(page);
  await page.screenshot({ path: "output/playwright/onboarding-step-1-mode-ru-dark-1160x760.png" });
  await page.getByRole("button", { name: "Продолжить" }).click();
  await expect(page.getByRole("button", { name: /Импортировать текущий профиль/ })).toBeVisible();
  await expectSetupFrame(page);
  await page.screenshot({ path: "output/playwright/onboarding-step-2-connection-ru-dark-1160x760.png" });

  await page.getByRole("button", { name: /Импортировать текущий профиль/ }).click();
  await expect(page.locator(".setup-current-profile-status.complete")).toBeVisible();
  await page.getByRole("button", { name: "Продолжить" }).click();
  await expect(page.locator(".client-options")).toBeVisible();
  await expectSetupFrame(page);
  await page.screenshot({ path: "output/playwright/onboarding-step-3-client-ru-dark-1160x760.png" });
  await page.getByRole("button", { name: "Продолжить" }).click();
  await expectSetupFrame(page);
  await page.screenshot({ path: "output/playwright/onboarding-step-4-ready-ru-dark-1160x760.png" });
});

test("local quick setup combines an account and an OpenRouter source in the pool", async ({ page }) => {
  await installTauriMock(page, { onboarding: false, locale: "en", mode: "local", populated: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Get started" }).click();
  await page.getByRole("button", { name: "Continue" }).click();
  await page.getByRole("button", { name: /Import current profile/ }).click();
  await expect(page.locator(".setup-current-profile-status.complete")).toBeVisible();
  await page.getByRole("button", { name: "Add API source" }).click();
  const dialog = page.getByRole("dialog", { name: "Add API source" });
  await dialog.getByRole("radio", { name: /OpenRouter/ }).click();
  await dialog.getByLabel("Upstream API key").fill("sk-or-synthetic");
  await dialog.getByRole("button", { name: "Save" }).click();
  await expect(dialog).toHaveCount(0);
  await expect(page.locator(".setup-current-profile-status.complete")).toBeVisible();
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
  await installTauriMock(page, { onboarding: false, locale: "en", mode: "zenith", populated: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Get started" }).click();
  await expect(page.getByRole("button", { name: /Computer/ })).toHaveAttribute("aria-pressed", "true");
  await page.getByRole("button", { name: "Continue" }).click();
  await page.getByRole("button", { name: "Add API source" }).click();
  const dialog = page.getByRole("dialog", { name: "Add API source" });
  await dialog.getByRole("radio", { name: /Custom API/ }).click();
  await dialog.getByLabel("Upstream API key").fill("synthetic-custom-api-key");
  await expect(dialog.getByRole("button", { name: "Save" })).toBeDisabled();
  await dialog.getByLabel("API address").fill("https://api.example.invalid/v1");
  await dialog.getByLabel("Name").fill("My API");
  await dialog.getByRole("button", { name: "Save" }).click();
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

test("remote quick setup requires explicit consent for plain HTTP", async ({ page }) => {
  await installTauriMock(page, { onboarding: false, locale: "en", populated: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Get started" }).click();
  await page.getByRole("button", { name: /On your server/ }).click();
  await page.getByRole("button", { name: "Continue" }).click();
  await page.getByLabel("Server address").fill("http://127.0.0.1:14999");
  await page.getByLabel("Management token").fill("synthetic-management-token-000000");
  await expect(page.getByRole("button", { name: "Continue" })).toBeDisabled();
  await page.getByLabel("Allow this unencrypted HTTP server connection.").check();
  await expect(page.getByRole("button", { name: "Continue" })).toBeEnabled();
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
      await expect(page.getByRole("button", { name: /Импортировать текущий профиль/ })).toBeVisible();
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
      await expect(dialog.getByRole("button", { name: "Сохранить" })).toBeEnabled();
      await expectSetupFrame(page);
      await page.screenshot({ path: `output/playwright/onboarding-pool-api-custom-ru-${theme}-${viewport.width}x${viewport.height}.png`, animations: "disabled" });
      await dialog.getByRole("button", { name: "Сохранить" }).click();
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
      await page.getByRole("button", { name: /На своём сервере/ }).click();
      await page.getByRole("button", { name: "Продолжить" }).click();
      await page.getByLabel("Адрес сервера", { exact: true }).fill("https://relay.example.invalid");
      await page.getByLabel("Токен управления", { exact: true }).fill("synthetic-management-token-000000");
      await expect(page.getByRole("button", { name: "Продолжить" })).toBeEnabled();
      await expectSetupFrame(page);
      await page.screenshot({ path: `output/playwright/onboarding-remote-ru-${theme}-${viewport.width}x${viewport.height}.png` });
      await page.getByRole("button", { name: "Продолжить" }).click();
      await expect(page.locator(".client-options")).toBeVisible();
    });
  }
}
