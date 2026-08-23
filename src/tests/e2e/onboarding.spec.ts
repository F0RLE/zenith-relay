import { expect, test } from "../bun-playwright";
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
  await expect(page.getByRole("heading", { name: "What should use this endpoint?" })).toBeVisible();
  await page.screenshot({ path: "output/playwright/onboarding-server-1160x760.png" });
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
  await expect(page.getByRole("heading", { name: "What should use this endpoint?" })).toBeVisible();
  await page.getByRole("button", { name: "ChatGPT" }).click();
  await page.getByRole("button", { name: "Continue" }).click();
  await expect(page.getByRole("heading", { name: "Relay is ready" })).toBeVisible();
  await expect(page.getByText("http://127.0.0.1:14998/v1")).toHaveCount(0);
  const calls = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: Record<string, unknown> }> }).__TAURI_TEST_INVOKES__);
  expect(calls.map((call) => call.command)).toEqual(expect.arrayContaining(["complete_codex_oauth", "set_local_pool_membership", "get_local_runtime_state", "attach_codex_to_local_gateway"]));
  expect(calls.find((call) => call.command === "set_local_pool_membership")?.args).toEqual({ input: { accountIds: ["account_synthetic"], sourceIds: [], inPool: true } });
  expect(calls.findLast((call) => call.command === "attach_codex_to_local_gateway")?.args).toEqual({ boundOauthAccountId: null });
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

test("local quick setup imports the current ChatGPT profile directly and advances to the client choice", async ({ page }) => {
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
  await expect(status).toContainText(/Continuing setup in [1-3]/);
  await expect(page.getByRole("button", { name: "Continue" })).toBeDisabled();
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
  await expect(page.getByRole("heading", { name: "What should use this endpoint?" })).toBeVisible({ timeout: 4_000 });
});

test("current profile import keeps setup on the connection step when an item fails", async ({ page }) => {
  await installTauriMock(page, { onboarding: false, locale: "en", populated: true, importResult: "item_failure" });
  await page.goto("/");
  await page.getByRole("button", { name: "Get started" }).click();
  await page.getByRole("button", { name: "Continue" }).click();
  await page.getByRole("button", { name: /Import current profile/ }).click();
  await expect(page.locator(".setup-current-profile-status.failed")).toContainText("Could not import the current profile");
  await expect(page.getByRole("button", { name: "Retry" })).toBeVisible();
  await expect(page.getByRole("heading", { name: "What should use this endpoint?" })).toHaveCount(0);
  const calls = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string }> }).__TAURI_TEST_INVOKES__);
  expect(calls.map((call) => call.command)).toContain("cancel_local_account_import");
});

test("leaving the local connection step cancels a delayed current-profile preview before confirmation", async ({ page }) => {
  await installTauriMock(page, { onboarding: false, locale: "en", populated: true, importPreviewDelayMs: 250 });
  await page.goto("/");
  await page.getByRole("button", { name: "Get started" }).click();
  await page.getByRole("button", { name: "Continue" }).click();
  await page.getByRole("button", { name: /Import current profile/ }).click();
  await expect(page.locator(".setup-current-profile-status")).toContainText("Importing current profile");
  await page.getByRole("button", { name: "Back" }).click();
  await expect(page.getByRole("heading", { name: "Where should Zenith Relay run?" })).toBeVisible();
  await page.waitForTimeout(350);
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
  await expect(page.locator(".setup-connect-options button")).toHaveCount(2);
  await expect(page.locator(".setup-connected")).toHaveCount(0);
  await page.screenshot({ path: "output/playwright/onboarding-step-2-no-profile-ru-dark-1160x760.png" });
});

test("all onboarding steps are centered and captured", async ({ page }) => {
  await installTauriMock(page, { onboarding: false, locale: "ru", theme: "dark", populated: true });
  await page.setViewportSize({ width: 1160, height: 760 });
  await page.goto("/");
  await page.getByRole("button", { name: "Приступить" }).click();

  const expectCenteredStep = async () => {
    const layout = await page.locator(".setup-step").evaluate((step) => {
      const children = [...step.children].filter((child): child is HTMLElement => child instanceof HTMLElement && child.offsetParent !== null);
      const first = children[0].getBoundingClientRect();
      const last = children.at(-1)!.getBoundingClientRect();
      const bounds = step.getBoundingClientRect();
      const heading = step.querySelector<HTMLElement>(".setup-heading");
      return {
        delta: Math.abs((first.top + last.bottom) / 2 - (bounds.top + bounds.bottom) / 2),
        headingAlign: heading ? getComputedStyle(heading).textAlign : "center",
      };
    });
    expect(layout.delta).toBeLessThanOrEqual(3);
    expect(layout.headingAlign).toBe("center");
  };

  await expectCenteredStep();
  await page.screenshot({ path: "output/playwright/onboarding-step-1-mode-ru-dark-1160x760.png" });
  await page.getByRole("button", { name: "Продолжить" }).click();
  await expect(page.getByRole("button", { name: /Импортировать текущий профиль/ })).toBeVisible();
  await expectCenteredStep();
  await page.screenshot({ path: "output/playwright/onboarding-step-2-connection-ru-dark-1160x760.png" });

  await page.getByRole("button", { name: /Импортировать текущий профиль/ }).click();
  await expect(page.locator(".client-options")).toBeVisible({ timeout: 4_000 });
  await expectCenteredStep();
  await page.screenshot({ path: "output/playwright/onboarding-step-3-client-ru-dark-1160x760.png" });
  await page.getByRole("button", { name: "Продолжить" }).click();
  await page.screenshot({ path: "output/playwright/onboarding-step-4-ready-ru-dark-1160x760.png" });
});

