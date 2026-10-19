import { expect, test, type Page } from "../../bun-playwright";
import { installTauriMock } from "../tauri-mock";

const applications = [
  { id: "chatgpt", name: "ChatGPT", connect: "attach_codex_to_local_gateway", launch: "launch_managed_codex_profile", feedback: "Client launched." },
  { id: "claude-desktop", name: "Claude Desktop", connect: "connect_claude_desktop", launch: "launch_claude_desktop", feedback: "Client launched." },
  { id: "opencode", name: "OpenCode", connect: "connect_opencode_to_local_gateway", launch: "restart_opencode_app", feedback: "Client launched." },
] as const;

const launchPreferenceLabel = "Launch applications after connecting";

async function commands(page: Page) {
  return page.evaluate(() => (window as unknown as {
    __TAURI_TEST_INVOKES__: Array<{ command: string }>;
  }).__TAURI_TEST_INVOKES__.map((call) => call.command));
}

async function openPoolPicker(page: Page) {
  await page.getByRole("button", { name: "Pool", exact: true }).click();
  await page.getByRole("button", { name: "Connect", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "Connect an application" });
  await expect(dialog).toBeVisible();
  await expect(dialog.getByRole("checkbox")).toHaveCount(0);
  return dialog;
}

for (const application of applications) {
  test(`pool connects and launches ${application.name} by default`, async ({ page }) => {
    await installTauriMock(page, { mode: "local", locale: "en", populated: true, gatewayRunning: true });
    await page.goto("/");
    const picker = await openPoolPicker(page);
    await picker.getByRole("button", { name: application.name, exact: true }).click();
    await expect(picker).toHaveCount(0);
    await expect(page.locator(".global-feedback.success")).toContainText(application.feedback);

    const calls = await commands(page);
    expect(calls.filter((command) => command === application.connect || command === application.launch))
      .toEqual([application.connect, application.launch]);
    expect(calls.slice(calls.indexOf(application.connect) + 1, calls.indexOf(application.launch)))
      .not.toContain("get_local_runtime_state");
    expect(calls).not.toContain("start_local_gateway");
  });

  test(`Settings persist the launch opt-out and re-enable it for ${application.name}`, async ({ page }) => {
    await installTauriMock(page, { mode: "local", locale: "en", populated: true, gatewayRunning: true });
    await page.goto("/");
    await page.getByRole("button", { name: "Settings", exact: true }).click();
    const toggle = page.getByRole("checkbox", { name: launchPreferenceLabel, exact: true });
    await expect(toggle).toBeChecked();
    await toggle.uncheck();
    await expect.poll(() => page.evaluate(() => localStorage.getItem("relay.launchApplicationAfterConnect"))).toBe("0");
    await page.reload();
    await page.getByRole("button", { name: "Settings", exact: true }).click();
    await expect(toggle).not.toBeChecked();

    const picker = await openPoolPicker(page);
    await picker.getByRole("button", { name: application.name, exact: true }).click();
    await expect(page.locator(".global-feedback.success")).toBeVisible();
    const calls = await commands(page);
    expect(calls).toContain(application.connect);
    expect(calls).not.toContain(application.launch);

    await page.getByRole("button", { name: "Settings", exact: true }).click();
    await toggle.check();
    await expect.poll(() => page.evaluate(() => localStorage.getItem("relay.launchApplicationAfterConnect"))).toBe("1");
    await openPoolPicker(page);
    await picker.getByRole("button", { name: application.name, exact: true }).click();
    await expect(page.locator(".global-feedback.success")).toContainText(application.feedback);
    expect((await commands(page)).filter((command) => command === application.launch)).toHaveLength(1);
  });

  test(`failed ${application.name} connection never launches the application`, async ({ page }) => {
    await installTauriMock(page, { mode: "local", locale: "en", populated: true, gatewayRunning: true });
    await page.goto("/");
    await page.evaluate((connectionCommand) => {
      const internals = (window as unknown as {
        __TAURI_INTERNALS__: { invoke: (command: string, args?: unknown, options?: unknown) => Promise<unknown> };
      }).__TAURI_INTERNALS__;
      const invoke = internals.invoke.bind(internals);
      internals.invoke = (command, args, options) => command === connectionCommand
        ? Promise.reject({ code: "profile_restore_blocked", message: "Synthetic connection failure" })
        : invoke(command, args, options);
    }, application.connect);
    const picker = await openPoolPicker(page);
    await picker.getByRole("button", { name: application.name, exact: true }).click();
    await expect(page.locator(".global-feedback.error")).toBeVisible();
    expect(await commands(page)).not.toContain(application.launch);
  });

  test(`launch opt-out still applies to ${application.name} if preference storage becomes unavailable`, async ({ page }) => {
    await installTauriMock(page, { mode: "local", locale: "en", populated: true, gatewayRunning: true });
    await page.goto("/");
    await page.evaluate(() => {
      const setItem = Storage.prototype.setItem;
      Storage.prototype.setItem = function (key: string, value: string) {
        if (key === "relay.launchApplicationAfterConnect") throw new Error("Synthetic unavailable preference storage");
        setItem.call(this, key, value);
      };
    });
    await page.getByRole("button", { name: "Settings", exact: true }).click();
    await page.getByRole("checkbox", { name: launchPreferenceLabel, exact: true }).uncheck();
    const picker = await openPoolPicker(page);
    await picker.getByRole("button", { name: application.name, exact: true }).click();
    await expect(page.locator(".global-feedback.success")).toBeVisible();
    expect(await commands(page)).not.toContain(application.launch);
    await page.getByRole("button", { name: "Settings", exact: true }).click();
    await expect(page.getByRole("checkbox", { name: launchPreferenceLabel, exact: true })).not.toBeChecked();
  });

  test(`pool keeps ${application.name} connection busy until launch finishes`, async ({ page }) => {
    await installTauriMock(page, { mode: "local", locale: "en", populated: true, gatewayRunning: true });
    await page.goto("/");
    await page.evaluate((launchCommand) => {
      const testWindow = window as unknown as {
        __RESUME_APPLICATION_LAUNCH__?: () => void;
        __TAURI_INTERNALS__: { invoke: (command: string, args?: unknown, options?: unknown) => Promise<unknown> };
      };
      const invoke = testWindow.__TAURI_INTERNALS__.invoke.bind(testWindow.__TAURI_INTERNALS__);
      testWindow.__TAURI_INTERNALS__.invoke = async (command, args, options) => {
        if (command === launchCommand) await new Promise<void>((resolve) => { testWindow.__RESUME_APPLICATION_LAUNCH__ = resolve; });
        return invoke(command, args, options);
      };
    }, application.launch);
    const picker = await openPoolPicker(page);
    await picker.getByRole("button", { name: application.name, exact: true }).click();
    await expect.poll(() => page.evaluate(() => Boolean((window as unknown as {
      __RESUME_APPLICATION_LAUNCH__?: () => void;
    }).__RESUME_APPLICATION_LAUNCH__))).toBe(true);
    const connect = page.getByRole("button", { name: "Connect", exact: true });
    await expect(connect).toBeDisabled();
    await expect(connect.locator(".spin")).toHaveCount(1);
    await expect(page.locator(".global-feedback.success")).toHaveCount(0);
    await page.evaluate(() => (window as unknown as { __RESUME_APPLICATION_LAUNCH__: () => void }).__RESUME_APPLICATION_LAUNCH__());
    await expect(page.locator(".global-feedback.success")).toContainText(application.feedback);
    await expect(connect).toBeEnabled();
    expect((await commands(page)).filter((command) => command === application.launch)).toHaveLength(1);
  });

  for (const gatewayRunning of [false, true]) {
    test(`Overview opens ${application.name} without connecting with API ${gatewayRunning ? "running" : "stopped"}`, async ({ page }) => {
      await installTauriMock(page, {
        mode: "local", locale: "en", populated: gatewayRunning, gatewayRunning,
        claudeDesktopConfigured: false, profileSwitchError: true,
      });
      await page.addInitScript(() => localStorage.setItem("relay.launchApplicationAfterConnect", "0"));
      await page.goto("/");
      const launch = page.getByRole("button", { name: "Launch application", exact: true });
      await expect(launch).toBeEnabled();
      await launch.click();
      const picker = page.getByRole("dialog", { name: "Which application do you want to launch?" });
      await expect(picker.getByRole("checkbox")).toHaveCount(0);
      await picker.getByRole("button", { name: application.name, exact: true }).click();
      await expect(picker).toHaveCount(0);
      await expect(page.locator(".global-feedback.success")).toContainText(application.feedback);

      const calls = await page.evaluate(() => (window as unknown as {
        __TAURI_TEST_INVOKES__: Array<{ command: string; args: Record<string, unknown> }>;
      }).__TAURI_TEST_INVOKES__);
      expect(calls.filter((call) => call.command === "launch_application").map((call) => call.args))
        .toEqual([{ application: application.id }]);
      const invoked = calls.map((call) => call.command);
      for (const managed of applications) {
        expect(invoked).not.toContain(managed.connect);
        expect(invoked).not.toContain(managed.launch);
      }
      expect(invoked).not.toContain("stop_managed_codex_profile");
      expect(invoked).not.toContain("start_local_gateway");
      expect(invoked).not.toContain("stop_local_gateway");
      expect(invoked).not.toContain("restart_local_gateway");
      await expect(page.getByRole("button", { name: gatewayRunning ? "Stop API" : "Start API", exact: true })).toBeVisible();
    });
  }

  test(`Overview opens ${application.name} after an external profile change without consulting Relay profiles`, async ({ page }) => {
    await installTauriMock(page, { mode: "local", locale: "en", populated: true, gatewayRunning: false });
    await page.goto("/");
    await expect(page.getByRole("button", { name: "Start API", exact: true })).toBeVisible();
    await page.evaluate((managedCommands) => {
      const state = window as unknown as {
        __PROFILE_CHECKS__: string[];
        __TAURI_INTERNALS__: { invoke: (command: string, args?: unknown, options?: unknown) => Promise<unknown> };
      };
      state.__PROFILE_CHECKS__ = [];
      const invoke = state.__TAURI_INTERNALS__.invoke.bind(state.__TAURI_INTERNALS__);
      state.__TAURI_INTERNALS__.invoke = (command, args, options) => {
        if (managedCommands.includes(command)) {
          state.__PROFILE_CHECKS__.push(command);
          return Promise.reject({ code: "profile_restore_blocked", message: "Synthetic external profile change" });
        }
        return invoke(command, args, options);
      };
    }, [
      ...applications.flatMap((managed) => [managed.connect, managed.launch]),
      "get_local_runtime_state", "list_codex_account_bindings", "get_claude_desktop_status", "get_opencode_config_status",
    ]);
    const launch = page.getByRole("button", { name: "Launch application", exact: true });
    await launch.click();
    await page.getByRole("dialog").getByRole("button", { name: application.name, exact: true }).click();
    await expect(page.locator(".global-feedback.success")).toContainText(application.feedback);
    await expect(launch).toBeEnabled();
    expect(await page.evaluate(() => (window as unknown as { __PROFILE_CHECKS__: string[] }).__PROFILE_CHECKS__)).toEqual([]);
    expect((await commands(page)).filter((command) => command === "launch_application")).toHaveLength(1);
  });

  test(`Overview reports ${application.name} launch errors without connecting as a fallback`, async ({ page }) => {
    await installTauriMock(page, { mode: "local", locale: "en", populated: false, gatewayRunning: false });
    await page.goto("/");
    await page.evaluate(() => {
      const internals = (window as unknown as {
        __TAURI_INTERNALS__: { invoke: (command: string, args?: unknown, options?: unknown) => Promise<unknown> };
      }).__TAURI_INTERNALS__;
      const invoke = internals.invoke.bind(internals);
      internals.invoke = (command, args, options) => command === "launch_application"
        ? Promise.reject({ code: "io_error", message: "Synthetic application launch failure" })
        : invoke(command, args, options);
    });
    await page.getByRole("button", { name: "Launch application", exact: true }).click();
    await page.getByRole("dialog").getByRole("button", { name: application.name, exact: true }).click();
    await expect(page.locator(".global-feedback.error")).toBeVisible();
    await expect(page.locator(".global-feedback.success")).toHaveCount(0);
    await expect(page.getByRole("button", { name: "Launch application", exact: true })).toBeEnabled();
    const calls = await commands(page);
    expect(calls).not.toContain(application.connect);
    expect(calls).not.toContain(application.launch);
    expect(calls).not.toContain("start_local_gateway");
  });
}

