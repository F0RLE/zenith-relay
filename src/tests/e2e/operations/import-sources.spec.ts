import type { Locator } from "@playwright/test";
import { expect, test } from "../../bun-playwright";
import { emitTauriEvent, installTauriMock } from "../tauri-mock";

async function expectSavingStatusCentered(dialog: Locator) {
  const content = dialog.locator(".oauth-completing-status > svg, .oauth-completing-status > span");
  await expect(content).toHaveCount(2);
  await expect.poll(async () => dialog.evaluate((element) => {
    const modal = element.getBoundingClientRect();
    const parts = Array.from(element.querySelectorAll(".oauth-completing-status > svg, .oauth-completing-status > span")).map((part) => {
      const rect = part.getBoundingClientRect();
      const style = getComputedStyle(part);
      // Measure the spinner's layout box, independent of its animated rotation.
      const width = part.tagName.toLowerCase() === "svg" ? parseFloat(style.width) : rect.width;
      const height = part.tagName.toLowerCase() === "svg" ? parseFloat(style.height) : rect.height;
      return { left: rect.x + (rect.width - width) / 2, top: rect.y + (rect.height - height) / 2, width, height };
    });
    const left = Math.min(...parts.map((part) => part.left));
    const right = Math.max(...parts.map((part) => part.left + part.width));
    const top = Math.min(...parts.map((part) => part.top));
    const bottom = Math.max(...parts.map((part) => part.top + part.height));
    return Math.max(Math.abs((left + right) / 2 - (modal.x + modal.width / 2)), Math.abs((top + bottom) / 2 - (modal.y + modal.height / 2)));
  }), { message: "Saving text and spinner should be centered together within the dialog" }).toBeLessThanOrEqual(1);
}

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

test("OAuth callback saves the account without another setup dialog or implicit pool membership", async ({ page }) => {
  await installTauriMock(page, { locale: "en", mode: "local", populated: false, proxyCount: 1 });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await page.getByRole("button", { name: "Sign in", exact: true }).first().click();
  const dialog = page.getByRole("dialog", { name: "Sign in" });
  await expect(dialog.getByRole("timer")).toBeVisible();

  await emitTauriEvent(page, "relay-oauth-status", { loginId: "oauth_synthetic", status: "callback_received" });

  await expect(dialog).toHaveCount(0);
  await expect(page.locator(".global-feedback.success")).toHaveText("Account added.");
  await expect(page.getByRole("dialog")).toHaveCount(0);
  await expect(page.locator(".account-card").filter({ hasText: "Personal Plus" })).toBeVisible();
  const calls = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string }> }).__TAURI_TEST_INVOKES__);
  expect(calls.map((call) => call.command)).toContain("complete_codex_oauth");
  expect(calls.some((call) => call.command === "set_local_pool_membership")).toBe(false);
  expect(calls.some((call) => call.command === "assign_free_local_account_proxies")).toBe(false);
});

test("BPS callback replaces sign-in controls with one saving status and dismisses on completion", async ({ page }, testInfo) => {
  await installTauriMock(page, { locale: "en", mode: "local", populated: false, oauthCompletionDeferred: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await page.getByRole("button", { name: "Sign in", exact: true }).first().click();
  const signIn = page.getByRole("dialog", { name: "Sign in" });
  await signIn.getByText("Excel / Basis Points", { exact: true }).click();
  await expect(signIn.getByRole("button", { name: "Open sign-in window", exact: true })).toBeEnabled();

  await emitTauriEvent(page, "relay-oauth-status", { loginId: "oauth_synthetic", status: "callback_received" });
  const saving = page.getByRole("dialog", { name: "Saving account…" });
  await expect(saving).toBeVisible();
  await expect(saving.getByRole("status")).toHaveText("Saving account…");
  await expect(saving.getByRole("button")).toHaveCount(0);
  await expect(saving.getByRole("radio")).toHaveCount(0);
  await expect(saving.getByRole("timer")).toHaveCount(0);
  await expectSavingStatusCentered(saving);
  await page.screenshot({ path: testInfo.outputPath("oauth-saving-en.png") });
  await page.keyboard.press("Escape");
  await expect(saving).toBeVisible();

  await emitTauriEvent(page, "relay-oauth-status", { loginId: "oauth_synthetic", status: "callback_received" });
  const completions = () => page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string }> }).__TAURI_TEST_INVOKES__.filter((call) => call.command === "complete_codex_oauth").length);
  await expect.poll(completions).toBe(1);
  await page.evaluate(() => window.dispatchEvent(new Event("relay-test-finish-oauth")));

  await expect(page.getByRole("dialog")).toHaveCount(0);
  await expect(page.locator(".global-feedback.success")).toHaveText("Account added.");
  expect(await completions()).toBe(1);
});