test("Choose API setup saves and launches an OpenRouter source directly", async ({ page }) => {
  await installTauriMock(page, { onboarding: false, locale: "en", mode: "local", populated: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Get started" }).click();
  await page.getByRole("button", { name: /Choose API/ }).click();
  await page.getByRole("button", { name: "Continue" }).click();
  await page.getByRole("radio", { name: /OpenRouter/ }).click();
  await page.getByLabel("API key").fill("sk-or-synthetic");
  await page.getByRole("button", { name: "Continue" }).click();
  await expect(page.getByRole("heading", { name: "What should use this endpoint?" })).toBeVisible();
  await page.getByRole("button", { name: "ChatGPT", exact: true }).click();
  await page.getByRole("button", { name: "Continue" }).click();
  await expect.poll(() => page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string }> }).__TAURI_TEST_INVOKES__.some((call) => call.command === "launch_codex_source"))).toBe(true);
  const calls = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: Record<string, unknown> }> }).__TAURI_TEST_INVOKES__);
  expect(calls.find((call) => call.command === "create_local_source")?.args.input).toMatchObject({ name: "OpenRouter", baseUrl: "https://openrouter.ai/api/v1", wireApi: "responses" });
  expect(calls.find((call) => call.command === "launch_codex_source")?.args).toEqual({ sourceId: "source_created_2" });
  expect(calls.map((call) => call.command)).not.toContain("set_local_pool_membership");
  await expect.poll(() => page.evaluate(() => localStorage.getItem("relay.directSourceId"))).toBe("source_created_2");
  await page.getByRole("button", { name: "Open application" }).click();
  await expect.poll(() => page.evaluate(() => localStorage.getItem("relay.mode"))).toBe("zenith");
});

test("Choose API quick setup focuses on the selected service and its key", async ({ page }) => {
  await installTauriMock(page, { onboarding: false, locale: "en", populated: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Get started" }).click();
  await page.getByRole("button", { name: /Choose API/ }).click();
  await page.getByRole("button", { name: "Continue" }).click();

  await expect(page.getByRole("heading", { name: "Choose an API" })).toHaveCount(1);
  await expect(page.locator(".api-provider-intro")).toHaveCount(0);
  await expect(page.getByRole("radiogroup", { name: "API provider" })).toBeVisible();

  await page.getByRole("radio", { name: /Zenith API/ }).click();
  await expect(page.getByRole("heading", { name: "Connect Zenith API" })).toBeVisible();
  await expect(page.locator(".source-routing-disclosure")).toHaveCount(0);
  await expect(page.getByLabel("API key")).toBeVisible();

  await page.getByRole("button", { name: "Edit" }).click();
  await page.getByRole("radio", { name: /Custom API/ }).click();
  await expect(page.getByText("Enter a name, endpoint, and API key.")).toBeVisible();
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
      const centeredMode = await page.locator(".setup-body").evaluate((body) => {
        const heading = body.querySelector<HTMLElement>(".setup-heading")!;
        const options = body.querySelector<HTMLElement>(".mode-options")!;
        const bodyBox = body.getBoundingClientRect();
        const headingBox = heading.getBoundingClientRect();
        const optionsBox = options.getBoundingClientRect();
        return {
          delta: Math.abs((headingBox.top + optionsBox.bottom) / 2 - (bodyBox.top + bodyBox.bottom) / 2),
          headingAlign: getComputedStyle(heading).textAlign,
        };
      });
      expect(centeredMode.headingAlign).toBe("center");
      expect(centeredMode.delta).toBeLessThanOrEqual(2);
      expect(await page.locator(".mode-options button").evaluateAll((buttons) => buttons.every((button) => {
        const rect = button.getBoundingClientRect();
        return rect.left >= 0 && rect.right <= innerWidth && button.scrollWidth <= button.clientWidth;
      }))).toBe(true);
      await page.screenshot({ path: `output/playwright/onboarding-mode-ru-${theme}-${viewport.width}x${viewport.height}.png` });
      await page.getByRole("button", { name: "Продолжить" }).click();
      await expect(page.getByRole("button", { name: /Импортировать текущий профиль/ })).toBeVisible();
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
