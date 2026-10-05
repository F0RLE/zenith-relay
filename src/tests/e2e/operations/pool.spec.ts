import { expect, test } from "../../bun-playwright";
import { emitTauriEvent, installTauriMock } from "../tauri-mock";
import { chooseOption, openGatewayApplication } from "./helpers";

for (const mode of ["local", "remote"] as const) {
  test(`${mode} pool stays off Usage and uses the lightweight runtime snapshot`, async ({ page }) => {
    await installTauriMock(page, {
      mode,
      locale: "en",
      populated: true,
      usageActive: false,
    });
    await page.goto("/");
    const usageCommand = mode === "local" ? "get_local_usage_page" : "get_remote_server_usage";
    const usageReads = () => page.evaluate((command) => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string }> }).__TAURI_TEST_INVOKES__.filter((call) => call.command === command).length, usageCommand);
    await expect.poll(usageReads).toBeGreaterThan(0);
    const before = await usageReads();
    await page.getByRole("button", { name: "Pool", exact: true }).click();
    await page.waitForTimeout(300);

    const routing = page.locator(".pool-controls");
    const member = page.locator('[data-member-label="Personal Plus"]');
    await expect(routing.locator("[data-ready-route]")).toHaveCount(0);
    expect(await usageReads()).toBe(before);
    await expect(member.locator(".pool-member-kind-icon")).toHaveAttribute("aria-label", "Waiting for quota");
  });
}

test("pool account avatar alone carries routing and quota refresh status", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, quotaAvailable: true, quotaRefreshStatus: "refreshing" });
  await page.goto("/");
  await page.getByRole("button", { name: "Pool", exact: true }).click();

  const member = page.locator('[data-member-label="Personal Plus"]');
  const indicator = member.locator(".pool-member-kind-icon");
  await expect(member.locator(".pool-member-state")).toHaveCount(0);
  await expect(indicator).toHaveAttribute("data-status", "disabled");
  await expect(indicator).toHaveAttribute("aria-label", "Checking quota · In rotation");
  await expect(indicator).not.toHaveClass(/refreshing/);
  await indicator.hover();
  await expect(page.getByRole("tooltip")).toHaveText("Checking quota · In rotation");
  expect(await member.locator(".pool-member-actions .relay-icon-button").evaluateAll((buttons) => buttons.every((button) => {
    const rect = button.getBoundingClientRect();
    return rect.width >= 60 && rect.height >= 44;
  }))).toBe(true);
});

test("pool account avatar opens the shared error details", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, accountCount: 3 });
  await page.goto("/");
  await page.getByRole("button", { name: "Pool", exact: true }).click();

  const member = page.locator('[data-member-label="Backup account"]');
  await member.getByRole("button", { name: "Connection error" }).click();
  const dialog = page.getByRole("dialog", { name: "Technical error details" });
  await expect(dialog).toBeVisible();
  await expect(dialog.locator("pre")).toContainText('"code": "quota_transport"');
});

test("pool places unavailable accounts after ready members", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, accountCount: 4, accountAuthReason: "invalid_grant" });
  await page.goto("/");
  await page.getByRole("button", { name: "Pool", exact: true }).click();

  await expect(page.locator(".pool-member-card").first()).toHaveAttribute("data-member-label", "Backup account");
  await expect(page.locator(".pool-member-card").last()).toHaveAttribute("data-member-label", "Personal Plus");
});

test("pool sign-in status starts reauthentication for that account", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, accountCount: 3, quotaAvailable: false, accountAuthReason: "invalid_grant" });
  await page.goto("/");
  await page.getByRole("button", { name: "Pool", exact: true }).click();

  const member = page.locator('[data-member-label="Personal Plus"]');
  const signIn = member.getByRole("button", { name: "Sign-in required", exact: true });
  await expect(signIn).toBeVisible();
  await signIn.click();
  await expect(page.getByRole("dialog", { name: "Sign in" })).toBeVisible();
  const oauthStart = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: Record<string, unknown> }> }).__TAURI_TEST_INVOKES__.findLast((call) => call.command === "start_codex_oauth"));
  expect(oauthStart?.args).toEqual({ openBrowser: false, accountId: "account_synthetic" });
});

test("connections show the same sign-in action instead of stale quota data", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, quotaAvailable: true, accountAuthReason: "invalid_grant" });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();

  const account = page.locator(".account-card").filter({ hasText: "Personal Plus" });
  const signIn = account.getByRole("button", { name: "Sign-in required", exact: true });
  await expect(account.locator(".quota-meter")).toHaveCount(0);
  await expect(signIn).toBeVisible();
  await signIn.click();
  await expect(page.getByRole("dialog", { name: "Sign in" })).toBeVisible();
  const oauthStart = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: Record<string, unknown> }> }).__TAURI_TEST_INVOKES__.findLast((call) => call.command === "start_codex_oauth"));
  expect(oauthStart?.args).toEqual({ openBrowser: false, accountId: "account_synthetic" });
});

test("connections stay outside the pool until the user adds selected members", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, accountCount: 3, poolMembers: false, gatewayRunning: false });
  await page.goto("/");
  await page.getByRole("button", { name: "Pool", exact: true }).click();
  await expect(page.getByText("No pool members", { exact: true })).toBeVisible();
  // The pool header no longer owns the endpoint power toggle; Overview does.
  await expect(page.getByRole("button", { name: "Start pool", exact: true })).toHaveCount(0);

  await page.getByRole("button", { name: "Add member", exact: true }).first().click();
  const dialog = page.getByRole("dialog", { name: "Add connections to pool" });
  await dialog.getByText("Business Workspace", { exact: true }).click();
  await dialog.getByRole("button", { name: "Add selected (1)" }).click();

  const rows = page.locator(".pool-member-card");
  await expect(rows).toHaveCount(1);
  await expect(rows.first()).toContainText("Business Workspace");
  await rows.first().getByRole("button", { name: "Remove from pool: Business Workspace" }).click();
  const confirmation = page.getByRole("dialog", { name: "Confirm action" });
  await expect(confirmation).toContainText("Remove Business Workspace from the pool?");
  await confirmation.getByRole("button", { name: "Remove from pool" }).click();
  await expect(page.getByText("No pool members", { exact: true })).toBeVisible();
  const calls = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: Record<string, unknown> }> }).__TAURI_TEST_INVOKES__.filter((item) => item.command === "set_local_pool_membership"));
  expect(calls.map((call) => call.args)).toEqual([
    { input: { accountIds: ["account_synthetic_2"], sourceIds: [], inPool: true } },
    { input: { accountIds: ["account_synthetic_2"], sourceIds: [], inPool: false } },
  ]);
});