test("Russian saving status fits a compact dark window and closes before a slow snapshot refresh", async ({ page }, testInfo) => {
  await installTauriMock(page, { locale: "ru", theme: "dark", mode: "local", populated: false, oauthCompletionDeferred: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Подключения", exact: true }).click();
  await page.getByRole("button", { name: "Войти", exact: true }).first().click();
  await expect(page.getByRole("dialog", { name: "Войти", exact: true })).toBeVisible();
  await emitTauriEvent(page, "relay-oauth-status", { loginId: "oauth_synthetic", status: "callback_received" });
  const saving = page.getByRole("dialog", { name: "Сохраняем аккаунт…" });
  await expect(saving.getByRole("status")).toHaveText("Сохраняем аккаунт…");
  await page.setViewportSize({ width: 540, height: 400 });
  const bounds = await saving.boundingBox();
  expect(bounds).not.toBeNull();
  expect(bounds!.x).toBeGreaterThanOrEqual(0);
  expect(bounds!.x + bounds!.width).toBeLessThanOrEqual(540);
  expect(bounds!.y).toBeGreaterThanOrEqual(0);
  expect(bounds!.y + bounds!.height).toBeLessThanOrEqual(400);
  await expectSavingStatusCentered(saving);
  await page.screenshot({ path: testInfo.outputPath("oauth-saving-ru-dark.png") });

  await page.evaluate(() => {
    const internals = (window as unknown as { __TAURI_INTERNALS__: { invoke: (command: string, args?: unknown, options?: unknown) => Promise<unknown> } }).__TAURI_INTERNALS__;
    const invoke = internals.invoke.bind(internals);
    internals.invoke = async (command, args, options) => {
      if (command === "get_local_runtime_state") {
        await new Promise<void>((resolve) => window.addEventListener("relay-test-finish-oauth-refresh", () => resolve(), { once: true }));
      }
      return invoke(command, args, options);
    };
  });
  await page.evaluate(() => window.dispatchEvent(new Event("relay-test-finish-oauth")));
  await expect(page.getByRole("dialog")).toHaveCount(0);
  await expect(page.locator(".global-feedback.success")).toBeVisible();
  await page.evaluate(() => window.dispatchEvent(new Event("relay-test-finish-oauth-refresh")));
  await expect(page.locator(".account-card")).toBeVisible();
});

test("failed OAuth completion leaves a closable failure instead of an endless saving state", async ({ page }) => {
  await installTauriMock(page, { locale: "en", mode: "local", populated: false, oauthCompletionDeferred: true, oauthCompletionError: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await page.getByRole("button", { name: "Sign in", exact: true }).first().click();
  await expect(page.getByRole("dialog", { name: "Sign in" })).toBeVisible();
  await emitTauriEvent(page, "relay-oauth-status", { loginId: "oauth_synthetic", status: "callback_received" });
  await expect(page.getByRole("dialog", { name: "Saving account…" })).toBeVisible();
  await page.evaluate(() => window.dispatchEvent(new Event("relay-test-finish-oauth")));

  const failed = page.getByRole("dialog", { name: "Sign in" });
  await expect(failed.getByRole("alert")).toBeVisible();
  await expect(page.locator(".global-feedback.error")).toBeVisible();
  await expect(page.locator(".account-card")).toHaveCount(0);
  await page.keyboard.press("Escape");
  await expect(page.getByRole("dialog")).toHaveCount(0);
});

test("reauthentication updates the account without reopening pool setup", async ({ page }) => {
  await installTauriMock(page, { locale: "en", mode: "local", populated: true, accountAuthReason: "invalid_grant" });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  const account = page.locator(".account-card").filter({ hasText: "Personal Plus" });
  await account.getByRole("button", { name: "Sign-in required", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "Sign in" });
  await expect(dialog).toBeVisible();

  await emitTauriEvent(page, "relay-oauth-status", { loginId: "oauth_synthetic", status: "callback_received" });

  await expect(dialog).toHaveCount(0);
  await expect(page.getByRole("dialog", { name: "Account added" })).toHaveCount(0);
});

test("local proxy storage confirms assigned deletion and sends one atomic batch command", async ({ page }) => {
  await installTauriMock(page, { locale: "en", mode: "local", populated: true, accountCount: 3, proxyCount: 3 });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await page.getByRole("tab", { name: "Proxies" }).click();

  const summary = page.locator(".proxy-storage-counts");
  await expect(summary.locator("strong")).toHaveText(["3", "2", "1"]);
  await expect(page.locator(".proxy-storage-list")).not.toContainText("secret");
  await expect(page.locator(".proxy-storage-row").first().getByRole("img", { name: "Connection works" })).toBeVisible();
  await page.locator(".proxy-storage-row .proxy-diagnostic").first().hover();
  await expect(page.getByRole("tooltip")).toContainText("Europe/Amsterdam");

  await page.getByRole("button", { name: "Manage assigned accounts" }).first().click();
  let manager = page.getByRole("dialog", { name: "Proxy accounts" });
  await expect(manager.getByText("Business Workspace", { exact: true })).toBeVisible();
  await manager.getByText("Personal Plus", { exact: true }).click();
  await manager.getByRole("button", { name: "Save" }).click();
  const assignedAccounts = page.locator(".proxy-storage-bindings").first().locator("span").filter({ hasText: "Accounts: 2" });
  await expect(assignedAccounts).toHaveText("Accounts: 2");
  await assignedAccounts.hover();
  await expect(page.getByRole("tooltip")).toHaveText("Business Workspace, Personal Plus");
  await expect(assignedAccounts).not.toHaveAttribute("title");

  await page.getByRole("button", { name: "Import", exact: true }).click();
  const importDialog = page.getByRole("dialog", { name: "Import proxies" });
  await importDialog.getByLabel("Proxy list").fill("new-proxy.example.test:12000:user:secret\nsecond-proxy.example.test:12001:user:secret");
  await importDialog.getByRole("button", { name: "Import 2" }).click();
  await expect(importDialog.getByText("Added 2; skipped 0 duplicate(s).", { exact: true })).toBeVisible();
  await importDialog.getByRole("button", { name: "Done" }).click();
  await expect(summary.locator("strong")).toHaveText(["5", "4", "1"]);

  await page.getByLabel("Select all visible proxies").check();
  await page.locator(".proxy-selection-toolbar").getByRole("button", { name: "Delete", exact: true }).click();
  const confirmation = page.getByRole("dialog", { name: "Confirm action" });
  await expect(confirmation).toContainText("1 selected proxy endpoint(s) are used by 2 account(s). Delete all 5 selected proxies and return those accounts to their inherited route?");
  await confirmation.getByRole("button", { name: "Detach and delete" }).click();
  await expect(page.getByText("Proxy storage is empty", { exact: true })).toBeVisible();
  const calls = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: { input?: { accountIds?: string[]; proxyIds?: string[] } } }> }).__TAURI_TEST_INVOKES__);
  const deletions = calls.filter((call) => call.command === "delete_local_stored_proxies");
  expect(deletions).toHaveLength(1);
  expect(deletions[0].args.input?.proxyIds).toHaveLength(5);
  // The host detaches and deletes atomically, never via separate renderer writes.
  expect(calls.filter((call) => call.command === "set_local_stored_proxy_accounts" && call.args.input?.accountIds?.length === 0)).toHaveLength(0);
});

