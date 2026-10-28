import { expect, test } from "../../bun-playwright";
import { installTauriMock } from "../tauri-mock";
import { connectPoolToChatGPT } from "./helpers";

test("OAuth sign-in exposes SIWC window and proxy actions without extra descriptions", async ({ page }, testInfo) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await page.getByRole("button", { name: "Sign in", exact: true }).first().click();
  const dialog = page.getByRole("dialog", { name: "Sign in" });
  await expect(dialog.getByRole("radio", { name: "Codex", exact: true })).toBeChecked();
  await expect(dialog.getByRole("button", { name: "Copy sign-in link" })).toBeVisible();
  await dialog.getByText("ChatGPT (SIWC)", { exact: true }).click();
  await expect(dialog.getByRole("button", { name: "Copy sign-in link" })).toBeVisible();
  const open = dialog.getByRole("button", { name: "Open sign-in window", exact: true });
  await expect(dialog.getByRole("radio", { name: "ChatGPT (SIWC)", exact: true })).toBeChecked();
  await expect(dialog.getByRole("radio")).toHaveCount(3);
  await expect(dialog.getByRole("radio", { name: "Codex", exact: true })).toBeVisible();
  await expect(dialog.getByRole("button", { name: "Sign in through a proxy", exact: true })).toBeVisible();
  await expect(dialog.locator("p")).toHaveCount(0);
  await expect(open).toBeEnabled();
  let calls = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: Record<string, unknown> }> }).__TAURI_TEST_INVOKES__);
  expect(calls.findLast((call) => call.command === "start_codex_oauth")?.args).toEqual({ openBrowser: false, clientKind: "siwc" });
  expect(calls.some((call) => call.command === "resume_codex_oauth")).toBe(false);
  await open.click();
  await expect(dialog.getByRole("button", { name: /Open again in \d+ s|Reopen sign-in window/ })).toBeVisible();
  calls = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: Record<string, unknown> }> }).__TAURI_TEST_INVOKES__);
  expect(calls.some((call) => call.command === "resume_codex_oauth")).toBe(true);
  await expect(dialog.locator("input:not([type=radio]), textarea, details")).toHaveCount(0);
  await dialog.screenshot({ path: testInfo.outputPath("siwc-sign-in.png") });
  await dialog.getByRole("button", { name: "Close", exact: true }).click();
  await expect(dialog).toHaveCount(0);
  const canceled = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: Record<string, unknown> }> }).__TAURI_TEST_INVOKES__.findLast((call) => call.command === "cancel_codex_oauth"));
  expect(canceled?.args.loginId).toBe(calls.findLast((call) => call.command === "resume_codex_oauth")?.args.loginId);
});

for (const locale of ["en", "ru"] as const) {
  test(`SIWC proxy sign-in keeps its client and opens the native window (${locale})`, async ({ page }, testInfo) => {
    const labels = locale === "ru"
      ? { connections: "Подключения", signIn: "Войти", proxy: "Войти через прокси", open: "Открыть окно входа" }
      : { connections: "Connections", signIn: "Sign in", proxy: "Sign in through a proxy", open: "Open sign-in window" };
    if (locale === "ru") await page.setViewportSize({ width: 390, height: 760 });
    await installTauriMock(page, { mode: "local", locale, populated: true });
    await page.goto("/");
    await page.getByRole("button", { name: labels.connections, exact: true }).click();
    await page.getByRole("button", { name: labels.signIn, exact: true }).first().click();
    const dialog = page.getByRole("dialog", { name: labels.signIn, exact: true });
    await dialog.getByText("ChatGPT (SIWC)", { exact: true }).click();
    await dialog.getByRole("button", { name: labels.proxy, exact: true }).click();
    const proxies = page.getByRole("dialog", { name: labels.proxy, exact: true });
    await proxies.getByRole("radio", { name: /http:\/\/proxy-2\.example\.test:10001/ }).click();
    await proxies.getByRole("button", { name: labels.signIn, exact: true }).click();
    await expect(proxies).toHaveCount(0);
    await expect.poll(() => page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: Record<string, unknown> }> }).__TAURI_TEST_INVOKES__.findLast((call) => call.command === "start_codex_oauth")?.args))
      .toEqual({ openBrowser: true, proxyId: "proxy_synthetic_2", clientKind: "siwc" });
    await expect(dialog.getByRole("radio", { name: "ChatGPT (SIWC)", exact: true })).toBeChecked();
    await expect(dialog.locator(".account-connection-purpose")).toHaveCount(0);
    await expect(dialog.getByRole("button", { name: labels.open, exact: true })).toBeEnabled();
    const openBounds = await dialog.getByRole("button", { name: labels.open, exact: true }).boundingBox();
    const copyBounds = await dialog.locator(".oauth-copy-link").boundingBox();
    expect(openBounds).not.toBeNull();
    expect(copyBounds).not.toBeNull();
    expect(Math.abs(openBounds!.y - copyBounds!.y)).toBeLessThan(2);
    await dialog.screenshot({ path: testInfo.outputPath(`siwc-proxy-${locale}.png`) });
  });
}

