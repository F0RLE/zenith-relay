import { expect, test } from "../../bun-playwright";
import { emitTauriEvent, installTauriMock } from "../tauri-mock";
import { chooseOption, settleConfirmation, openGatewayApi, openGatewayApplication, connectPoolToChatGPT } from "./helpers";

test("global errors expose sanitized details and a copy confirmation", async ({ page }) => {
  await page.context().grantPermissions(["clipboard-read", "clipboard-write"], { origin: "http://127.0.0.1:1420" });
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, gatewayRunning: true, profileSwitchError: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Pool", exact: true }).click();
  await connectPoolToChatGPT(page);

  const feedback = page.locator(".global-feedback.error");
  await expect(feedback.locator(".global-feedback-actions .relay-icon-button")).toHaveCount(0);
  await feedback.locator(".global-feedback-error-trigger").click();
  const details = page.getByRole("dialog", { name: "Error details" });
  await expect(details).toContainText('"code": "profile_restore_blocked"');
  await expect(details).toContainText('"message": "Synthetic profile conflict"');
  await expect(page.locator(".global-feedback")).toHaveCount(0);
  await expect(page.locator(".global-feedback-error-trigger")).toHaveCount(0);

  await details.getByRole("button", { name: "Copy error JSON" }).click();
  await expect(details.locator(".global-feedback-dialog-copy-state")).toHaveText("Copied");
  await expect.poll(() => page.evaluate(() => navigator.clipboard.readText())).toContain('"profile_restore_blocked"');
  await details.locator("header .relay-icon-button").click();
  await expect(feedback).toHaveCount(0);
});

test("focus refreshes runtime only after a state revision changes", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true });
  await page.goto("/");
  const stateReads = () => page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string }> }).__TAURI_TEST_INVOKES__.filter((call) => call.command === "get_local_runtime_state").length);
  await expect.poll(stateReads).toBeGreaterThan(0);
  const before = await stateReads();
  await page.evaluate(() => window.dispatchEvent(new Event("focus")));
  await page.waitForTimeout(300);
  expect(await stateReads()).toBe(before);

  await emitTauriEvent(page, "zenith-state-changed", null);
  await expect.poll(stateReads).toBeGreaterThan(before);
  const refreshed = await stateReads();
  await page.evaluate(() => window.dispatchEvent(new Event("focus")));
  await page.waitForTimeout(300);
  expect(await stateReads()).toBe(refreshed);
});

test("background refresh catches a revision emitted during an in-flight snapshot", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true });
  await page.goto("/");
  await page.evaluate(() => {
    const internals = (window as unknown as { __TAURI_INTERNALS__: { invoke: (command: string, args?: unknown, options?: unknown) => Promise<unknown> } }).__TAURI_INTERNALS__;
    const invoke = internals.invoke.bind(internals);
    let delayNextStateRead = true;
    internals.invoke = (command, args, options) => {
      if (command !== "get_local_runtime_state" || !delayNextStateRead) return invoke(command, args, options);
      delayNextStateRead = false;
      (window as unknown as { __DELAYED_STATE_READ_STARTED__: boolean }).__DELAYED_STATE_READ_STARTED__ = true;
      return new Promise((resolve, reject) => window.setTimeout(() => invoke(command, args, options).then(resolve, reject), 300));
    };
  });
  const stateReads = () => page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string }> }).__TAURI_TEST_INVOKES__.filter((call) => call.command === "get_local_runtime_state").length);
  const before = await stateReads();
  await emitTauriEvent(page, "zenith-state-changed", null);
  await expect.poll(() => page.evaluate(() => Boolean((window as unknown as { __DELAYED_STATE_READ_STARTED__?: boolean }).__DELAYED_STATE_READ_STARTED__))).toBe(true);
  await emitTauriEvent(page, "zenith-state-changed", null);
  await expect.poll(stateReads).toBeGreaterThanOrEqual(before + 2);
});

test("background snapshots and analytics stay dormant on inactive pages", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Settings", exact: true }).click();
  const stateReads = () => page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string }> }).__TAURI_TEST_INVOKES__.filter((call) => call.command === "get_local_runtime_state").length);
  const usageReads = () => page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string }> }).__TAURI_TEST_INVOKES__.filter((call) => call.command === "get_local_usage_page").length);
  const stateReadsBefore = await stateReads();
  const before = await usageReads();
  await emitTauriEvent(page, "zenith-state-changed", null);
  await page.waitForTimeout(800);
  expect(await stateReads()).toBe(stateReadsBefore);
  expect(await usageReads()).toBe(before);

  await page.getByRole("button", { name: "Overview", exact: true }).click();
  await expect.poll(stateReads).toBeGreaterThan(stateReadsBefore);
});