for (const staleReadOutcome of ["resolve", "reject"] as const) {
  test(`a proxy assignment survives an older pool read that will ${staleReadOutcome}`, async ({ page }) => {
    await installTauriMock(page, { locale: "en", mode: "local", populated: true, accountCount: 3, proxyCount: 3 });
    await page.goto("/");
    await page.getByRole("button", { name: "Connections", exact: true }).click();
    await page.getByRole("tab", { name: "Proxies" }).click();
    await expect(page.locator(".proxy-storage-counts strong").first()).toHaveText("3");

    await page.evaluate((outcome) => {
      const internals = (window as unknown as { __TAURI_INTERNALS__: { invoke: (command: string, args?: unknown, options?: unknown) => Promise<unknown> } }).__TAURI_INTERNALS__;
      const invoke = internals.invoke.bind(internals);
      let held = false;
      internals.invoke = async (command, args, options) => {
        if (command !== "get_local_proxy_pool" || held) return invoke(command, args, options);
        held = true;
        const snapshot = await invoke(command, args, options);
        document.body.dataset.proxyRead = "held";
        await new Promise<void>((resolve, reject) => {
          window.addEventListener("relay-test-release-proxy-read", () => {
            if (outcome === "reject") reject(new Error("synthetic stale pool read"));
            else resolve();
          }, { once: true });
        });
        return snapshot;
      };
    }, staleReadOutcome);
    await page.locator(".proxy-storage-toolbar").getByRole("button", { name: "Refresh", exact: true }).click();
    await page.waitForFunction(() => document.body.dataset.proxyRead === "held");

    await page.getByRole("button", { name: "Manage assigned accounts" }).first().click();
    const manager = page.getByRole("dialog", { name: "Proxy accounts" });
    await manager.getByText("Personal Plus", { exact: true }).click();
    await manager.getByRole("button", { name: "Save" }).click();
    const assignedAccounts = page.locator(".proxy-storage-bindings").first().getByText("Accounts: 2", { exact: true });
    await expect(assignedAccounts).toBeVisible();

    await page.evaluate(async () => {
      window.dispatchEvent(new Event("relay-test-release-proxy-read"));
      // Let the stale promise and its React state updates finish before checking.
      await new Promise<void>((resolve) => requestAnimationFrame(() => requestAnimationFrame(() => resolve())));
    });
    await expect(assignedAccounts).toBeVisible();
    await expect(page.locator(".proxy-storage-counts strong").first()).toHaveText("3");
  });
}

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