test("OAuth countdown follows the active locale", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "ru", populated: true });
  await page.goto("/");

  await page.getByRole("button", { name: "Подключения", exact: true }).click();
  await page.getByRole("button", { name: "Войти", exact: true }).first().click();
  const dialog = page.getByRole("dialog", { name: "Войти" });
  await expect(dialog).toContainText(/Осталось времени\d+:\d{2}/);
  await expect(dialog).not.toContainText(/\b(?:AM|PM)\b/);
});

test("account identities are controlled only from the global action", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, distinctAccountIdentityHints: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();

  const identity = page.locator(".account-card").first().locator(".account-identity > strong");
  await expect(identity).toHaveText("p***@example.test");
  await expect(page.getByText("Personal Plus", { exact: true })).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Show full identity", exact: true })).toHaveCount(0);
  await page.getByRole("button", { name: "Show all account identities" }).click();
  await expect(identity).toHaveText("person@example.test");
  await expect(page.locator(".account-card").first().getByText("Personal Plus", { exact: true })).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Hide full identity", exact: true })).toHaveCount(0);
  await page.getByRole("button", { name: "Hide all account identities" }).click();
  await expect(identity).toHaveText("p***@example.test");

  const call = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: Record<string, unknown> }> }).__TAURI_TEST_INVOKES__.find((item) => item.command === "reveal_local_account_identity"));
  expect(call?.args).toEqual({ accountId: "account_synthetic" });
});

test("stale identity reveal cannot replace a newer request after switching through Hosted", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, distinctAccountIdentityHints: true });
  await page.goto("/");
  await page.evaluate(() => {
    type PendingReveal = { accountId: string; resolve: (value: unknown) => void };
    const pending: PendingReveal[] = [];
    const internals = (window as unknown as {
      __TAURI_INTERNALS__: {
        invoke: (command: string, args?: unknown, options?: unknown) => Promise<unknown>;
      };
    }).__TAURI_INTERNALS__;
    const originalInvoke = internals.invoke.bind(internals);
    internals.invoke = (command, args, options) => {
      if (command !== "reveal_local_account_identity") return originalInvoke(command, args, options);
      const accountId = String((args as { accountId?: unknown } | undefined)?.accountId ?? "");
      return new Promise((resolve) => pending.push({ accountId, resolve }));
    };
    Object.defineProperty(window, "__RESOLVE_IDENTITY_REVEAL__", {
      configurable: true,
      value: (identity: string) => {
        const request = pending.shift();
        if (!request) throw new Error("no pending identity reveal");
        request.resolve({ accountId: request.accountId, identity });
      },
    });
    Object.defineProperty(window, "__PENDING_IDENTITY_REVEALS__", {
      configurable: true,
      value: () => pending.length,
    });
  });

  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await page.getByRole("button", { name: "Show all account identities" }).click();
  await expect.poll(() => page.evaluate(() => (window as unknown as { __PENDING_IDENTITY_REVEALS__: () => number }).__PENDING_IDENTITY_REVEALS__())).toBe(1);

  await page.locator('.mode-picker > button[aria-haspopup="menu"]').click();
  await page.getByRole("menuitemradio", { name: "Relay Hosted", exact: true }).click();
  await expect(page.getByText("In development", { exact: true })).toBeVisible();
  await page.locator(".mode-picker-trigger").click();
  await page.getByRole("menuitemradio", { name: "Computer", exact: true }).click();
  await expect.poll(() => page.evaluate(() => localStorage.getItem("relay.mode"))).toBe("local");
  await expect.poll(() => page.evaluate(() => (window as unknown as { __PENDING_IDENTITY_REVEALS__: () => number }).__PENDING_IDENTITY_REVEALS__())).toBe(2);

  await page.evaluate(() => (window as unknown as { __RESOLVE_IDENTITY_REVEAL__: (identity: string) => void }).__RESOLVE_IDENTITY_REVEAL__("stale@example.test"));
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  const identityAction = page.getByRole("button", { name: "Hide all account identities", exact: true });
  await expect(identityAction).toBeVisible();
  await expect(identityAction).toBeDisabled();
  await expect(page.getByText("stale@example.test", { exact: true })).toHaveCount(0);

  await page.evaluate(() => (window as unknown as { __RESOLVE_IDENTITY_REVEAL__: (identity: string) => void }).__RESOLVE_IDENTITY_REVEAL__("fresh@example.test"));
  await expect(identityAction).toBeEnabled();
  await expect(page.getByText("stale@example.test", { exact: true })).toHaveCount(0);
  await expect(page.locator(".account-identity > strong").first()).toHaveText("fresh@example.test");
});