test("pool member removal on right click skips confirmation", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, accountCount: 3 });
  await page.goto("/");
  await page.getByRole("button", { name: "Pool", exact: true }).click();

  const member = page.locator(".pool-member-card").first();
  const memberLabel = await member.getAttribute("data-member-label");
  expect(memberLabel).toBeTruthy();
  const remove = member.getByRole("button", { name: /Remove from pool:/ });
  await remove.click({ button: "right" });

  await expect(page.getByRole("dialog", { name: "Confirm action" })).toHaveCount(0);
  await expect(page.locator(".pool-member-card").filter({ hasText: memberLabel! })).toHaveCount(0);
  await expect.poll(() => page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string }> }).__TAURI_TEST_INVOKES__.filter((call) => call.command === "set_local_pool_membership").length)).toBe(1);
});

for (const mode of ["local", "remote"] as const) {
  test(`${mode} API sources expose routing role and pool membership`, async ({ page }) => {
    await installTauriMock(page, { mode, locale: "en", populated: true });
    await page.goto("/");
    await page.getByRole("button", { name: "Connections", exact: true }).click();
    await page.getByRole("tab", { name: "Sources" }).click();

    const row = page.getByRole("row").filter({ hasText: "Example compatible API" });
    await expect(row).not.toContainText("Stabilizer");
    await row.locator(".relay-action-menu summary").click();
    await page.getByRole("menuitem", { name: "Remove from pool" }).click();
    await row.locator(".relay-action-menu summary").click();
    await page.getByRole("menuitem", { name: "Add to pool" }).click();

    const calls = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: Record<string, unknown> }> }).__TAURI_TEST_INVOKES__);
    if (mode === "local") {
      expect(calls.filter((call) => call.command === "set_local_pool_membership").map((call) => call.args)).toEqual([
        { input: { accountIds: [], sourceIds: ["source_synthetic"], inPool: false } },
        { input: { accountIds: [], sourceIds: ["source_synthetic"], inPool: true } },
      ]);
    } else {
      expect(calls.filter((call) => call.command === "execute_remote_server_action").map((call) => call.args.input).filter((input) => (input as { action?: { type?: string } }).action?.type === "set_pool_membership")).toEqual([
        { action: { type: "set_pool_membership" }, payload: { accountIds: [], sourceIds: ["source_synthetic"], inPool: false } },
        { action: { type: "set_pool_membership" }, payload: { accountIds: [], sourceIds: ["source_synthetic"], inPool: true } },
      ]);
    }
  });

  test(`${mode} pool creates an API source through the shared picker and applies shared rotation modes`, async ({ page }) => {
    await installTauriMock(page, { mode, locale: "en", populated: false, gatewayRunning: false });
    await page.goto("/");
    await page.getByRole("button", { name: "Pool", exact: true }).click();
    await page.getByRole("button", { name: "Add member", exact: true }).first().click();
    await page.getByRole("dialog", { name: "Add connections to pool" }).getByRole("button", { name: "Add API source" }).click();

    const sourceDialog = page.getByRole("dialog", { name: "Add API source" });
    await sourceDialog.getByRole("radio", { name: /Custom API/ }).click();
    await sourceDialog.getByLabel("Name", { exact: true }).fill("Failover API");
    await sourceDialog.getByLabel("API address").fill("https://failover.example.invalid/v1");
    await sourceDialog.getByLabel("Upstream API key").fill("synthetic-upstream-key");
    await sourceDialog.getByRole("button", { name: "Save" }).click();
    await expect(page.getByRole("dialog", { name: "Add API source" })).toBeHidden();

    const member = page.locator(".pool-member-card").filter({ hasText: "Failover API" });
    await expect(member).toContainText("Automatic");
    for (const name of ["Manual"]) {
      await page.getByRole("button", { name: "Pool rotation settings", exact: true }).click();
      const editor = page.getByRole("dialog", { name: "Pool rotation", exact: true });
      await editor.getByRole("radio", { name, exact: true }).click();
      await editor.getByRole("button", { name: "Close", exact: true }).last().click();
      await expect(member).toContainText(name);
    }

    const calls = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: Record<string, unknown> }> }).__TAURI_TEST_INVOKES__);
    if (mode === "local") {
      expect(calls.find((call) => call.command === "create_local_source")?.args.input).toMatchObject({ name: "Failover API", priority: 0 });
      expect(calls.find((call) => call.command === "set_local_pool_membership")?.args).toEqual({ input: { accountIds: [], sourceIds: ["source_created_1"], inPool: true } });
      expect(calls.filter((call) => call.command === "update_local_routing")).toHaveLength(1);
    } else {
      const actions = calls.filter((call) => call.command === "execute_remote_server_action").map((call) => call.args.input as { action: { type: string }; payload?: Record<string, unknown> });
      expect(actions.find((call) => call.action.type === "create_source")?.payload).toMatchObject({ name: "Failover API", priority: 0 });
      expect(actions.find((call) => call.action.type === "set_pool_membership")?.payload).toMatchObject({ sourceIds: ["source_remote_created_1"] });
      expect(actions.filter((call) => call.action.type === "set_routing_policy")).toHaveLength(1);
    }
  });

}

test("pool keeps access key management internal", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Pool", exact: true }).click();
  await expect(page.locator(".relay-tabs").getByRole("tab")).toHaveText(["Members", "Model Rules"]);
  await expect(page.getByRole("tab", { name: "Client Access" })).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Create access key" })).toHaveCount(0);
});