test("runtime snapshots stay off Usage while active Usage reloads its own data", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true });
  await page.goto("/");
  const stateReads = () => page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string }> }).__TAURI_TEST_INVOKES__.filter((call) => call.command === "get_local_runtime_state").length);
  const usageReads = () => page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string }> }).__TAURI_TEST_INVOKES__.filter((call) => call.command === "get_local_usage_page").length);
  await expect.poll(usageReads).toBeGreaterThan(0);
  const overviewReads = await usageReads();
  await emitTauriEvent(page, "zenith-state-changed", null);
  await expect.poll(usageReads).toBeGreaterThan(overviewReads);

  await page.getByRole("button", { name: "Usage", exact: true }).click();
  await expect.poll(usageReads).toBeGreaterThan(overviewReads + 1);
  const usagePageReads = await usageReads();
  const usagePageStateReads = await stateReads();
  await emitTauriEvent(page, "zenith-state-changed", null);
  await expect.poll(usageReads).toBeGreaterThan(usagePageReads);
  expect(await stateReads()).toBe(usagePageStateReads);
});

test("usage records refresh only the visible Usage page", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true });
  await page.goto("/");
  const stateReads = () => page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string }> }).__TAURI_TEST_INVOKES__.filter((call) => call.command === "get_local_runtime_state").length);
  const usageReads = () => page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string }> }).__TAURI_TEST_INVOKES__.filter((call) => call.command === "get_local_usage_page").length);
  await expect.poll(stateReads).toBeGreaterThan(0);
  // The overview snapshot is asynchronous. Let it settle before attributing
  // subsequent reads to the usage-recorded event under test.
  await page.waitForTimeout(300);
  const overviewStateReads = await stateReads();
  const overviewUsageReads = await usageReads();

  await emitTauriEvent(page, "zenith-usage-recorded", null);
  await page.waitForTimeout(700);
  expect(await stateReads()).toBe(overviewStateReads);
  expect(await usageReads()).toBe(overviewUsageReads);

  await page.getByRole("button", { name: "Usage", exact: true }).click();
  await expect.poll(usageReads).toBeGreaterThan(overviewUsageReads);
  const activeUsageReads = await usageReads();
  const activeStateReads = await stateReads();
  await emitTauriEvent(page, "zenith-usage-recorded", null);
  await expect.poll(usageReads).toBeGreaterThan(activeUsageReads);
  expect(await stateReads()).toBe(activeStateReads);
});

test("Usage renders the cached report while a return navigation refresh is pending", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true });
  await page.goto("/");
  const usageRow = page.getByRole("row").filter({ hasText: "req_synthetic_local" });
  await page.getByRole("button", { name: "Usage", exact: true }).click();
  await expect(usageRow).toBeVisible();
  await page.getByRole("button", { name: "Settings", exact: true }).click();

  await page.evaluate(() => {
    const testWindow = window as unknown as {
      __USAGE_RETURN_PENDING__?: boolean;
      __RELEASE_USAGE_RETURN__?: () => void;
      __TAURI_INTERNALS__: { invoke: (command: string, args?: unknown, options?: unknown) => Promise<unknown> };
    };
    const invoke = testWindow.__TAURI_INTERNALS__.invoke.bind(testWindow.__TAURI_INTERNALS__);
    testWindow.__TAURI_INTERNALS__.invoke = (command, args, options) => {
      if (command !== "get_local_usage_page") return invoke(command, args, options);
      testWindow.__USAGE_RETURN_PENDING__ = true;
      return new Promise((resolve, reject) => {
        testWindow.__RELEASE_USAGE_RETURN__ = () => void invoke(command, args, options).then(resolve, reject);
      });
    };
  });
  await page.waitForTimeout(1_100);
  await page.getByRole("button", { name: "Usage", exact: true }).click();
  await expect.poll(() => page.evaluate(() => Boolean((window as unknown as { __USAGE_RETURN_PENDING__?: boolean }).__USAGE_RETURN_PENDING__))).toBe(true);
  await expect(usageRow).toBeVisible();
  await page.evaluate(() => {
    const release = (window as unknown as { __RELEASE_USAGE_RETURN__?: () => void }).__RELEASE_USAGE_RETURN__;
    if (!release) throw new Error("usage return was not pending");
    release();
  });
  await expect(usageRow).toBeVisible();
});