test("account identity visibility applies across the workspace and survives reloads", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, accountCount: 3, distinctAccountIdentityHints: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await page.getByRole("button", { name: "Show all account identities" }).click();
  await expect(page.locator(".account-identity > strong", { hasText: "person@example.test" })).toHaveCount(3);
  await expect.poll(() => page.evaluate(() => localStorage.getItem("relay.accountIdentitiesVisible"))).toBe("1");

  await page.getByRole("button", { name: "Pool", exact: true }).click();
  await expect(page.locator('.pool-member-card[data-member-kind="account"] .pool-member-name')).toHaveText(["person@example.test", "person@example.test", "person@example.test"]);

  await page.getByRole("button", { name: "Usage", exact: true }).click();
  await expect(page.locator('.usage-request-table tbody tr td[data-column="connection"]')).toHaveText("person@example.test");
  await page.getByRole("tab", { name: "Pool members", exact: true }).click();
  await expect(page.locator(".usage-aggregate-table tbody tr td").first()).toHaveText("person@example.test");

  await page.reload();
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await expect(page.getByRole("button", { name: "Hide all account identities" })).toBeVisible();
  await expect(page.locator(".account-identity > strong").first()).toHaveText("person@example.test");
  await page.getByRole("button", { name: "Hide all account identities" }).click();
  await expect(page.getByText("p***@example.test", { exact: true })).toBeVisible();
  await expect(page.getByText("b***@example.test", { exact: true })).toBeVisible();
  await expect(page.getByText("r***@example.test", { exact: true })).toBeVisible();
  await expect.poll(() => page.evaluate(() => localStorage.getItem("relay.accountIdentitiesVisible"))).toBe("0");

  await page.reload();
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await expect(page.getByRole("button", { name: "Show all account identities" })).toBeVisible();
  expect(await page.locator(".account-identity > strong").allTextContents()).toEqual(expect.arrayContaining(["p***@example.test", "b***@example.test", "r***@example.test"]));
  await expect(page.getByText("person@example.test", { exact: true })).toHaveCount(0);

  const calls = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string }> }).__TAURI_TEST_INVOKES__.filter((item) => item.command === "reveal_local_account_identity").length);
  expect(calls).toBe(0);
});



test("Russian usage labels format the saved request tier", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "ru", populated: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Использование", exact: true }).click();

  await expect(page.locator('.usage-request-table thead th[data-column="equivalent"]')).toHaveText("API-экв.");
  await page.getByRole("button", { name: "Сведения о запросе: req_synthetic_local" }).click();
  await expect(page.locator(".request-details-parameters > div").filter({ hasText: "Режим обслуживания" })).toHaveText("Режим обслуживанияОбычная");
});

test("stale local usage and proxy references never expose internal account IDs", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, proxyCount: 1, staleAccountReferences: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Usage", exact: true }).click();
  await expect(page.locator('.usage-request-table tbody tr td[data-column="connection"]')).toHaveText("Removed account");

  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await page.getByRole("tab", { name: "Proxies" }).click();
  await expect(page.locator(".proxy-card-accounts").first()).toHaveText("1");
  await page.locator(".proxy-card-accounts").first().hover();
  await expect(page.getByRole("tooltip")).toHaveText("Accounts");
  await expect(page.getByText("account_deleted_internal", { exact: true })).toHaveCount(0);
});