test("pool preserves scheduler priority within availability groups", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, accountCount: 4, usageAccountIndex: 3 });
  await page.goto("/");
  await page.getByRole("button", { name: "Pool", exact: true }).click();
  await expect(page.locator(".pool-sort-menu")).toHaveCount(0);
  const priority = page.locator(".pool-controls");
  await expect(priority).toContainText("Usage order");
  await expect(priority).toContainText("Active now: Pro account");
  await expect(priority.locator("[data-active-models]")).toHaveAttribute("data-active-models", "gpt-5.4:1");
  await expect(priority.locator("[data-active-models]")).toHaveText("Active now (1): gpt-5.4");
  await expect(page.locator(".pool-member-card").first()).toHaveAttribute("data-member-label", "Backup account");
  const current = page.locator('[data-member-label="Pro account"]');
  await expect(current).toHaveAttribute("data-current", "true");
  const names = () => page.locator(".pool-member-card").evaluateAll((items) => items.map((item) => item.getAttribute("data-member-label") ?? ""));
  expect(await names()).toEqual(["Backup account", "Pro account", "Business Workspace", "Example compatible API", "Personal Plus"]);
  await expect(page.locator(".pool-member-list")).not.toContainText("Priority 30");
});

test("pool groups concurrent requests by their active model", async ({ page }) => {
  await installTauriMock(page, {
    mode: "local",
    locale: "en",
    populated: true,
    accountCount: 4,
    usageAccountIndex: 3,
    activeModelCounts: [
      { model: "gpt-5.4", requestCount: 3 },
      { model: "gpt-5.4-mini", requestCount: 2 },
    ],
  });
  await page.goto("/");
  await page.getByRole("button", { name: "Pool", exact: true }).click();

  const activeModels = page.locator(".pool-controls [data-active-models]");
  await expect(activeModels).toHaveAttribute("data-active-request-count", "5");
  await expect(activeModels).toHaveAttribute("data-active-models", "gpt-5.4:3,gpt-5.4-mini:2");
  await expect(activeModels).toHaveText("Active now (5): gpt-5.4 ×3 · gpt-5.4-mini ×2");
  const current = page.locator('.pool-member-card[data-member-label="Pro account"]');
  await expect(current).toHaveAttribute("data-current", "true");
  await expect(current.locator(".pool-member-active-runtime")).toHaveCount(0);
});

test("pool reflects active models from the live runtime order", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, usageActive: false });
  await page.goto("/");
  await page.getByRole("button", { name: "Pool", exact: true }).click();
  await expect(page.locator('.pool-controls [data-ready-route="source_synthetic"]')).toHaveCount(0);

  await page.evaluate(() => {
    const internals = (window as unknown as { __TAURI_INTERNALS__: { invoke: (command: string, args?: unknown, options?: unknown) => Promise<unknown> } }).__TAURI_INTERNALS__;
    const invoke = internals.invoke.bind(internals);
    internals.invoke = async (command, args, options) => {
      const result = await invoke(command, args, options);
      if (command !== "get_local_runtime_order") return result;
      const order = structuredClone(result) as Array<{
        candidateId: string;
        inFlight: number;
        activeRequestCount: number;
        activeModels: Array<{ model: string; requestCount: number }>;
      }>;
      const account = order.find((candidate) => candidate.candidateId === "account_synthetic");
      if (account) {
        account.inFlight = 3;
        account.activeRequestCount = 3;
        account.activeModels = [
          { model: "gpt-5.4", requestCount: 2 },
          { model: "gpt-5.4-mini", requestCount: 1 },
        ];
      }
      return order;
    };
    (window as unknown as { __TAURI_TEST_EMIT__: (event: string, payload: unknown) => void }).__TAURI_TEST_EMIT__("zenith-runtime-activity", {
      revision: 1,
      candidateId: "account_synthetic",
      inFlight: 3,
      activeRequestCount: 3,
      activeModels: [
        { model: "gpt-5.4", requestCount: 2 },
        { model: "gpt-5.4-mini", requestCount: 1 },
      ],
    });
  });

  const activeModels = page.locator(".pool-controls [data-active-models]");
  await expect(activeModels).toHaveText("Active now (3): gpt-5.4 ×2 · gpt-5.4-mini");
  const current = page.locator('[data-member-label="Personal Plus"]');
  await expect(current).toHaveAttribute("data-current", "true");
  await expect(current.locator(".pool-member-active-runtime")).toHaveCount(0);
});

test("pool refreshes the active member immediately on a runtime activity event", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, usageActive: false });
  await page.goto("/");
  await page.getByRole("button", { name: "Pool", exact: true }).click();
  await expect.poll(() => page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string }> }).__TAURI_TEST_INVOKES__.some((call) => call.command === "plugin:event|listen"))).toBe(true);

  await page.evaluate(() => {
    const internals = (window as unknown as { __TAURI_INTERNALS__: { invoke: (command: string, args?: unknown, options?: unknown) => Promise<unknown> } }).__TAURI_INTERNALS__;
    const invoke = internals.invoke.bind(internals);
    internals.invoke = async (command, args, options) => {
      const result = await invoke(command, args, options);
      if (command !== "get_local_runtime_order") return result;
      const order = structuredClone(result) as Array<{ candidateId: string; inFlight: number; activeRequestCount: number; activeModels: Array<{ model: string; requestCount: number }> }>;
      const account = order.find((candidate) => candidate.candidateId === "account_synthetic");
      if (account) {
        account.inFlight = 1;
        account.activeRequestCount = 1;
        account.activeModels = [{ model: "gpt-5.4", requestCount: 1 }];
      }
      return order;
    };
    (window as unknown as { __TAURI_TEST_EMIT__: (event: string, payload: unknown) => void }).__TAURI_TEST_EMIT__("zenith-runtime-activity", {
      revision: 1,
      candidateId: "account_synthetic",
      inFlight: 1,
      activeRequestCount: 1,
      activeModels: [{ model: "gpt-5.4", requestCount: 1 }],
    });
  });

  const current = page.locator('[data-member-label="Personal Plus"]');
  await expect(current).toHaveAttribute("data-current", "true");
  await expect(current.locator(".pool-member-active-runtime")).toHaveCount(0);

  await page.evaluate(() => {
    (window as unknown as { __TAURI_TEST_EMIT__: (event: string, payload: unknown) => void }).__TAURI_TEST_EMIT__("zenith-runtime-activity", {
      revision: 2,
      candidateId: "account_synthetic",
      inFlight: 0,
      activeRequestCount: 0,
      activeModels: [],
    });
  });
  await expect(current).toHaveAttribute("data-current", "false");
  await expect(current.locator(".pool-member-active-runtime")).toHaveCount(0);
});