test("Overview keeps rendered analytics while a background refresh is pending or fails", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true });
  await page.goto("/");
  const analytics = page.locator(".overview-analytics");
  const tokenSummary = page.locator(".overview-chart.tokens .overview-chart-summary");
  await expect(tokenSummary).toHaveText("28");

  await page.evaluate(() => {
    const testWindow = window as unknown as {
      __OVERVIEW_USAGE_REFRESH_PENDING__?: boolean;
      __RESOLVE_OVERVIEW_USAGE_REFRESH__?: () => void;
      __TAURI_INTERNALS__: { invoke: (command: string, args?: unknown, options?: unknown) => Promise<unknown> };
    };
    const internals = testWindow.__TAURI_INTERNALS__;
    const invoke = internals.invoke.bind(internals);
    let delayNextUsageRead = true;
    internals.invoke = (command, args, options) => {
      if (command !== "get_local_usage_page" || !delayNextUsageRead) return invoke(command, args, options);
      delayNextUsageRead = false;
      testWindow.__OVERVIEW_USAGE_REFRESH_PENDING__ = true;
      return new Promise((resolve, reject) => {
        testWindow.__RESOLVE_OVERVIEW_USAGE_REFRESH__ = () => void invoke(command, args, options).then(resolve, reject);
      });
    };
  });
  await emitTauriEvent(page, "zenith-state-changed", null);
  await expect.poll(() => page.evaluate(() => Boolean((window as unknown as { __OVERVIEW_USAGE_REFRESH_PENDING__?: boolean }).__OVERVIEW_USAGE_REFRESH_PENDING__))).toBe(true);
  await expect(analytics).toHaveAttribute("aria-busy", "true");
  await expect(tokenSummary).toHaveText("28");
  await page.evaluate(() => {
    const release = (window as unknown as { __RESOLVE_OVERVIEW_USAGE_REFRESH__?: () => void }).__RESOLVE_OVERVIEW_USAGE_REFRESH__;
    if (!release) throw new Error("overview usage refresh was not pending");
    release();
  });
  await expect(analytics).toHaveAttribute("aria-busy", "false");

  await page.evaluate(() => {
    const internals = (window as unknown as { __TAURI_INTERNALS__: { invoke: (command: string, args?: unknown, options?: unknown) => Promise<unknown> } }).__TAURI_INTERNALS__;
    const invoke = internals.invoke.bind(internals);
    let failNextUsageRead = true;
    internals.invoke = (command, args, options) => {
      if (command !== "get_local_usage_page" || !failNextUsageRead) return invoke(command, args, options);
      failNextUsageRead = false;
      return Promise.reject(new Error("Synthetic overview usage error"));
    };
  });
  await emitTauriEvent(page, "zenith-state-changed", null);
  await expect(analytics.getByRole("alert")).toBeVisible();
  await expect(tokenSummary).toHaveText("28");
});

test("Overview restores the last analytics snapshot when re-entered", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true });
  await page.goto("/");
  const tokenSummary = page.locator(".overview-chart.tokens .overview-chart-summary");
  await expect(tokenSummary).toHaveText("28");

  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await expect(page.getByRole("heading", { name: "Connections" })).toBeVisible();
  await page.evaluate(() => {
    const testWindow = window as unknown as {
      __OVERVIEW_REENTRY_USAGE_PENDING__?: boolean;
      __RELEASE_OVERVIEW_REENTRY_USAGE__?: () => void;
      __TAURI_INTERNALS__: { invoke: (command: string, args?: unknown, options?: unknown) => Promise<unknown> };
    };
    const internals = testWindow.__TAURI_INTERNALS__;
    const invoke = internals.invoke.bind(internals);
    let delayNextUsageRead = true;
    internals.invoke = (command, args, options) => {
      if (command !== "get_local_usage_page" || !delayNextUsageRead) return invoke(command, args, options);
      delayNextUsageRead = false;
      testWindow.__OVERVIEW_REENTRY_USAGE_PENDING__ = true;
      return new Promise((resolve, reject) => {
        testWindow.__RELEASE_OVERVIEW_REENTRY_USAGE__ = () => void invoke(command, args, options).then(resolve, reject);
      });
    };
  });
  await page.waitForTimeout(1_100);
  await page.getByRole("button", { name: "Overview", exact: true }).click();
  await expect.poll(() => page.evaluate(() => Boolean((window as unknown as { __OVERVIEW_REENTRY_USAGE_PENDING__?: boolean }).__OVERVIEW_REENTRY_USAGE_PENDING__))).toBe(true);
  await expect(tokenSummary).toHaveText("28");
  await page.evaluate(() => {
    const release = (window as unknown as { __RELEASE_OVERVIEW_REENTRY_USAGE__?: () => void }).__RELEASE_OVERVIEW_REENTRY_USAGE__;
    if (!release) throw new Error("overview re-entry usage request was not pending");
    release();
  });
  await expect(page.locator(".overview-analytics")).toHaveAttribute("aria-busy", "false");
});

