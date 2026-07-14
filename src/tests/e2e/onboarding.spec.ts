import { expect, test } from "@playwright/test";
import { emitTauriEvent, installTauriMock } from "./tauri-mock";

test("quick setup covers all three runtime choices", async ({ page }) => {
  await installTauriMock(page, { onboarding: false, locale: "en", populated: true });
  await page.goto("/");
  await expect(page.getByRole("heading", { name: "Zenith Relay" })).toBeVisible();
  await page.getByRole("button", { name: "Get started" }).click();
  await expect(page.getByRole("heading", { name: "Where should Zenith Relay run?" })).toBeVisible();
  await page.getByRole("button", { name: /My server/ }).click();
  await page.getByRole("button", { name: "Continue" }).click();
  await page.getByLabel("Server address").fill("https://relay.example.invalid");
  await page.getByLabel("Management token").fill("synthetic-management-token-000000");
  await page.getByRole("button", { name: "Continue" }).click();
  await expect(page.getByRole("heading", { name: "Connection check" })).toBeVisible();
  await expect(page.locator(".check-stages strong")).toHaveText(["Ready", "Ready", "Ready", "Ready"]);
  await page.screenshot({ path: "output/playwright/onboarding-server-1160x760.png" });
});

test("local quick setup verifies runtime and applies Codex only after explicit choices", async ({ page }) => {
  await installTauriMock(page, { onboarding: false, locale: "en", populated: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Get started" }).click();
  await page.getByRole("button", { name: "Continue" }).click();
  await page.getByRole("button", { name: "Sign in" }).click();
  await expect(page.getByText("Waiting for sign-in", { exact: true })).toBeVisible();
  await emitTauriEvent(page, "relay-oauth-status", { loginId: "oauth_synthetic", status: "callback_received" });
  await page.getByLabel("Create a persistent local gateway key if no key exists.").check();
  await page.getByRole("button", { name: "Continue" }).click();
  await expect(page.locator(".check-stages strong")).toHaveText(["Ready", "Ready", "Ready", "Ready"]);
  await page.getByRole("button", { name: "Continue" }).click();
  await page.getByRole("button", { name: "Codex" }).click();
  await page.getByRole("button", { name: "Continue" }).click();
  await expect(page.getByText("http://127.0.0.1:14998/v1")).toBeVisible();
  const calls = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: Record<string, unknown> }> }).__TAURI_TEST_INVOKES__);
  expect(calls.map((call) => call.command)).toEqual(expect.arrayContaining(["complete_codex_oauth", "get_local_runtime_state", "attach_codex_to_local_gateway"]));
  expect(calls.findLast((call) => call.command === "attach_codex_to_local_gateway")?.args).toEqual({ keyId: "key_synthetic", boundOauthAccountId: null });
});

test("remote quick setup requires explicit consent for plain HTTP", async ({ page }) => {
  await installTauriMock(page, { onboarding: false, locale: "en", populated: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Get started" }).click();
  await page.getByRole("button", { name: /My server/ }).click();
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
  await expect(page.getByRole("button", { name: "Приступить" })).toBeVisible();
  await expect(page.locator("body")).not.toContainText(/(?:common|onboarding|modes)\.[a-z]/);
});