test("pool keeps the last completed route visible after its lease is released", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, accountCount: 4, usageAccountIndex: 3, usageActive: false });
  await page.goto("/");
  await page.getByRole("button", { name: "Pool", exact: true }).click();
  const priority = page.locator(".pool-controls");
  await expect(priority).toContainText("Usage order");
  await expect(priority).toContainText("Next candidate: Pro account");
  await expect(priority.locator("[data-ready-route]")).toHaveCount(0);
  await expect(page.locator('.pool-member-card[data-member-label="Pro account"]')).toHaveAttribute("data-last-used", "true");
  await expect(page.locator('.pool-member-card[data-member-label="Pro account"]')).toHaveAttribute("data-next", "true");
  await expect(page.locator(".pool-member-card[data-current=true]")).toHaveCount(0);
});

test("pool does not show the next route's models before any request", async ({ page }) => {
  await installTauriMock(page, {
    mode: "local",
    locale: "en",
    populated: true,
    mixedModels: true,
    quotaAvailable: true,
    usagePresent: false,
    usageActive: false,
  });
  await page.goto("/");
  await page.getByRole("button", { name: "Pool", exact: true }).click();

  const priority = page.locator(".pool-controls");
  await expect(priority).toContainText("Next candidate: Personal Plus");
  await expect(priority.locator("[data-ready-route]")).toHaveCount(0);
  await expect(priority).not.toContainText("gpt-5.4");
  await expect(priority).not.toContainText("claude-opus-4-8");
});

test("pool follows a lower-priority stabilizer reported by runtime activity", async ({ page }) => {
  await installTauriMock(page, {
    mode: "local",
    locale: "en",
    populated: true,
    usagePresent: false,
    usageActive: false,
    accountCount: 1,
  });
  await page.goto("/");
  await page.getByRole("button", { name: "Pool", exact: true }).click();
  await expect.poll(() => page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string }> }).__TAURI_TEST_INVOKES__.some((call) => call.command === "plugin:event|listen"))).toBe(true);

  await emitTauriEvent(page, "zenith-runtime-activity", {
    revision: 1,
    candidateId: "source_synthetic",
    inFlight: 1,
    activeRequestCount: 1,
    activeModels: [{ model: "claude-opus-4-8", requestCount: 1 }],
  });
  // A quota/usage state notification can arrive while the request is still
  // running. It must not clear the activity overlay before the next snapshot.
  await emitTauriEvent(page, "zenith-state-changed", null);

  const priority = page.locator(".pool-controls");
  await expect(priority).toContainText("Active now: Example compatible API");
  await expect(priority).toContainText("Active now (1): claude-opus-4-8");
  await expect(priority).not.toContainText("Next candidate");
  await expect(page.locator('[data-member-label="Example compatible API"]')).toHaveAttribute("data-current", "true");
  await expect(page.locator('[data-member-label="Personal Plus"]')).toHaveAttribute("data-current", "false");

  await emitTauriEvent(page, "zenith-runtime-activity", {
    revision: 2,
    candidateId: "source_synthetic",
    inFlight: 0,
    activeRequestCount: 0,
    activeModels: [],
  });
  await expect(priority).toContainText("Last request: Example compatible API");
  await expect(priority).not.toContainText("Next candidate");
  await expect(page.locator(".pool-member-card").first()).toHaveAttribute("data-member-label", "Example compatible API");
  await expect(page.locator('[data-member-label="Example compatible API"]')).toHaveAttribute("data-current", "false");
});

test("pool member picker lists individual accounts instead of subscription groups", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, accountCount: 4, poolMembers: false });
  await page.goto("/");
  await page.getByRole("button", { name: "Pool", exact: true }).click();
  await page.getByRole("button", { name: "Add member", exact: true }).first().click();
  const dialog = page.getByRole("dialog", { name: "Add connections to pool" });
  const accountRows = dialog.getByRole("region", { name: "Accounts", exact: true }).locator(":scope > label");
  await expect(accountRows).toHaveCount(4);
  await expect(accountRows.locator("strong")).toHaveText(["Pro account", "Business Workspace", "Personal Plus", "Backup account"]);
  // One account per row with its own routing status. A subscription group would
  // instead add a second, unnamed line describing the group.
  await expect(accountRows.locator("small")).toHaveCount(4);
  await expect(accountRows.locator("small.pool-picker-status")).toHaveCount(4);
  await expect(accountRows.nth(2).locator("small.pool-picker-status")).toContainText("Waiting for quota");
  await expect(accountRows.locator(".account-plan-badge")).toHaveText(["Pro 200", "Business", "Plus", "Free"]);

  await dialog.getByRole("navigation").getByRole("button", { name: "Accounts", exact: true }).click();
  await chooseOption(page, dialog, "Filter by plan", "business");
  await expect(accountRows).toHaveCount(1);
  await expect(accountRows).toContainText("Business Workspace");
  await dialog.getByRole("checkbox", { name: "Select shown", exact: true }).check();
  await expect(dialog.getByRole("button", { name: "Add selected (1)" })).toBeEnabled();

  await dialog.getByRole("navigation").getByRole("button", { name: "All connections", exact: true }).click();
  await dialog.getByRole("checkbox", { name: "Select shown", exact: true }).check();
  await expect(dialog.getByRole("button", { name: "Add selected (5)" })).toBeEnabled();
  await dialog.getByRole("button", { name: "Clear selection" }).click();
  await expect(dialog.getByRole("button", { name: "Add selected (0)" })).toBeDisabled();

  await dialog.getByRole("navigation").getByRole("button", { name: "Accounts", exact: true }).click();
  await chooseOption(page, dialog, "Filter by plan", "all");
  await dialog.getByLabel("Find a connection").fill("team");
  await expect(accountRows).toHaveCount(1);
  await expect(accountRows).toContainText("Business Workspace");
});