test("open request details follow the terminal fallback result", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, usageFailure: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Usage", exact: true }).click();
  await page.getByRole("button", { name: "Request details: req_synthetic_local" }).click();
  const dialog = page.getByRole("dialog", { name: "Request details" });
  const error = dialog.locator(".request-details-error");
  await expect(dialog.getByText("Failed", { exact: true })).toBeVisible();
  await expect(error.locator(".request-details-list > div").filter({ hasText: "HTTP status" }).locator("dd")).toHaveText("502");

  await page.evaluate(() => {
    const internals = (window as unknown as { __TAURI_INTERNALS__: { invoke: (command: string, args?: unknown, options?: unknown) => Promise<unknown> } }).__TAURI_INTERNALS__;
    const invoke = internals.invoke.bind(internals);
    internals.invoke = async (command, args, options) => {
      const result = await invoke(command, args, options);
      if (command !== "get_local_usage_page") return result;
      const usage = structuredClone(result) as { events: Array<{ attempt: number; success: boolean; httpStatus: number; errorCategory: string | null; latencyMs: number }>; totals: { requests: number; successfulRequests: number } };
      if (usage.events[0]) Object.assign(usage.events[0], { attempt: 2, success: true, httpStatus: 200, errorCategory: null, latencyMs: 16_157 });
      usage.totals.successfulRequests = usage.totals.requests;
      return usage;
    };
  });
  await emitTauriEvent(page, "zenith-state-changed", null);

  await expect(dialog.getByText("Success", { exact: true })).toBeVisible();
  await expect(dialog.locator(".request-details-error")).toHaveCount(0);
  await expect(dialog.getByText("16.2 s", { exact: true })).toBeVisible();
  await expect(page.getByRole("row").filter({ hasText: "req_synthetic_local" }).locator('td[data-column="timing"]')).toHaveText("128 ms / 16.2 s");
});

test("switching modes ignores a late failure from the previous mode", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, gatewayRunning: true });
  await page.goto("/");
  await page.evaluate(() => {
    const internals = (window as unknown as { __TAURI_INTERNALS__: { invoke: (command: string, args?: unknown, options?: unknown) => Promise<unknown> } }).__TAURI_INTERNALS__;
    const invoke = internals.invoke.bind(internals);
    internals.invoke = (command, args, options) => command === "stop_local_gateway"
      ? new Promise((_, reject) => window.setTimeout(() => reject({ code: "not_found", message: "Synthetic stale record" }), 150))
      : invoke(command, args, options);
  });

  await page.getByRole("button", { name: "Stop API", exact: true }).click();
  await page.locator('.mode-picker > button[aria-haspopup="menu"]').click();
  await page.getByRole("menuitemradio", { name: "Choose API", exact: true }).click();

  await expect.poll(() => page.evaluate(() => localStorage.getItem("relay.mode"))).toBe("zenith");
  await page.waitForTimeout(250);
  await expect(page.getByText("The requested record was not found.", { exact: true })).toHaveCount(0);
});

test("startup records theme, i18n, first-frame, and interactive timings", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", theme: "dark", populated: true });
  await page.goto("/");
  await expect.poll(() => page.evaluate(() => performance.getEntriesByType("measure").filter((entry) => entry.name.startsWith("zenith:")).map((entry) => entry.name))).toEqual(expect.arrayContaining(["zenith:i18n", "zenith:first-frame", "zenith:interactive"]));
  await expect(page.locator("#splash-screen")).toHaveCount(0);
  await expect.poll(() => page.evaluate(() => document.documentElement.dataset.startupReady)).toBe("true");
  const timings = await page.evaluate(() => Object.fromEntries(performance.getEntriesByType("measure").filter((entry) => entry.name.startsWith("zenith:")).map((entry) => [entry.name, entry.duration])));
  expect(timings["zenith:i18n"]).toBeGreaterThanOrEqual(0);
  expect(timings["zenith:first-frame"]).toBeGreaterThanOrEqual(timings["zenith:i18n"]);
  expect(timings["zenith:interactive"]).toBeGreaterThanOrEqual(timings["zenith:i18n"]);
});