test("picker supports keyboard selection, cancellation, and focus return", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, gatewayRunning: true });
  await page.goto("/");
  const picker = await openPoolPicker(page);
  await page.keyboard.press("Tab");
  await expect(picker.getByRole("button", { name: "Close", exact: true })).toBeFocused();
  await page.keyboard.press("Tab");
  await expect(picker.getByRole("button", { name: "ChatGPT", exact: true })).toBeFocused();
  await page.keyboard.press("Tab");
  await expect(picker.getByRole("button", { name: "Claude Desktop", exact: true })).toBeFocused();
  await page.keyboard.press("Tab");
  await expect(picker.getByRole("button", { name: "OpenCode", exact: true })).toBeFocused();
  await page.keyboard.press("Tab");
  await expect(picker.getByRole("button", { name: "Close", exact: true })).toBeFocused();
  // Move off Close so its tooltip does not consume Escape before the dialog.
  await page.keyboard.press("Tab");
  await expect(picker.getByRole("button", { name: "ChatGPT", exact: true })).toBeFocused();
  await page.keyboard.press("Escape");
  await expect(picker).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Connect", exact: true })).toBeFocused();
  expect((await commands(page)).filter((command) => applications.some((application) => application.connect === command))).toEqual([]);

  await page.keyboard.press("Enter");
  await expect(picker).toBeVisible();
  await page.keyboard.press("Shift+Tab");
  await expect(picker.getByRole("button", { name: "OpenCode", exact: true })).toBeFocused();
  await page.keyboard.press("Shift+Tab");
  await expect(picker.getByRole("button", { name: "Claude Desktop", exact: true })).toBeFocused();
  await page.keyboard.press("Enter");
  await expect(page.locator(".global-feedback.success")).toContainText("Client launched.");
  expect(await commands(page)).toContain("launch_claude_desktop");
});