test("an exhausted weekly quota makes the account effectively unavailable in connections and pool", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, exhaustedQuotaWindow: "secondary" });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  const connection = page.locator(".account-card").first();
  await expect(connection.locator(".quota-meter strong")).toHaveText(["72%", "0%"]);
  await expect(connection.locator('.relay-status-icon[aria-label="Waiting for quota"]')).toBeVisible();

  await page.getByRole("button", { name: "Pool", exact: true }).click();
  const accountCard = page.locator('.pool-member-card[data-member-label="Personal Plus"]');
  await expect(accountCard.locator(".quota-meter strong")).toHaveText(["72%", "0%"]);
  await expect(accountCard.locator('.relay-status-icon[aria-label="Waiting for quota"]')).toBeVisible();
});

test("weekly calibrated account value remains controlled by the dollar toggle", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  const connectionValue = page.locator(".account-card .account-value-strip");

  await expect(connectionValue.first()).toBeVisible();
  await expect(connectionValue.first()).toHaveAttribute("data-columns", "3");
  await expect(connectionValue.first().locator("dt")).toHaveText(["API equiv. used", "Weekly remaining", "Payback"]);
  await expect(connectionValue.first().locator("dd").nth(1)).toHaveText("≈$6.4");
  await expect(connectionValue.first().locator("dd small")).toHaveCount(0);
  await expect(page.locator(".account-provider-quota-strip")).toHaveCount(0);
  await expect(page.locator(".account-card .quota-meter").first()).toBeVisible();
  await expect(page.getByRole("button", { name: "Hide account calculation" })).toHaveAttribute("aria-pressed", "true");
  await expect(page.getByRole("button", { name: "Hide account calculation" }).locator("svg.lucide-dollar-sign")).toBeVisible();
  await page.getByRole("button", { name: "Hide account calculation" }).click();
  await expect(connectionValue).toHaveCount(0);
  await expect.poll(() => page.evaluate(() => localStorage.getItem("relay.accountValueVisible"))).toBe("false");

  await page.getByRole("button", { name: "Pool", exact: true }).click();
  const poolValue = page.locator('.pool-member-card[data-member-kind="account"] .account-value-strip');
  await expect(page.getByRole("button", { name: "Show account calculation" })).toHaveAttribute("aria-pressed", "false");
  await expect(poolValue).toHaveCount(0);
  await expect(page.locator(".pool-economics")).toHaveCount(0);
  await page.getByRole("button", { name: "Show account calculation" }).click();
  await expect(poolValue.first()).toBeVisible();
  await expect(page.locator(".pool-economics")).toBeVisible();
  await expect(page.locator(".account-provider-quota-strip")).toHaveCount(0);

  await page.getByRole("button", { name: "Settings", exact: true }).click();
  await expect(page.getByRole("button", { name: "Standard calculation", exact: true })).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Relay estimate (experimental)", exact: true })).toHaveCount(0);

  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await expect(connectionValue.first()).toBeVisible();
  await expect(page.getByRole("button", { name: "Hide account calculation" })).toHaveAttribute("aria-pressed", "true");
  await page.getByRole("button", { name: "Hide account calculation" }).click();
  await expect(connectionValue).toHaveCount(0);
  await expect.poll(() => page.evaluate(() => localStorage.getItem("relay.accountValueVisible"))).toBe("false");

  await page.getByRole("button", { name: "Pool", exact: true }).click();
  await expect(page.getByRole("button", { name: "Show account calculation" })).toHaveAttribute("aria-pressed", "false");
  await expect(poolValue).toHaveCount(0);
  await page.getByRole("button", { name: "Show account calculation" }).click();
  await expect(poolValue.first()).toBeVisible();
});

test("provider-reported credits keep an exhausted account in the pool", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, providerCredits: 222.75 });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();

  const summary = page.locator(".connections-account-summary");
  const totalCredits = summary.locator('[data-summary="provider-credits"]');
  await expect(summary).toHaveAttribute("data-has-provider-credits", "true");
  await expect(totalCredits).toContainText("Total credits");
  await expect(totalCredits.locator("strong")).toHaveText("222.8");

  const connectionCredits = page.locator(".account-card").first().locator(".account-provider-quota-strip");
  await expect(connectionCredits.locator("dt")).toHaveText("Credits");
  await expect(connectionCredits.locator("dd")).toHaveText("222.8");
  await page.getByRole("button", { name: "Hide account calculation" }).click();
  await expect(connectionCredits).toBeVisible();

  await page.getByRole("button", { name: "Pool", exact: true }).click();
  const poolCredits = page.locator('.pool-member-card[data-member-kind="account"]').first().locator(".account-provider-quota-strip");
  await expect(poolCredits.locator("dt")).toHaveText("Credits");
  await expect(poolCredits.locator("dd")).toHaveText("222.8");
});