test("navigation records Pool, Connections, and mode switch timings", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true });
  await page.goto("/");
  const samples = () => page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: { name?: string; durationMs?: number; context?: string } }> }).__TAURI_TEST_INVOKES__.filter((call) => call.command === "record_local_performance_sample"));

  await page.getByRole("button", { name: "Pool", exact: true }).click();
  await expect.poll(async () => (await samples()).some((call) => call.args.name === "page_open" && call.args.context === "pool")).toBe(true);
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await expect.poll(async () => (await samples()).some((call) => call.args.name === "page_open" && call.args.context === "connections")).toBe(true);
  await page.locator('.mode-picker > button[aria-haspopup="menu"]').click();
  await page.getByRole("menuitemradio", { name: "Choose API", exact: true }).click();
  await expect.poll(async () => (await samples()).some((call) => call.args.name === "mode_switch" && call.args.context === "zenith")).toBe(true);

  const measured = (await samples()).filter((call) =>
    (call.args.name === "page_open" && ["pool", "connections"].includes(call.args.context ?? ""))
    || (call.args.name === "mode_switch" && call.args.context === "zenith"));
  expect(measured).toHaveLength(3);
  expect(measured.every((call) => Number.isFinite(call.args.durationMs) && call.args.durationMs! >= 0)).toBe(true);
});

test("row launch keeps all quota windows visible", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();

  await expect(page.getByRole("button", { name: "Launch selected" })).toHaveCount(0);
  await expect(page.locator(".quota-display-menu")).toHaveCount(0);
  await expect(page.locator(".account-list .quota-meter")).toHaveCount(2);
  const launch = page.getByRole("button", { name: "Launch in ChatGPT" });
  await expect(launch).toBeEnabled();
  await launch.click();
  await expect(page.getByText("Client launched.")).toBeVisible();
  await expect(page.getByRole("dialog", { name: /sessions visible|видимость чатов/i })).toHaveCount(0);

  await page.getByRole("button", { name: "Settings", exact: true }).click();
  await expect(page.getByText("Visible quota windows")).toHaveCount(0);

  const call = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: Record<string, unknown> }> }).__TAURI_TEST_INVOKES__.findLast((item) => item.command === "launch_codex_account"));
  expect(call?.args).toEqual({ accountId: "account_synthetic" });
  const profileCommands = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string }> }).__TAURI_TEST_INVOKES__.map((item) => item.command).filter((command) => ["launch_codex_account", "launch_managed_codex_profile"].includes(command)));
  expect(profileCommands).toEqual(["launch_codex_account", "launch_managed_codex_profile"]);
});

test("remote account export uses the capability-gated server command", async ({ page }) => {
  await installTauriMock(page, { mode: "remote", locale: "en", populated: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await page.locator(".account-bulk-menu summary").click();
  await page.getByRole("menuitem", { name: "Export all" }).click();
  const dialog = page.getByRole("dialog", { name: "Export accounts" });
  await expect(dialog.getByRole("radio", { name: "Zenith" })).toHaveAttribute("aria-checked", "true");
  await dialog.getByRole("button", { name: "Download JSON" }).click();

  const call = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: Record<string, unknown> }> }).__TAURI_TEST_INVOKES__.findLast((item) => item.command === "export_remote_accounts"));
  expect(call?.args.input).toEqual({ accountIds: ["account_synthetic"], format: "zenith", destination: "download" });
});