test("pool members use one responsive card grid", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, accountCount: 4 });
  await page.goto("/");
  await page.getByRole("button", { name: "Pool", exact: true }).click();
  const members = page.locator(".pool-member-list");
  await expect(members.locator('.account-plan-badge[data-plan="plus"]')).toBeVisible();
  await expect(members.locator('.account-plan-badge[data-plan="business"]')).toBeVisible();
  await expect(page.getByRole("button", { name: "Compact pool view" })).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Pool card grid" })).toHaveCount(0);
  await expect(members.locator(".pool-member-rank")).toHaveCount(0);
  await page.setViewportSize({ width: 2048, height: 1152 });
  expect(await members.evaluate((list) => getComputedStyle(list).gridTemplateColumns.split(" ").filter((track) => Number.parseFloat(track) > 1).length)).toBeGreaterThan(0);
  expect(await members.locator(".pool-member-card").evaluateAll((cards) => cards.every((card) => card.getBoundingClientRect().width <= 360))).toBe(true);
  expect(await page.evaluate(() => localStorage.getItem("relay.poolLayout"))).toBeNull();
});

test("local pool refreshes all account quotas without an interval setting", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, accountCount: 3, freeAccountHealthy: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Pool", exact: true }).click();

  const freeMember = page.locator('[data-member-label="Backup account"]');
  await expect(freeMember.locator('.relay-status-icon[aria-label^="In rotation"]')).toBeVisible();

  await page.locator('[data-toolbar-group="refresh"]').getByRole("button", { name: "Refresh", exact: true }).click();
  await expect(page.getByText("Updated: 3 · Errors: 0", { exact: true })).toBeVisible();
  await expect(page.getByRole("button", { name: "Quota refresh settings", exact: true })).toHaveCount(0);
  await expect(freeMember.locator(".relay-status-icon")).toHaveAttribute("aria-label", "In rotation");

  const calls = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: Record<string, unknown> }> }).__TAURI_TEST_INVOKES__);
  expect(calls.some((call) => call.command === "refresh_all_local_account_quotas")).toBe(true);
});

test("local pool saves adaptive distribution without chat pinning", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Pool", exact: true }).click();
  const personalPlus = page.locator('[data-member-label="Personal Plus"]');
  const subscription = personalPlus.locator(".account-subscription-line");
  await expect(subscription.locator("span").first()).toHaveText(/\d{1,2}\/\d{1,2}\/\d{4}/);
  await expect(subscription.locator(".account-subscription-countdown")).toHaveText(/^\d+ d \d+ h \d+ min$/);
  await expect(personalPlus.locator(".quota-meter-heading small").first()).toHaveText(/^\d+ h \d+ min$/);
  await expect(personalPlus.locator(".quota-meter-heading small").nth(1)).toHaveText(/^\d+ d \d+ h \d+ min$/);

  const speed = page.getByRole("radiogroup", { name: "Request speed" });
  await expect(speed.getByRole("radio", { name: "Standard" })).toBeChecked();
  await speed.getByRole("radio", { name: "Fast", exact: true }).click();
  await expect(speed.getByRole("radio", { name: "Fast", exact: true })).toBeEnabled();
  await speed.getByRole("radio", { name: "Standard", exact: true }).click();
  await expect(speed.getByRole("radio", { name: "Standard", exact: true })).toBeEnabled();
  await speed.getByRole("radio", { name: "Fast", exact: true }).click();
  await expect(speed.getByRole("radio", { name: "Fast", exact: true })).toBeEnabled();

  await page.getByRole("button", { name: "Pool rotation settings", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "Pool rotation" });
  await expect(dialog).not.toContainText("Request speed");
  await expect(dialog).not.toContainText("Keep one chat on one account");
  await expect(dialog).not.toContainText("Accounts tried after an error");
  await expect(dialog.getByRole("radio")).toHaveCount(2);
  await expect(dialog.getByRole("radio", { name: "Automatic", exact: true })).toHaveAttribute("aria-checked", "true");
  await dialog.getByRole("radio", { name: "Manual", exact: true }).click();
  await expect(dialog.getByLabel("Retry candidates")).toHaveCount(0);
  await expect(dialog.getByLabel("Failures before cooldown")).toHaveCount(0);
  await dialog.getByRole("button", { name: "Close", exact: true }).last().click();
  await expect(subscription.locator("span").first()).toHaveText(/\d{1,2}\/\d{1,2}\/\d{4}/);
  await expect(subscription.locator(".account-subscription-countdown")).toHaveText(/^\d+ d \d+ h \d+ min$/);

  const calls = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: Record<string, unknown> }> }).__TAURI_TEST_INVOKES__);
  const routingInput = calls.findLast((call) => call.command === "update_local_routing")?.args.input;
  expect(routingInput).toMatchObject({ poolRouting: { mode: "in_order" }, expectedPoolRouting: { mode: "automatic" }, maxRetryCandidates: 3, defaultServiceTier: "fast" });
  expect(routingInput).not.toHaveProperty("cooldownAfterFailures");
  expect(routingInput).not.toHaveProperty("keepLastCandidateAvailable");
  expect(routingInput).not.toHaveProperty("routingStrategy");
  expect(routingInput).not.toHaveProperty("subscriptionPlanOrder");
});

test("pool card grid preserves scheduler order at every width", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "ru", populated: true, accountCount: 8, quotaAvailable: true, usageActive: false });
  await page.goto("/");
  await page.getByRole("button", { name: "Пул", exact: true }).click();

  const members = page.locator(".pool-member-list");
  const labels = () => members.locator(".pool-member-card").evaluateAll((cards) => cards.map((card) => card.getAttribute("data-member-label")));
  await expect(members.locator(".pool-member-rank")).toHaveCount(0);
  await expect(page.getByRole("radio", { name: "Компактный вид пула" })).toHaveCount(0);
  await expect(members.locator(".pool-member-card-quota").first()).toBeVisible();
  const expectedOrder = await labels();

  await page.setViewportSize({ width: 2048, height: 1152 });
  expect(await labels()).toEqual(expectedOrder);
  expect(await members.evaluate((list) => getComputedStyle(list).gridTemplateColumns.split(" ").filter((track) => Number.parseFloat(track) > 1).length)).toBeGreaterThan(1);
  expect(await members.locator(".pool-member-card").evaluateAll((cards) => cards.every((card) => card.getBoundingClientRect().width <= 360))).toBe(true);
  expect(await members.evaluate((list) => list.scrollWidth <= list.clientWidth)).toBe(true);

  await page.setViewportSize({ width: 840, height: 900 });
  expect(await labels()).toEqual(expectedOrder);
  expect(await members.evaluate((list) => getComputedStyle(list).gridTemplateColumns.split(" ").filter((track) => Number.parseFloat(track) > 1).length)).toBe(2);
  expect(await members.evaluate((list) => list.scrollWidth <= list.clientWidth)).toBe(true);

  await page.reload();
  await page.getByRole("button", { name: "Пул", exact: true }).click();
  await expect(members.locator(".pool-member-card").first()).toBeVisible();
  expect(await labels()).toEqual(expectedOrder);
  expect(await page.locator(".pool-member-list").evaluate((list) => list.scrollWidth <= list.clientWidth)).toBe(true);
  expect(await page.evaluate(() => localStorage.getItem("relay.poolLayout"))).toBeNull();
});