test("pool retains monetary controls after its last account is removed", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, accountCount: 1, sourceCount: 1 });
  await page.goto("/");
  await page.getByRole("button", { name: "Pool", exact: true }).click();

  await page.getByRole("button", { name: "Remove from pool: Personal Plus" }).click({ button: "right" });
  await expect(page.locator('.pool-member-card[data-member-kind="account"]')).toHaveCount(0);

  await expect(page.locator('.pool-member-card[data-member-kind="source"]')).toHaveCount(1);
  await page.getByRole("button", { name: "Hide account calculation", exact: true }).click();
  await expect(page.locator(".pool-economics")).toHaveCount(0);
  await page.getByRole("button", { name: "Show account calculation", exact: true }).click();
  await expect(page.locator(".pool-economics")).toBeVisible();
  await expect(page.locator('.pool-member-card[data-member-kind="account"]')).toHaveCount(0);
});

test("quota cards name provider windows and make remaining percentages explicit", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, accountCount: 3 });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();

  const free = page.locator(".account-card").filter({ hasText: "Backup account" });
  await expect(free.locator(".quota-meter-heading > span")).toHaveText("30 days");
  await expect(free.locator(".quota-meter-heading > strong")).toHaveText("95%");
  await expect(free.locator(".quota-track")).toHaveAttribute("aria-label", "30 days: 95%");
});

test("pool toggle changes state without switching ChatGPT", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, gatewayRunning: false });
  await page.goto("/");
  // The endpoint power control lives in Overview; Pool only routes members.
  await page.getByRole("button", { name: "Start API", exact: true }).click();
  await expect(page.getByText("Endpoint started.")).toBeVisible();
  await page.getByRole("button", { name: "Pool", exact: true }).click();
  const header = page.locator(".relay-page-header");
  await expect(header.getByRole("button", { name: "Connect", exact: true })).toBeVisible();
  await expect(header.locator(".pool-header-actions > *")).toHaveCount(3);
  await header.locator(".pool-preset-menu summary").click();
  await expect(header.getByRole("menuitem", { name: "Save preset", exact: true })).toBeVisible();
  await page.keyboard.press("Escape");
  await page.getByRole("button", { name: "Overview", exact: true }).click();
  await page.getByRole("button", { name: "Stop API", exact: true }).click();
  await expect(page.getByText("Endpoint stopped.")).toBeVisible();

  const calls = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: Record<string, unknown> }> }).__TAURI_TEST_INVOKES__);
  const workflow = calls.filter((call) => ["start_local_gateway", "stop_local_gateway", "attach_codex_to_local_gateway", "launch_managed_codex_profile"].includes(call.command));
  expect(workflow.map((call) => call.command)).toEqual(["start_local_gateway", "stop_local_gateway"]);
});

test("pool controls delegate an exhausted OAuth account to the backend", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, accountOAuthClientKind: "excel_bps", accountCount: 1, poolMembers: false, gatewayRunning: false });
  await page.goto("/");
  await page.getByRole("button", { name: "Pool", exact: true }).click();

  await page.getByRole("button", { name: "Add member", exact: true }).first().click();
  const dialog = page.getByRole("dialog", { name: "Add connections to pool" });
  await dialog.getByText("Personal Plus", { exact: true }).click();
  await dialog.getByRole("button", { name: "Add selected (1)" }).click();
  await expect(page.getByRole("dialog", { name: "Add a regular account to the pool?", exact: true })).toHaveCount(0);

  await page.getByRole("button", { name: "Overview", exact: true }).click();
  const start = page.getByRole("button", { name: "Start API", exact: true });
  await expect(start).toBeEnabled();
  await start.click();
  await page.getByRole("button", { name: "Pool", exact: true }).click();
  const switchToPool = page.getByRole("button", { name: "Connect", exact: true });
  await expect(switchToPool).toBeEnabled();
  await connectPoolToChatGPT(page);

  await expect.poll(() => page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string }> }).__TAURI_TEST_INVOKES__.map((call) => call.command))).toEqual(expect.arrayContaining([
    "start_local_gateway",
    "attach_codex_to_local_gateway",
  ]));
});