test("SIWC import labels its client and requires sign-in before pool admission", async ({ page }) => {
  await installTauriMock(page, { locale: "en", mode: "local", populated: true, accountOAuthClientKind: "siwc" });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await page.getByRole("button", { name: "Import", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "Import accounts" });
  await expect(dialog.getByRole("textbox", { name: "Account data or tokens", exact: true })).toBeVisible();
  await dialog.getByRole("button", { name: "Choose account files" }).click();
  await expect(dialog.getByRole("alert")).toContainText("Sign in again to authorize this host");
  await expect(dialog.getByText("ChatGPT (SIWC)", { exact: true })).toHaveCount(3);
  await expect(dialog.getByLabel("Add imported accounts to the pool", { exact: true })).toHaveCount(0);
  await dialog.getByRole("button", { name: "Import 2 account(s)" }).click();
  await expect(dialog).toBeHidden();
  const calls = await page.evaluate(() => (window as unknown as {
    __TAURI_TEST_INVOKES__: Array<{ command: string; args: { input?: { addToPool?: boolean } } }>;
  }).__TAURI_TEST_INVOKES__);
  expect(calls.findLast((call) => call.command === "confirm_local_account_import")?.args.input?.addToPool).toBe(false);
});

test("OAuth callback before start returns survives unrelated flow events", async ({ page }) => {
  await installTauriMock(page, { locale: "en", mode: "local", populated: false, oauthCallbackBeforeStartReturns: true });
  await page.goto("/");
  await page.evaluate(() => {
    const bridge = window as unknown as {
      __TAURI_INTERNALS__: { invoke: (command: string, args?: unknown, options?: unknown) => Promise<unknown> };
      __TAURI_TEST_EMIT__: (event: string, payload: unknown) => void;
    };
    const invoke = bridge.__TAURI_INTERNALS__.invoke.bind(bridge.__TAURI_INTERNALS__);
    bridge.__TAURI_INTERNALS__.invoke = async (command, args, options) => {
      const result = await invoke(command, args, options);
      if (command === "start_codex_oauth") {
        bridge.__TAURI_TEST_EMIT__("relay-oauth-status", { loginId: "oauth_unrelated", status: "expired" });
      }
      return result;
    };
  });
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await page.getByRole("button", { name: "Sign in", exact: true }).first().click();

  await expect(page.getByRole("dialog", { name: "Sign in" })).toHaveCount(0);
  await expect(page.locator(".global-feedback.success")).toHaveText("Account added.");
  const calls = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string }> }).__TAURI_TEST_INVOKES__);
  expect(calls.filter((call) => call.command === "complete_codex_oauth")).toHaveLength(1);
});