test("closing mixed rotation retains the immediately applied order", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, accountCount: 4 });
  await page.goto("/");
  await page.getByRole("button", { name: "Pool", exact: true }).click();
  await page.getByRole("button", { name: "Pool rotation settings", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "Pool rotation", exact: true });
  await dialog.getByRole("radio", { name: "Manual", exact: true }).click();
  const ids = () => dialog.getByRole("listitem").evaluateAll((rows) => rows.map((row) => row.getAttribute("data-member-id")));
  const original = await ids();
  await dialog.getByRole("listitem").first().getByRole("button", { name: / down$/ }).click();
  expect(await ids()).not.toEqual(original);
  const reordered = await ids();
  await dialog.getByRole("button", { name: "Close", exact: true }).last().click();
  await page.getByRole("button", { name: "Pool rotation settings", exact: true }).click();
  await expect(dialog.getByRole("radio", { name: "Manual", exact: true })).toHaveAttribute("aria-checked", "true");
  expect(await ids()).toEqual(reordered);
});

test("remote pool saves distribution settings on the connected runtime", async ({ page }) => {
  await installTauriMock(page, { mode: "remote", locale: "en", populated: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Pool", exact: true }).click();
  const speed = page.getByRole("radiogroup", { name: "Request speed" });
  await speed.getByRole("radio", { name: "Fast", exact: true }).click();
  await expect(speed.getByRole("radio", { name: "Fast", exact: true })).toBeEnabled();
  await page.getByRole("button", { name: "Pool rotation settings", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "Pool rotation" });
  await expect(dialog).not.toContainText("Keep one chat on one account");
  await expect(dialog).not.toContainText("Request speed");
  await dialog.getByRole("radio", { name: "Manual", exact: true }).click();
  await dialog.getByRole("button", { name: "Close", exact: true }).last().click();

  const calls = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: Record<string, unknown> }> }).__TAURI_TEST_INVOKES__);
  expect(calls.findLast((call) => call.command === "execute_remote_server_action")?.args.input).toMatchObject({
    action: { type: "set_routing_policy" },
    payload: { poolRouting: { mode: "in_order" }, expectedPoolRouting: { mode: "automatic" }, maxRetryCandidates: 3, defaultServiceTier: "fast" },
  });
  const remoteRoutingPayload = calls.findLast((call) => call.command === "execute_remote_server_action")?.args.input as { payload: Record<string, unknown> };
  expect(remoteRoutingPayload.payload).not.toHaveProperty("cooldownAfterFailures");
  expect(remoteRoutingPayload.payload).not.toHaveProperty("keepLastCandidateAvailable");
  expect(remoteRoutingPayload.payload).not.toHaveProperty("routingStrategy");
  expect(remoteRoutingPayload.payload).not.toHaveProperty("subscriptionPlanOrder");
  expect(calls.findLast((call) => call.command === "sync_codex_default_service_tier")?.args).toEqual({ defaultServiceTier: "fast" });
});

test("remote configuration presets require preview before an explicit apply", async ({ page }) => {
  await installTauriMock(page, { mode: "remote", locale: "en", populated: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Pool", exact: true }).click();

  await page.locator(".pool-preset-menu summary").click();
  await page.getByRole("menuitem", { name: "Save preset", exact: true }).click();
  await page.locator(".pool-preset-menu summary").click();
  await page.getByRole("menuitem", { name: "Apply preset", exact: true }).click();

  const dialog = page.getByRole("dialog", { name: "Configuration preset" });
  await expect(dialog.getByText("Changes: 1", { exact: true })).toBeVisible();
  await expect(dialog.getByText("routing / maxRetryCandidates", { exact: true })).toBeVisible();
  await expect(dialog.getByRole("cell", { name: "3", exact: true })).toBeVisible();
  await expect(dialog.getByRole("cell", { name: "4", exact: true })).toBeVisible();
  await dialog.getByRole("button", { name: "Apply changes" }).click();
  await expect(dialog).toBeHidden();

  const calls = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: Record<string, unknown> }> }).__TAURI_TEST_INVOKES__);
  expect(calls.some((call) => call.command === "export_remote_configuration_preset")).toBe(true);
  expect(calls.some((call) => call.command === "preview_remote_configuration_preset")).toBe(true);
  expect(calls.findLast((call) => call.command === "apply_remote_configuration_preset")?.args).toMatchObject({ input: { baseRevision: "cfg_synthetic_current" } });
});

test("local configuration presets require preview before an explicit apply", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Pool", exact: true }).click();

  await page.locator(".pool-preset-menu summary").click();
  await page.getByRole("menuitem", { name: "Save preset", exact: true }).click();
  await page.locator(".pool-preset-menu summary").click();
  await page.getByRole("menuitem", { name: "Apply preset", exact: true }).click();

  const dialog = page.getByRole("dialog", { name: "Configuration preset" });
  await expect(dialog.getByText("Changes: 1", { exact: true })).toBeVisible();
  await dialog.getByRole("button", { name: "Apply changes" }).click();
  await expect(dialog).toBeHidden();

  const calls = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: Record<string, unknown> }> }).__TAURI_TEST_INVOKES__);
  expect(calls.some((call) => call.command === "export_local_configuration_preset")).toBe(true);
  expect(calls.some((call) => call.command === "preview_local_configuration_preset")).toBe(true);
  expect(calls.findLast((call) => call.command === "apply_local_configuration_preset")?.args).toMatchObject({ input: { baseRevision: "cfg_synthetic_current" } });
});

