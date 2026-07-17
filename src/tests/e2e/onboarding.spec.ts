import { expect, test } from "@playwright/test";
import { emitTauriEvent, installTauriMock } from "./tauri-mock";

test("quick setup covers all three runtime choices", async ({ page }) => {
  await installTauriMock(page, { onboarding: false, locale: "en", populated: true });
  await page.goto("/");
  await expect(page.getByRole("heading", { name: "Zenith Relay" })).toBeVisible();
  await page.getByRole("button", { name: "Get started" }).click();
  await expect(page.getByRole("heading", { name: "Where should Zenith Relay run?" })).toBeVisible();
  await page.getByRole("button", { name: /On your server/ }).click();
  await page.getByRole("button", { name: "Continue" }).click();
  await page.getByLabel("Server address").fill("https://relay.example.invalid");
  await page.getByLabel("Management token").fill("synthetic-management-token-000000");
  await page.getByRole("button", { name: "Continue" }).click();
  await expect(page.getByRole("heading", { name: "Connection check" })).toBeVisible();
  await expect(page.locator(".check-stages strong")).toHaveText(["Ready", "Ready", "Ready", "Ready"]);
  await page.screenshot({ path: "output/playwright/onboarding-server-1160x760.png" });
});

test("local quick setup verifies runtime and applies ChatGPT only after explicit choices", async ({ page }) => {
  await installTauriMock(page, { onboarding: false, locale: "en", populated: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Get started" }).click();
  await page.getByRole("button", { name: "Continue" }).click();
  await page.getByRole("button", { name: "Sign in" }).click();
  await expect(page.getByText("Waiting for sign-in", { exact: true })).toBeVisible();
  await emitTauriEvent(page, "relay-oauth-status", { loginId: "oauth_synthetic", status: "callback_received" });
  await expect(page.getByText("Connection is ready and will be checked next.")).toBeVisible();
  await page.getByRole("button", { name: "Continue" }).click();
  await expect(page.locator(".check-stages strong")).toHaveText(["Ready", "Ready", "Ready", "Ready"]);
  await page.getByRole("button", { name: "Continue" }).click();
  await page.getByRole("button", { name: "ChatGPT" }).click();
  await page.getByRole("button", { name: "Continue" }).click();
  await page.getByRole("dialog", { name: "Confirm action" }).getByRole("button", { name: "Cancel" }).click();
  await expect(page.getByRole("heading", { name: "Relay is ready" })).toBeVisible();
  await expect(page.getByText("http://127.0.0.1:14998/v1")).toHaveCount(0);
  const calls = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: Record<string, unknown> }> }).__TAURI_TEST_INVOKES__);
  expect(calls.map((call) => call.command)).toEqual(expect.arrayContaining(["complete_codex_oauth", "set_local_pool_membership", "get_local_runtime_state", "attach_codex_to_local_gateway"]));
  expect(calls.find((call) => call.command === "set_local_pool_membership")?.args).toEqual({ input: { accountIds: ["account_synthetic"], sourceIds: [], inPool: true } });
  expect(calls.findLast((call) => call.command === "attach_codex_to_local_gateway")?.args).toEqual({ keyId: "key_synthetic", boundOauthAccountId: null });
});

test("local quick setup imports through the unified dialog and selects pool membership", async ({ page }) => {
  await installTauriMock(page, { onboarding: false, locale: "en", populated: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Get started" }).click();
  await page.getByRole("button", { name: "Continue" }).click();
  await page.getByRole("button", { name: /Import accounts/ }).click();
  const dialog = page.getByRole("dialog", { name: "Import accounts" });
  await dialog.getByRole("button", { name: "Choose JSON files" }).click();
  await expect(dialog.getByLabel("Add selected to pool after import")).toBeChecked();
  await dialog.getByRole("button", { name: /Import 2 account/ }).click();
  await expect(dialog).not.toBeVisible();
  const call = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: Record<string, unknown> }> }).__TAURI_TEST_INVOKES__.findLast((item) => item.command === "confirm_local_account_import"));
  expect(call?.args.input).toMatchObject({ addToPool: true });
});

test("ready API step can add OpenRouter as a checked local source", async ({ page }) => {
  await installTauriMock(page, { onboarding: false, locale: "en", mode: "local", populated: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Get started" }).click();
  await page.getByRole("button", { name: /Choose API/ }).click();
  await page.getByRole("button", { name: "Continue" }).click();
  await page.getByRole("radio", { name: /OpenRouter/ }).click();
  await page.getByLabel("Upstream API key").fill("sk-or-synthetic");
  await page.getByRole("button", { name: "Continue" }).click();
  await expect(page.locator(".check-stages strong")).toHaveText(["Ready", "Ready", "Ready", "Ready"]);
  const calls = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: Record<string, unknown> }> }).__TAURI_TEST_INVOKES__);
  expect(calls.find((call) => call.command === "create_local_source")?.args.input).toMatchObject({ name: "OpenRouter", baseUrl: "https://openrouter.ai/api/v1", wireApi: "chat_completions" });
  expect(calls.find((call) => call.command === "set_local_pool_membership")?.args).toEqual({ input: { accountIds: [], sourceIds: ["source_created_2"], inPool: true } });
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
  await expect(page.locator("body")).not.toContainText(/(?:common|onboarding|modes)\.[a-z]/);
});

for (const theme of ["light", "dark"] as const) {
  for (const viewport of [{ width: 1160, height: 760 }, { width: 840, height: 560 }] as const) {
    test(`onboarding layout ${theme} ${viewport.width}x${viewport.height}`, async ({ page }) => {
      await installTauriMock(page, { onboarding: false, locale: "ru", theme, populated: true });
      await page.setViewportSize(viewport);
      await page.goto("/");
      await expect(page.getByRole("heading", { name: "Zenith Relay" })).toBeVisible();
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= document.documentElement.clientWidth)).toBe(true);
      expect(await page.locator(".product-intro button").evaluateAll((buttons) => buttons.every((button) => button.scrollWidth <= button.clientWidth))).toBe(true);
      await page.screenshot({ path: `output/playwright/onboarding-intro-ru-${theme}-${viewport.width}x${viewport.height}.png` });
      await page.getByRole("button", { name: "Приступить" }).click();
      expect(await page.locator(".setup-body").evaluate((body) => body.scrollWidth <= body.clientWidth)).toBe(true);
      expect(await page.locator(".mode-options button").evaluateAll((buttons) => buttons.every((button) => {
        const rect = button.getBoundingClientRect();
        return rect.left >= 0 && rect.right <= innerWidth && button.scrollWidth <= button.clientWidth;
      }))).toBe(true);
      await page.screenshot({ path: `output/playwright/onboarding-mode-ru-${theme}-${viewport.width}x${viewport.height}.png` });
      await page.getByRole("button", { name: "Продолжить" }).click();
      expect(await page.locator(".setup-connect-options button").evaluateAll((buttons) => buttons.every((button) => button.scrollWidth <= button.clientWidth))).toBe(true);
      await page.screenshot({ path: `output/playwright/onboarding-local-ru-${theme}-${viewport.width}x${viewport.height}.png` });
      await page.getByRole("button", { name: "Назад" }).click();
      await page.getByRole("button", { name: /Выбор API/ }).click();
      await page.getByRole("button", { name: "Продолжить" }).click();
      expect(await page.locator(".api-provider-options button").evaluateAll((buttons) => buttons.every((button) => button.scrollWidth <= button.clientWidth))).toBe(true);
      await page.screenshot({ path: `output/playwright/onboarding-api-ru-${theme}-${viewport.width}x${viewport.height}.png` });
    });
  }
}