test("stored account proxy controls keep saved addresses hidden", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true });
  await page.goto("/");

  await page.getByRole("button", { name: "Connections", exact: true }).click();
  const accountCard = page.locator(".account-card").first();
  await accountCard.locator(".account-row-menu summary").click();
  await page.getByRole("menuitem", { name: "Proxy: Common", exact: true }).click();
  let accountDialog = page.getByRole("dialog", { name: "Account proxy" });
  await accountDialog.getByRole("radio", { name: /No proxy/ }).click();
  await accountDialog.getByRole("button", { name: "Save" }).click();
  await accountCard.locator(".account-row-menu summary").click();
  await expect(page.getByRole("menuitem", { name: "Proxy: No proxy", exact: true })).toBeVisible();
  await page.getByRole("menuitem", { name: "Proxy: No proxy", exact: true }).click();
  const accountProxy = "account-user:account-pass@us-account.example:8081";
  accountDialog = page.getByRole("dialog", { name: "Account proxy" });
  await accountDialog.getByRole("radio", { name: /Add a new proxy/ }).click();
  await accountDialog.getByLabel("HTTP(S) proxy").fill(accountProxy);
  await accountDialog.getByRole("button", { name: "Save" }).click();
  await expect(page.getByText(accountProxy)).toHaveCount(0);
  await accountCard.locator(".account-row-menu summary").click();
  await expect(page.getByRole("menuitem", { name: "Proxy: Per-account", exact: true })).toBeVisible();
  await page.keyboard.press("Escape");

  await page.locator(".account-bulk-menu summary").click();
  await page.getByRole("menuitem", { name: "Assign proxies" }).click();
  const bulkDialog = page.getByRole("dialog", { name: "Assign account proxies" });
  await bulkDialog.getByRole("button", { name: "Assign automatically" }).click();
  await expect(bulkDialog.getByRole("status")).toContainText("Assigned 0; unchanged 1; unavailable 0.");
  await expect(page.getByText("account-pass")).toHaveCount(0);

  const calls = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: Record<string, unknown> }> }).__TAURI_TEST_INVOKES__);
  expect(calls.some((call) => call.command === "set_local_account_proxy" && (call.args.input as { bypassCommonProxy?: boolean })?.bypassCommonProxy === true)).toBe(true);
  expect(calls.findLast((call) => call.command === "set_local_account_proxy")?.args).toEqual({ input: { accountId: "account_synthetic", proxyUrl: accountProxy, bypassCommonProxy: false } });
  expect(calls.findLast((call) => call.command === "assign_free_local_account_proxies")?.args).toEqual({ input: { accountIds: ["account_synthetic"] } });
});

test("remote proxy controls use the capability-gated management actions", async ({ page }) => {
  await installTauriMock(page, { mode: "remote", locale: "en", populated: true });
  await page.goto("/");

  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await page.locator(".account-card").first().locator(".account-row-menu summary").click();
  await page.getByRole("menuitem", { name: "Proxy: Common", exact: true }).click();
  const accountDialog = page.getByRole("dialog", { name: "Account proxy" });
  await accountDialog.getByRole("radio", { name: /Add a new proxy/ }).click();
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
  expect(actions).toEqual(["set_account_proxy", "assign_account_proxies"]);
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
  await page.getByLabel("Allow unencrypted HTTP").check();
  await page.getByLabel("Trust a new identity").check();
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
  await dialog.getByRole("button", { name: "Choose account files" }).click();
  const imported = dialog.getByLabel("Select Imported account for import");
  const secondImported = dialog.getByLabel("Select Second imported account for import");
  const existing = dialog.getByLabel("Select Existing account for import");
  await expect(imported).toBeChecked();
  await expect(secondImported).toBeChecked();
  await expect(existing).not.toBeChecked();
  await dialog.getByLabel("Add selected to pool after import").check();
  await dialog.getByRole("button", { name: "Import 2 account(s)" }).click();
  await page.getByRole("dialog", { name: "Add a regular account to the pool?", exact: true }).getByRole("button", { name: "Continue", exact: true }).click();
  await expect(dialog).toBeHidden();

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
        addToPool: true,
      },
    },
  ]);
});