test("remote pool refreshes quotas without exposing an interval setting", async ({ page }) => {
  await installTauriMock(page, { mode: "remote", locale: "en", populated: true, accountCount: 3, freeAccountHealthy: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Pool", exact: true }).click();

  await page.locator('[data-toolbar-group="refresh"]').getByRole("button", { name: "Refresh", exact: true }).click();
  await expect(page.getByRole("button", { name: "Quota refresh settings", exact: true })).toHaveCount(0);

  const actions = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: { input?: { action?: { type?: string }; payload?: unknown } } }> }).__TAURI_TEST_INVOKES__.filter((call) => call.command === "execute_remote_server_action").map((call) => call.args.input));
  expect(actions).toContainEqual({ action: { type: "refresh_all_quotas" }, payload: null });
  expect(actions.some((input) => input?.action?.type === "set_quota_policy")).toBe(false);
});

test("remote connections expose the same account refresh action as the pool", async ({ page }) => {
  await installTauriMock(page, { mode: "remote", locale: "en", populated: true, accountCount: 3, freeAccountHealthy: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();

  const refreshAll = page.locator(".account-command-actions").getByRole("button", { name: "Refresh", exact: true });
  await expect(refreshAll).toBeVisible();
  await refreshAll.click();

  const actions = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: { input?: { action?: { type?: string }; payload?: unknown } } }> }).__TAURI_TEST_INVOKES__.filter((call) => call.command === "execute_remote_server_action").map((call) => call.args.input));
  expect(actions).toContainEqual({ action: { type: "refresh_all_quotas" }, payload: null });
});

test("connections route Free accounts like other pool members", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, accountCount: 3, freeAccountHealthy: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  const freeAccount = page.locator(".account-card").filter({ hasText: "Backup account" });
  await expect(freeAccount.locator(".relay-status-icon")).toHaveAttribute("aria-label", "In rotation");
  await expect(freeAccount).toContainText("95%");
  await expect(freeAccount).toContainText("30 days");
});

test("page navigation resets the shared content scroll position", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, accountCount: 6 });
  await page.setViewportSize({ width: 840, height: 560 });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await expect(page.getByRole("heading", { name: "Connections", exact: true })).toBeVisible();
  await page.locator(".relay-content").evaluate((element) => { element.scrollTop = element.scrollHeight; });
  await expect.poll(() => page.locator(".relay-content").evaluate((element) => element.scrollTop)).toBeGreaterThan(0);
  await page.getByRole("button", { name: "Usage", exact: true }).click();
  await expect(page.locator(".relay-content")).toHaveJSProperty("scrollTop", 0);
  await expect(page.getByRole("heading", { name: "Usage", exact: true })).toBeInViewport();
});

test("connection tabs stay visible while the account list is scrolled", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, accountCount: 6 });
  await page.setViewportSize({ width: 840, height: 560 });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();

  const content = page.locator(".relay-content");
  const tabs = page.locator('.relay-page[data-view="accounts"] .relay-workspace-header .relay-tabs');
  await expect(tabs).toBeVisible();
  await content.evaluate((element) => { element.scrollTop = 120; });
  await expect.poll(() => content.evaluate((element) => element.scrollTop)).toBeGreaterThan(0);
  await expect(tabs.locator("button.active")).toBeVisible();
  const tabGeometry = await tabs.evaluate((tabsElement) => {
    const rect = tabsElement.getBoundingClientRect();
    const button = tabsElement.querySelector("button.active");
    const buttonRect = button?.getBoundingClientRect();
    const contentRect = tabsElement.closest(".relay-content")?.getBoundingClientRect();
    return { rect: { top: rect.top, bottom: rect.bottom, height: rect.height }, button: buttonRect ? { top: buttonRect.top, bottom: buttonRect.bottom, height: buttonRect.height } : null, content: contentRect ? { top: contentRect.top, bottom: contentRect.bottom } : null };
  });
  const tabButtonGeometry = await tabs.locator("button.active").evaluate((button) => {
    const rect = button.getBoundingClientRect();
    return { top: rect.top, bottom: rect.bottom, height: rect.height };
  });
  expect(tabGeometry.content && tabButtonGeometry.top >= tabGeometry.content.top - 2 && tabButtonGeometry.bottom <= tabGeometry.content.bottom + 1 && tabButtonGeometry.height >= 34).toBe(true);
});

test("invalid OAuth grants keep the account and explain the required action", async ({ page }) => {
  await installTauriMock(page, {
    mode: "local",
    locale: "en",
    populated: true,
    accountAuthReason: "invalid_grant",
    quotaAvailable: true,
  });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();

  await expect(page.locator(".account-card")).toHaveCount(1);
  await expect(page.locator(".account-card")).toContainText("Personal Plus");
  await expect(page.locator(".account-status-button")).toHaveAttribute("aria-label", "Signed out or account changed");
  await expect(page.locator(".account-card")).not.toContainText("auth_invalid_grant");

  await page.getByRole("button", { name: "Pool", exact: true }).click();
  const member = page.locator('[data-member-label="Personal Plus"]');
  await expect(member.locator('.pool-member-kind-icon[data-status="error"]')).toHaveAttribute("aria-label", /Signed out or account changed$/);
  await expect(member.locator('.relay-status-icon[aria-label="In rotation"]')).toHaveCount(0);
});

test("source and automation rows keep rare actions in consistent menus", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, codexBindings: false });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();

  await page.getByRole("tab", { name: "Sources" }).click();
  let actions = page.locator(".relay-table .row-actions");
  expect(await actions.locator(":scope > *").evaluateAll((items) => items.map((item) => item.tagName === "DETAILS" ? item.querySelector("summary")?.getAttribute("aria-label") : item.getAttribute("aria-label")))).toEqual(["Actions", "Edit", "Launch"]);
  await actions.locator("summary").click();
  expect(await page.getByRole("menuitem").allTextContents()).toEqual(["Refresh API data", "Remove from pool", "Disable", "Delete"]);
  await page.keyboard.press("Escape");

  await page.getByRole("tab", { name: "Automations" }).click();
  actions = page.locator(".automation-list .row-actions");
  expect(await actions.locator(":scope > *").evaluateAll((items) => items.map((item) => item.tagName === "DETAILS" ? item.querySelector("summary")?.getAttribute("aria-label") : item.getAttribute("aria-label")))).toEqual(["Edit", "Actions"]);
  await actions.locator("summary").click();
  await expect(page.getByRole("menuitem")).toHaveText("Delete");
});