test("source picker explains unavailable ChatGPT and leaves OpenCode usable", async ({ page }) => {
  await installTauriMock(page, {
    mode: "local", locale: "en", populated: true,
    sourceProtocolBindings: [{ wireApi: "chat_completions", adapter: "native", reasoningMode: "disabled", modelIds: ["gpt-5.4"] }],
  });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await page.getByRole("tab", { name: "Sources", exact: true }).click();
  const sourceRow = page.locator(".source-card").filter({ hasText: "Example compatible API" });
  await sourceRow.getByRole("button", { name: "Launch", exact: true }).click();
  const picker = page.getByRole("dialog", { name: "Where do you want to launch this source?" });
  const chatgpt = picker.getByRole("button", { name: "ChatGPT", exact: true });
  await expect(chatgpt).toBeDisabled();
  await expect(chatgpt).toHaveAccessibleDescription("ChatGPT direct launch requires a native Responses endpoint.");
  await expect(picker.getByText("ChatGPT direct launch requires a native Responses endpoint.")).toBeVisible();
  await picker.getByRole("button", { name: "OpenCode", exact: true }).click();
  await expect(page.locator(".global-feedback.success")).toContainText("Client launched.");
  expect(await commands(page)).toContain("launch_opencode_source");
  expect(await commands(page)).not.toContain("launch_codex_source");
});