test("switch ChatGPT uses the backend system key and relaunches ChatGPT without starting the gateway", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, gatewayRunning: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Pool", exact: true }).click();
  await connectPoolToChatGPT(page);
  const feedback = page.locator(".global-feedback.success");
  await expect(feedback).toContainText("Client launched.");
  const [feedbackBox, shellBox, helpBox] = await Promise.all([
    feedback.boundingBox(),
    page.locator(".relay-shell").boundingBox(),
    page.getByRole("button", { name: "Help" }).boundingBox(),
  ]);
  expect(feedbackBox).not.toBeNull();
  expect(shellBox).not.toBeNull();
  expect(helpBox).not.toBeNull();
  expect(feedbackBox!.x).toBeGreaterThanOrEqual(shellBox!.x);
  expect(feedbackBox!.x).toBeLessThanOrEqual(helpBox!.x + 2);
  expect(feedbackBox!.y + feedbackBox!.height).toBeLessThanOrEqual(helpBox!.y + 1);
  expect(feedbackBox!.x + feedbackBox!.width).toBeLessThanOrEqual(shellBox!.x + shellBox!.width + 1);

  const calls = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: Record<string, unknown> }> }).__TAURI_TEST_INVOKES__);
  const workflow = calls.filter((call) => ["start_local_gateway", "attach_codex_to_local_gateway", "launch_managed_codex_profile"].includes(call.command));
  expect(workflow.map((call) => call.command)).toEqual(["attach_codex_to_local_gateway", "launch_managed_codex_profile"]);
  expect(workflow[0].args).toEqual({ boundOauthAccountId: null, disableOauthBinding: true });
  await expect(page.getByRole("dialog", { name: "Confirm action" })).toHaveCount(0);
  await expect(feedback).toBeHidden({ timeout: 5_000 });
});

test("pool connection launches Codex before waiting for a renderer snapshot", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, gatewayRunning: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Pool", exact: true }).click();
  await page.evaluate(() => {
    const internals = (window as unknown as { __TAURI_INTERNALS__: { invoke: (command: string, args?: unknown, options?: unknown) => Promise<unknown> } }).__TAURI_INTERNALS__;
    const invoke = internals.invoke.bind(internals);
    let attached = false;
    let launched = false;
    internals.invoke = async (command, args, options) => {
      // A slow state refresh must not leave the successfully attached client
      // closed. Only the launch command can release this synthetic snapshot.
      if (command === "get_local_runtime_state" && attached && !launched) {
        await new Promise<void>((resolve) => {
          const check = window.setInterval(() => {
            if (launched) { window.clearInterval(check); resolve(); }
          }, 10);
        });
      }
      const result = await invoke(command, args, options);
      if (command === "attach_codex_to_local_gateway") attached = true;
      if (command === "launch_managed_codex_profile") launched = true;
      return result;
    };
  });
  await connectPoolToChatGPT(page);
  await expect(page.locator(".global-feedback.success")).toContainText("Client launched.");
  const calls = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string }> }).__TAURI_TEST_INVOKES__.map((call) => call.command));
  const attachIndex = calls.lastIndexOf("attach_codex_to_local_gateway");
  const launchIndex = calls.lastIndexOf("launch_managed_codex_profile");
  expect(launchIndex).toBeGreaterThan(attachIndex);
  expect(calls.slice(attachIndex + 1, launchIndex)).not.toContain("get_local_runtime_state");
});

test("profile switch errors stay visible until the one-minute timeout", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, gatewayRunning: true, profileSwitchError: true });
  await page.clock.install({ time: 0 });
  await page.goto("/");
  await page.getByRole("button", { name: "Pool", exact: true }).click();
  await connectPoolToChatGPT(page);

  const feedback = page.locator(".global-feedback.error");
  await expect(feedback).toContainText("Something went wrong. Click to view details.");
  await expect(feedback).not.toContainText("The profile changed during the operation.");
  await expect(feedback).not.toContainText("profile_restore_blocked");
  await page.clock.runFor(59_000);
  await expect(feedback).toBeVisible();
  await page.clock.runFor(2_000);
  await expect(feedback).toHaveCount(0);
});