test("account import preview selects all rows and exposes partial selection", async ({ page }) => {
  await installTauriMock(page, { mode: "remote", locale: "en", populated: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await page.getByRole("button", { name: "Import", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "Import accounts" });
  await dialog.getByRole("button", { name: "Choose account files" }).click();

  const selectAll = dialog.getByLabel("Select all records");
  const imported = dialog.getByLabel("Select Imported account for import");
  const secondImported = dialog.getByLabel("Select Second imported account for import");
  const existing = dialog.getByLabel("Select Existing account for import");

  await expect(selectAll).not.toBeChecked();
  await selectAll.check();
  await expect(imported).toBeChecked();
  await expect(secondImported).toBeChecked();
  await expect(existing).toBeChecked();
  await expect(dialog.getByRole("button", { name: "Import 3 account(s)" })).toBeVisible();

  await secondImported.uncheck();
  await expect(selectAll).not.toBeChecked();
  await expect(selectAll).toHaveAttribute("aria-checked", "mixed");
  await expect(selectAll).toHaveJSProperty("indeterminate", true);
  await expect(dialog.getByRole("button", { name: "Import 2 account(s)" })).toBeVisible();

  await selectAll.check();
  await expect(selectAll).toBeChecked();
  await selectAll.uncheck();
  await expect(imported).not.toBeChecked();
  await expect(secondImported).not.toBeChecked();
  await expect(existing).not.toBeChecked();
});

test("account import preview keeps existing and invalid rows selectable", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, importPreviewError: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await page.getByRole("button", { name: "Import", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "Import accounts" });
  await dialog.getByRole("button", { name: "Choose account files" }).click();

  const selectAll = dialog.getByLabel("Select all records");
  const imported = dialog.getByLabel("Select Imported account for import");
  const secondImported = dialog.getByLabel("Select Second imported account for import");
  const existing = dialog.getByLabel("Select Existing account for import");
  const invalid = dialog.getByLabel("Select Invalid imported account for import");

  await expect(existing).toBeEnabled();
  await expect(invalid).toBeEnabled();
  await invalid.check();
  await expect(invalid).toBeChecked();
  await expect(dialog.getByRole("button", { name: "Import 3 account(s)" })).toBeVisible();

  await selectAll.check();
  await expect(imported).toBeChecked();
  await expect(secondImported).toBeChecked();
  await expect(existing).toBeChecked();
  await expect(invalid).toBeChecked();
  await expect(dialog.getByRole("button", { name: "Import 4 account(s)" })).toBeVisible();
  await dialog.getByRole("button", { name: "Cancel" }).click();
});

test("remote server-side usage filters and clear logs use managed commands", async ({ page }) => {
  await installTauriMock(page, { mode: "remote", locale: "en", populated: true });
  await page.goto("/");

  await openGatewayApi(page);
  await expect(page.locator(".gateway-api-status")).toContainText("API is running");
  await expect(page.getByText("https://relay.example.invalid/v1")).toBeVisible();

  await page.getByRole("button", { name: "Usage", exact: true }).click();
  await chooseOption(page, page, "Model", "gpt-5.4");
  await chooseOption(page, page, "Pool member", "a1b2c3d4e5f6");
  await page.getByRole("button", { name: "More filters" }).click();
  await expect(page.getByLabel("Local key")).toHaveCount(0);
  await expect(page.getByRole("button", { name: /^Error category:/ })).toBeVisible();
  await page.getByRole("textbox", { name: "Request ID" }).fill("req_synthetic_remote");
  await expect(page.getByText("req_synthetic_remote")).toBeVisible();
  await expect.poll(() => page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: { input?: { modelQuery?: string; sourceOrAccountQuery?: string; requestIdQuery?: string } } }> }).__TAURI_TEST_INVOKES__.some((call) => call.command === "get_remote_server_usage" && call.args.input?.modelQuery === "gpt-5.4" && call.args.input?.sourceOrAccountQuery === "a1b2c3d4e5f6" && call.args.input?.requestIdQuery === "req_synthetic_remote"))).toBe(true);
  await page.getByLabel("Actions").click();
  await page.getByRole("menuitem", { name: "Clear logs" }).click();
  await settleConfirmation(page);
  await expect(page.getByText("Request logs cleared.")).toBeVisible();

  const calls = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: Record<string, unknown> }> }).__TAURI_TEST_INVOKES__);
  expect(calls.some((call) => call.command === "get_remote_server_state")).toBe(true);
  expect(calls.findLast((call) => call.command === "get_remote_server_usage" && (call.args.input as { modelQuery?: string } | undefined)?.modelQuery === "gpt-5.4")?.args.input).toMatchObject({ modelQuery: "gpt-5.4", sourceOrAccountQuery: "a1b2c3d4e5f6", requestIdQuery: "req_synthetic_remote" });
  expect(calls.findLast((call) => call.command === "execute_remote_server_action")?.args).toMatchObject({ input: { action: { type: "clear_usage" } } });
});

test("remote account usage uses the server usage identity without exposing its hash", async ({ page }) => {
  await installTauriMock(page, { mode: "remote", locale: "en", populated: true, distinctAccountIdentityHints: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Usage", exact: true }).click();
  await chooseOption(page, page, "Pool member", "account_synthetic");

  await expect(page.locator(".usage-pool-member-menu")).toContainText("p***@example.test");
  await expect.poll(() => page.evaluate(() => {
    const calls = (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: { input?: { sourceOrAccountQuery?: string } } }> }).__TAURI_TEST_INVOKES__;
    return calls.findLast((call) => call.command === "get_remote_server_usage")?.args.input?.sourceOrAccountQuery;
  })).toBe("account_synthetic");
  await expect(page.getByText("a1b2c3d4e5f6", { exact: true })).toHaveCount(0);
});