test("pasted Cockpit arrays reach the Rust batch preview unchanged", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await page.getByRole("button", { name: "Import", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "Import accounts" });
  await expect(dialog.locator(".import-start .form-note")).toHaveCount(0);
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
  { mode: "local", locale: "en", code: "models_forbidden", nav: "Connections", action: "Import", title: "Import accounts", input: "Account data or tokens", preview: "Preview import", confirm: "Import 2 account(s)", heading: "Some accounts were not imported", reason: "The provider denied access to the model list. Check this account's access and proxy region.", close: "Close" },
  { mode: "local", locale: "ru", code: "item_not_found", nav: "Подключения", action: "Импорт", title: "Импортировать учётные записи", input: "Данные аккаунтов или токены", preview: "Проверить импорт", confirm: "Импортировать: 2", heading: "Часть учётных записей не импортирована", reason: "Не пройдена финальная проверка аккаунта. Обновите его данные или прокси и повторите импорт.", close: "Закрыть" },
] as const) {
  test(`${scenario.mode} ${scenario.locale} ${scenario.code} import failures identify the safe account and explain the cause`, async ({ page }) => {
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

test("empty local Sources opens the compact source picker", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: false });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await expect(page.getByRole("tab", { name: "Sources", exact: true })).toBeVisible();
  await page.getByRole("tab", { name: "Sources", exact: true }).click();
  await expect(page.getByText("No API sources", { exact: true })).toBeVisible();
  await expect(page.getByRole("button", { name: "Add source", exact: true })).toHaveCount(1);
  await expect(page.getByText("Zenith API", { exact: true })).toHaveCount(0);

  await page.getByRole("button", { name: "Add source", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "Add source" });
  await expect(dialog.locator(".api-provider-title")).toHaveText(["OpenAI", "Anthropic", "Google Gemini", "DeepSeek", "Groq", "Mistral", "Moonshot / Kimi API", "Kimi Code", "MiniMax", "OpenRouter", "Zenith API", "Custom API"]);
  await expect(dialog.getByRole("radio", { name: "Custom API", exact: true })).toBeChecked();
  await expect(dialog.getByRole("radio")).toHaveCount(12);
  await expect(dialog.locator('[role="radio"][aria-checked="true"]')).toHaveCount(1);
  await expect(dialog.getByText("Recommended", { exact: true })).toHaveCount(0);
  await expect(dialog.getByRole("button", { name: "Get API key", exact: true })).toHaveCount(0);
  await expect(dialog.locator("footer").getByRole("button", { name: "Add source", exact: true })).toBeDisabled();

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
  const key = dialog.getByRole("textbox", { name: "Upstream API key", exact: true });
  await key.focus();
  expect(await key.evaluate((input) => {
    const field = input.closest<HTMLElement>(".secret-field")!;
    return { inputOutline: getComputedStyle(input).outlineStyle, fieldOutline: getComputedStyle(field).outlineStyle, fieldShadow: getComputedStyle(field).boxShadow };
  })).toEqual({ inputOutline: "none", fieldOutline: "none", fieldShadow: "none" });

  await dialog.getByRole("radio", { name: /Zenith API/ }).click();
  await dialog.getByRole("textbox", { name: "Upstream API key", exact: true }).fill("test-source-key");
  await dialog.getByRole("button", { name: "Add source", exact: true }).click();
  await expect(page.getByRole("dialog", { name: "Add source" })).toBeHidden();
  await expect(page.getByText("Zenith API", { exact: true })).toBeVisible();
  await expect(page.getByRole("button", { name: "Mode: Computer", exact: true })).toBeVisible();
  const createdSource = page.locator(".source-card").filter({ hasText: "Zenith API" });
  await expect(createdSource.getByRole("button", { name: "Launch", exact: true })).toBeEnabled();
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
  await installTauriMock(page, { mode: "local", locale: "en", populated: false });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await page.getByRole("tab", { name: "Sources", exact: true }).click();
  await page.getByRole("button", { name: "Add source", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "Add source" });

  await dialog.getByRole("radio", { name: /OpenAI/ }).click();
  await expect(dialog.locator(".source-route-simple-options")).toHaveCount(0);

  await dialog.getByRole("textbox", { name: "Upstream API key", exact: true }).fill("sk-synthetic-ready-key");
  await dialog.getByRole("button", { name: "Add source", exact: true }).click();
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
  await installTauriMock(page, { mode: "local", locale: "en", populated: false });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await page.getByRole("tab", { name: "Sources", exact: true }).click();
  await page.getByRole("button", { name: "Add source", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "Add source" });

  await dialog.getByRole("radio", { name: /Custom API/ }).click();
  await expect(dialog.locator(".source-model-mode")).toHaveCount(0);
  await expect(dialog.getByLabel("Model identifier")).toHaveCount(0);
  await expect(dialog.locator(".source-add-adapters")).toHaveCount(0);
});