test("source toolbar refresh discovers API data instead of only reloading the snapshot", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, sourceCount: 2 });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await page.getByRole("tab", { name: "Sources" }).click();

  await page.getByRole("button", { name: "Refresh API data", exact: true }).click();
  await expect(page.getByRole("status")).toHaveText("Updated: 2 · Errors: 0 · Skipped: 0");
  await expect.poll(() => page.evaluate(() => (
    window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: { sourceId?: string } }> }
  ).__TAURI_TEST_INVOKES__.filter((call) => call.command === "test_local_source"))).toEqual([
    { command: "test_local_source", args: { sourceId: "source_synthetic" } },
    { command: "test_local_source", args: { sourceId: "source_synthetic_2" } },
  ]);
});

test("empty connection views keep the page header as the single action area", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: false });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await page.getByRole("tab", { name: "Sources" }).click();
  await expect(page.getByRole("button", { name: "Add source", exact: true })).toHaveCount(1);
  await expect(page.getByPlaceholder("Search")).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Refresh", exact: true })).toHaveCount(0);

  await page.getByRole("tab", { name: "Automations" }).click();
  await expect(page.getByRole("button", { name: "Add automation", exact: true })).toHaveCount(1);
  await expect(page.getByPlaceholder("Search")).toHaveCount(0);
});

test("ChatGPT client setup combines account selection, fixed reserve, and forced switching", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, accountCount: 2, gatewayRunning: true, historyRepairChanges: false });
  await page.goto("/");
  await openGatewayApplication(page);
  const setup = page.locator(".client-setup");
  const accountMenu = setup.getByRole("button", { name: /^Account:/ });
  await expect(accountMenu).toHaveAttribute("data-value", "auto");
  expect(await page.evaluate(() => localStorage.getItem("relay.codexPoolOauthSelection"))).toBe("auto");
  const reserve = setup.getByRole("checkbox", { name: "Keep 1% reserved" });
  await expect(reserve).toBeChecked();
  await reserve.uncheck();
  await expect.poll(() => page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: Record<string, unknown> }> }).__TAURI_TEST_INVOKES__.findLast((item) => item.command === "update_chatgpt_interface_quota_reserve")?.args)).toEqual({ input: { reserveBasisPoints: 0 } });
  await reserve.check();
  await expect.poll(() => page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: Record<string, unknown> }> }).__TAURI_TEST_INVOKES__.findLast((item) => item.command === "update_chatgpt_interface_quota_reserve")?.args)).toEqual({ input: { reserveBasisPoints: 100 } });
  await expect(setup).not.toContainText("Generated configuration");
  const selectionMatchesTheme = await setup.evaluate((element) => {
    const probe = document.createElement("span");
    probe.style.backgroundColor = "var(--relay-accent-soft)";
    element.appendChild(probe);
    const expected = getComputedStyle(probe).backgroundColor;
    probe.remove();
    return getComputedStyle(element, "::selection").backgroundColor === expected;
  });
  expect(selectionMatchesTheme).toBe(true);

  await chooseOption(page, setup, "Account", "account_synthetic");
  await expect(accountMenu).toHaveAttribute("data-value", "account_synthetic");

  await chooseOption(page, setup, "Account", "none");
  await expect(setup.getByRole("checkbox", { name: "Keep 1% reserved" })).toHaveCount(0);
  expect(await page.evaluate(() => localStorage.getItem("relay.codexPoolOauthSelection"))).toBe("none");
  await setup.getByRole("button", { name: "Switch", exact: true }).click();
  await expect.poll(() => page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string }> }).__TAURI_TEST_INVOKES__.filter((item) => item.command === "attach_codex_to_local_gateway").length)).toBe(1);
  let call = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: Record<string, unknown> }> }).__TAURI_TEST_INVOKES__.findLast((item) => item.command === "attach_codex_to_local_gateway"));
  expect(call?.args).toEqual({ boundOauthAccountId: null, disableOauthBinding: true });

  await chooseOption(page, page, "Account", "account_synthetic_2");
  expect(await page.evaluate(() => localStorage.getItem("relay.codexPoolOauthSelection"))).toBe("account_synthetic_2");
  await setup.getByRole("button", { name: "Switch", exact: true }).click();
  await expect.poll(() => page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string }> }).__TAURI_TEST_INVOKES__.filter((item) => item.command === "attach_codex_to_local_gateway").length)).toBe(2);
  call = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: Record<string, unknown> }> }).__TAURI_TEST_INVOKES__.findLast((item) => item.command === "attach_codex_to_local_gateway"));
  expect(call?.args).toEqual({ boundOauthAccountId: "account_synthetic_2" });
});

test("ChatGPT pool identity migrates the previous stored account selection", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, accountCount: 2 });
  await page.addInitScript(() => localStorage.setItem("relay.codexPoolOauthAccountId", "account_synthetic_2"));
  await page.goto("/");
  await openGatewayApplication(page);
  await expect(page.getByRole("button", { name: /^Account:/ })).toHaveAttribute("data-value", "account_synthetic_2");
  expect(await page.evaluate(() => ({ current: localStorage.getItem("relay.codexPoolOauthSelection"), legacy: localStorage.getItem("relay.codexPoolOauthAccountId") }))).toEqual({ current: "account_synthetic_2", legacy: null });
});

test("ChatGPT account picker includes quota-wait and Free pool accounts", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, accountCount: 3, quotaAvailable: false });
  await page.goto("/");
  await openGatewayApplication(page);
  const setup = page.locator(".client-setup");
  await setup.getByRole("button", { name: /^Account:/ }).click();
  await expect(page.locator('[role="option"][data-value="account_synthetic"]')).toContainText("Personal Plus");
  await expect(page.locator('[role="option"][data-value="account_synthetic_2"]')).toContainText("Business Workspace");
  await expect(page.locator('[role="option"][data-value="account_synthetic_3"]')).toContainText("Backup account");
  await page.locator('[role="option"][data-value="account_synthetic"]').click();
  await setup.getByRole("button", { name: "Switch", exact: true }).click();

  const call = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: Record<string, unknown> }> }).__TAURI_TEST_INVOKES__.findLast((item) => item.command === "attach_codex_to_local_gateway"));
  expect(call?.args).toEqual({ boundOauthAccountId: "account_synthetic" });
});