test("remote ChatGPT setup stays behind the managed profile command", async ({ page }) => {
  await installTauriMock(page, { mode: "remote", locale: "en", populated: true });
  await page.goto("/");
  await openGatewayApplication(page);

  await page.getByRole("button", { name: "Connect ChatGPT", exact: true }).click();
  await expect.poll(() => page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string }> }).__TAURI_TEST_INVOKES__.some((call) => call.command === "attach_codex_to_remote_gateway"))).toBe(true);
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
  await expect(page.getByRole("button", { name: "Show all account identities" })).toHaveCount(0);

  await openGatewayApplication(page);
  await expect(page.getByRole("button", { name: "Connect ChatGPT", exact: true })).toBeDisabled();
  await expect(page.locator(".gateway-settings-panel")).toHaveCount(0);
  await expect(page.locator(".proxy-settings")).toHaveCount(0);
  await page.getByRole("button", { name: "API", exact: true }).click();
  await page.locator(".relay-page-actions .relay-action-menu summary").click();
  await expect(page.getByRole("menuitem", { name: "Restart API" })).toBeDisabled();

  await page.getByRole("button", { name: "Pool", exact: true }).click();
  await expect(page.getByRole("tab", { name: "Client Access" })).toHaveCount(0);
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

test("overview presents time-based usage analytics for the local relay", async ({ page }) => {
  await installTauriMock(page, { locale: "en", mode: "local", theme: "light", populated: true });
  await page.goto("/");
  await expect(page.getByRole("heading", { name: "Usage over time" })).toBeVisible();
  await expect(page.getByText("Requests", { exact: true })).toHaveCount(2);
  await expect(page.locator(".overview-chart.requests .overview-chart-summary")).toHaveText("1");
  await expect(page.getByText("Token usage", { exact: true })).toBeVisible();
  await expect(page.getByText("API equivalent", { exact: true })).toBeVisible();
  await expect(page.locator(".overview-chart.cost .overview-chart-summary")).toHaveText("≈$0.000148");
  await expect(page.getByText("Generation speed", { exact: true })).toBeVisible();
  await expect(page.locator(".overview-chart.speed .overview-chart-summary")).toHaveText("6.7 tok/s");
  await expect(page.getByText("E2E speed", { exact: true })).toBeVisible();
  await expect(page.locator(".overview-chart.e2e-speed .overview-chart-summary")).toHaveText("18.7 tok/s");
  await expect(page.locator(".overview-analytics-header p")).toHaveCount(0);
  await expect(page.locator(".overview-chart-title small")).toHaveCount(0);
  await expect(page.locator(".activity-section")).toHaveCount(0);
  await expect(page.getByText("Runtime", { exact: true })).toHaveCount(0);
  await expect(page.getByText("Connections and capacity", { exact: true })).toHaveCount(0);
  await page.getByRole("tab", { name: "Week" }).click();
  await expect(page.getByRole("tab", { name: "Week" })).toHaveAttribute("aria-selected", "true");
});

test("overview asks the runtime for one aggregated series per selected period", async ({ page }) => {
  await installTauriMock(page, { locale: "en", mode: "local", theme: "light", populated: true });
  await page.goto("/");
  await page.getByRole("tab", { name: "Month" }).click();
  await expect.poll(() => page.evaluate(() => {
    const calls = (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: { input?: { range?: string; bucketMs?: number; fromMs?: number; toMs?: number } } }> }).__TAURI_TEST_INVOKES__;
    return calls.findLast((call) => call.command === "get_local_usage_page" && call.args.input?.bucketMs === 86_400_000)?.args.input;
  })).toMatchObject({ range: "custom", bucketMs: 86_400_000 });
});

test("overview analytics can be scoped to a concrete API or account", async ({ page }) => {
  await installTauriMock(page, { locale: "en", mode: "local", theme: "light", populated: true });
  await page.goto("/");
  const scope = page.getByRole("button", { name: /Analytics connection:/ });
  await scope.click();
  await page.getByRole("option", { name: "API · Example compatible API" }).click();
  await expect.poll(() => page.evaluate(() => {
    const calls = (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: { input?: { sourceOrAccountQuery?: string } } }> }).__TAURI_TEST_INVOKES__;
    return calls.findLast((call) => call.command === "get_local_usage_page")?.args.input?.sourceOrAccountQuery;
  })).toBe("source_synthetic");
});