test("source creation errors stay in one topmost dialog", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: false, sourceCreateError: "source_test_failed" });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await page.getByRole("tab", { name: "Sources", exact: true }).click();
  await page.getByRole("button", { name: "Add source", exact: true }).click();
  const sourceDialog = page.getByRole("dialog", { name: "Add source" });
  await sourceDialog.getByRole("radio", { name: /OpenAI/ }).click();
  await sourceDialog.getByRole("textbox", { name: "Upstream API key", exact: true }).fill("sk-synthetic-invalid");
  await sourceDialog.getByRole("button", { name: "Add source", exact: true }).click();

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
  await expect(sourceDialog.getByRole("textbox", { name: "Upstream API key", exact: true })).toHaveValue("sk-synthetic-invalid");
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

  const sourceRow = page.locator(".source-card").filter({ hasText: "Example compatible API" });
  const launch = sourceRow.getByRole("button", { name: "Launch", exact: true });
  await launch.click();
  const dialog = page.getByRole("dialog", { name: "Where do you want to launch this source?" });
  await expect(dialog.getByRole("button", { name: "ChatGPT", exact: true })).toBeDisabled();
  await expect(dialog.getByRole("button", { name: "OpenCode", exact: true })).toBeEnabled();
});

test("local Sources manages and launches saved sources without balance controls", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await page.getByRole("tab", { name: "Sources", exact: true }).click();
  await expect(page.getByText("Example compatible API", { exact: true })).toBeVisible();
  await expect(page.getByText("Balance", { exact: true })).toBeVisible();
  await expect(page.getByRole("button", { name: "Top up", exact: true })).toHaveCount(0);
  const source = page.locator(".source-card").filter({ hasText: "Example compatible API" });
  await source.getByRole("button", { name: "Launch", exact: true }).click();
  await page.getByRole("dialog", { name: "Where do you want to launch this source?" }).getByRole("button", { name: "ChatGPT", exact: true }).click();
  await expect.poll(() => page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string }> }).__TAURI_TEST_INVOKES__.some((call) => call.command === "launch_codex_source"))).toBe(true);
  const calls = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: Record<string, unknown> }> }).__TAURI_TEST_INVOKES__);
  expect(calls.find((call) => call.command === "launch_codex_source")?.args).toEqual({ sourceId: "source_synthetic" });
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
