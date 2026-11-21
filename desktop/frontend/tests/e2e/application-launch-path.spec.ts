import { expect, test, type Page } from "../bun-playwright";
import { installTauriMock } from "./tauri-mock";
import { chooseOption } from "./operations/helpers";

const applications = [
  { id: "chatgpt", name: "ChatGPT", path: "D:\\Apps\\ChatGPT\\ChatGPT.exe", detected: "C:\\Program Files\\ChatGPT\\ChatGPT.exe" },
  { id: "claude-desktop", name: "Claude Desktop", path: "D:\\Apps\\Claude\\Claude.exe", detected: "C:\\Users\\Test\\AppData\\Local\\Claude\\Claude.exe" },
  { id: "opencode", name: "OpenCode", path: "D:\\Apps\\OpenCode\\OpenCode.exe", detected: "C:\\Users\\Test\\AppData\\Local\\Programs\\OpenCode\\OpenCode.exe" },
] as const;

type InvokeCall = { command: string; args: Record<string, unknown> };

test("only Claude offers an application connection", async ({ page }) => {
  await installTauriMock(page, { locale: "en", mode: "local", populated: true });
  await page.goto("/");
  await openApplications(page);
  for (const application of applications) {
    await selectApplication(page, application);
    await expect(page.locator("[data-application-connection]")).toHaveCount(application.id === "claude-desktop" ? 1 : 0);
  }
  const reads = (await calls(page)).filter(({ command }) => command === "get_application_connection");
  expect(reads.length).toBeGreaterThan(0);
  expect(reads.every(({ args }) => args.application === "claude-desktop")).toBe(true);
});

test("Claude connection is saved without changing sign-in or launching", async ({ page }) => {
  await installTauriMock(page, { locale: "en", mode: "local", populated: true, gatewayRunning: false });
  await page.goto("/");
  await page.evaluate(async () => {
    const tauri = (window as unknown as { __TAURI_INTERNALS__: { invoke: (name: string, args: unknown) => Promise<unknown> } }).__TAURI_INTERNALS__;
    await tauri.invoke("rename_local_stored_proxy", { proxyId: "proxy_synthetic_1", label: "Work connection" });
  });
  await openApplications(page);
  const form = await selectApplication(page, applications[1]);
  const connection = page.locator('[data-application-connection="claude-desktop"]');
  expect(await connection.evaluate((element) => element.closest(".application-launch-settings") === null)).toBe(true);
  await expect(form.locator("[data-application-connection]")).toHaveCount(0);
  await expect(connection.getByRole("button", { name: "Connection: As usual" })).toBeEnabled();
  await chooseOption(page, connection, "Connection", "proxy_synthetic_1");
  await expect(connection.locator(".application-connection-status")).not.toHaveClass(/is-default|is-pending/);
  await expect(connection.locator("small, p, [role=alert]")).toHaveCount(0);
  await selectApplication(page, applications[0]);
  await selectApplication(page, applications[1]);
  await expect(connection.getByRole("button", { name: "Connection: Work connection" })).toBeEnabled();
  await chooseOption(page, connection, "Connection", "");
  await expect(connection.getByRole("button", { name: "Connection: As usual" })).toBeEnabled();
  const invoked = await calls(page);
  expect(invoked.filter((call) => call.command === "set_application_connection").map((call) => call.args)).toEqual([
    { application: "claude-desktop", proxyId: "proxy_synthetic_1", expectedRevision: "0" },
    { application: "claude-desktop", proxyId: null, expectedRevision: "1" },
  ]);
  expect(invoked.filter((call) => /^(launch_|stop_|restart_|attach_|connect_)/.test(call.command))).toEqual([]);
});

test("a removed application connection is shown as unavailable, not as the default", async ({ page }) => {
  await installTauriMock(page, { locale: "en", mode: "local", populated: true });
  await page.goto("/");
  await overrideCommand(page, { command: "get_application_connection", result: {
    proxyId: "proxy_removed", nativeSupported: true, status: "saved", revision: "0",
  } });
  const connection = await openClaudeConnection(page);
  await expect(connection.getByRole("button", { name: "Connection: Connection unavailable" })).toBeEnabled();
  await expect(connection.locator(".application-connection-status")).toHaveClass(/is-error/);
  await connection.locator(".application-connection-status").hover();
  await expect(page.getByRole("tooltip")).toHaveText("Connection unavailable");
});

test("default Claude connection does not depend on reading the proxy pool", async ({ page }) => {
  await installTauriMock(page, { locale: "en", mode: "local", populated: true, gatewayRunning: false });
  await page.goto("/");
  await page.evaluate(() => {
    const tauri = (window as unknown as {
      __TAURI_INTERNALS__: { invoke: (name: string, args: unknown, options?: unknown) => Promise<unknown> };
    }).__TAURI_INTERNALS__;
    const invoke = tauri.invoke.bind(tauri);
    tauri.invoke = (name, args, options) => name === "get_local_proxy_pool"
      ? Promise.reject({ code: "synthetic_proxy_pool_unavailable" }) : invoke(name, args, options);
  });
  const connection = await openClaudeConnection(page);
  const icon = connection.locator(".application-connection-status");
  await expect(icon).toHaveClass(/is-default/);
  await expect(icon).toHaveAttribute("aria-label", "Connection status: As usual");
  await expect(connection.locator("p, small, [role=alert]")).toHaveCount(0);
  expect((await calls(page)).filter((call) => /^(launch_|stop_|restart_|attach_|connect_)/.test(call.command))).toEqual([]);
});

test("a failed save leaves the prior choice intact and shows the error only in the icon tooltip", async ({ page }) => {
  await installTauriMock(page, { locale: "ru", mode: "local", populated: true });
  await page.goto("/");
  await page.evaluate(() => {
    const tauri = (window as unknown as {
      __TAURI_INTERNALS__: { invoke: (name: string, args: unknown, options?: unknown) => Promise<unknown> };
    }).__TAURI_INTERNALS__;
    const invoke = tauri.invoke.bind(tauri);
    tauri.invoke = (name, args, options) => name === "set_application_connection"
      ? Promise.reject({ code: "connection_changed" })
      : invoke(name, args, options);
  });
  const connection = await openClaudeConnection(page, true);
  await chooseOption(page, connection, "Подключение", "proxy_synthetic_1");
  await expect(connection.getByRole("alert")).toHaveCount(0);
  const icon = connection.locator(".application-connection-status");
  await expect(icon).toHaveClass(/is-error/);
  await icon.hover();
  await expect(page.getByRole("tooltip")).toHaveText("Настройки приложения изменились. Нажмите значок, чтобы обновить.");
  await icon.blur();
  await icon.focus();
  await expect(page.getByRole("tooltip")).toBeVisible();
  await expect(connection.getByRole("button", { name: "Подключение: Как обычно" })).toBeEnabled();
});

test("upstream protocol is independent per application and does not restart clients", async ({ page }) => {
  await installTauriMock(page, { locale: "en", mode: "local", populated: true });
  await page.goto("/");
  await openApplications(page);
  const protocols = ["messages", "responses", "gemini"];
  for (const [index, application] of applications.entries()) {
    await selectApplication(page, application);
    const control = page.locator(`[data-application-protocol="${application.id}"]`);
    await expect(control.getByRole("button", { name: /^Request format:/ })).toBeEnabled();
    await chooseOption(page, control, "Request format", protocols[index]);
  }
  await selectApplication(page, applications[0]);
  await expect(page.getByRole("button", { name: "Request format: Anthropic Messages" })).toBeVisible();
  const invoked = await calls(page);
  expect(invoked.filter((call) => call.command === "set_application_upstream_protocol").map((call) => call.args)).toEqual([
    { application: "chatgpt", policy: "messages", expected: "auto" },
    { application: "claude-desktop", policy: "responses", expected: "auto" },
    { application: "opencode", policy: "gemini", expected: "auto" },
  ]);
  expect(invoked.filter((call) => /^(launch_|stop_|restart_|attach_|connect_)/.test(call.command))).toEqual([]);
});

test("proxy reachability never claims an application route is active", async ({ page }) => {
  await installTauriMock(page, { locale: "en", mode: "local", populated: true, gatewayRunning: false });
  await page.goto("/");
  await page.evaluate(async () => {
    const tauri = (window as unknown as {
      __TAURI_INTERNALS__: { invoke: (name: string, args?: Record<string, unknown>, options?: unknown) => Promise<unknown> };
    }).__TAURI_INTERNALS__;
    const invoke = tauri.invoke.bind(tauri);
    const pool = await invoke("get_local_proxy_pool") as { entries: { lastCheck?: unknown }[] };
    pool.entries[0].lastCheck = null;
    pool.entries[1].lastCheck = { ip: "192.0.2.1", checkedAtMs: Date.now(), errorCode: null };
    pool.entries[2].lastCheck = { ip: null, checkedAtMs: Date.now(), errorCode: "proxy_check_timeout" };
    tauri.invoke = (name, args, options) => name === "get_local_proxy_pool" ? Promise.resolve(pool)
      : invoke(name, args, options);
  });
  const connection = await openClaudeConnection(page);
  const states = ["unverified", "unverified", "error"];
  for (const [index, status] of states.entries()) {
    await overrideCommand(page, { command: "get_application_connection", result: {
      proxyId: `proxy_synthetic_${index + 1}`, nativeSupported: true, status: "saved", revision: String(index),
    } });
    const icon = connection.locator(".application-connection-status");
    await icon.click();
    await expect(icon).toHaveClass(new RegExp(`is-${status}`));
    await expect(icon).not.toHaveClass(/is-ready/);
    await expect(connection.locator("p, small, [role=alert]")).toHaveCount(0);
    await icon.hover();
    await expect(page.getByRole("tooltip")).toBeVisible();
    if (index === 1) {
      await expect(page.getByRole("tooltip")).toHaveText("Saved in the application. Restart it to apply.");
    }
    expect(await icon.evaluate((element) => getComputedStyle(element).color)).not.toBe("rgba(0, 0, 0, 0)");
    await page.screenshot({ path: `output/playwright/claude-connection-${index}-${status}.png` });
    await page.mouse.move(0, 0);
  }
});

test("connection loading and failed reads stay in the icon and retry without launching", async ({ page }) => {
  await installTauriMock(page, { locale: "en", mode: "local", populated: true, gatewayRunning: false });
  await page.goto("/");
  await overrideCommand(page, { command: "get_application_connection", failTimes: 1 });
  const connection = await openClaudeConnection(page);
  const icon = connection.locator(".application-connection-status");
  await expect(icon).toHaveClass(/is-error/);
  await expect(connection.locator(".relay-option-trigger")).toBeDisabled();
  await expect(connection.locator("p, small, [role=alert]")).toHaveCount(0);
  await page.keyboard.press("Tab");
  await icon.focus();
  await expect(page.getByRole("tooltip")).toBeVisible();
  await icon.click();
  await expect(connection.locator(".relay-option-trigger")).toBeEnabled();
  await expect(icon).toHaveClass(/is-default/);
  expect((await calls(page)).filter((call) => /^(launch_|stop_|restart_|attach_|connect_)/.test(call.command))).toEqual([]);
});

test("refresh reads native settings without starting a proxy probe or claiming application routing", async ({ page }) => {
  await installTauriMock(page, { locale: "en", mode: "local", populated: true, gatewayRunning: false });
  await page.goto("/");
  await page.evaluate(async () => {
    const host = window as unknown as {
      __TAURI_INTERNALS__: { invoke: (name: string, args?: unknown, options?: unknown) => Promise<unknown> };
      finishConnectionRead?: () => void;
    };
    const invoke = host.__TAURI_INTERNALS__.invoke.bind(host.__TAURI_INTERNALS__);
    const pool = await invoke("get_local_proxy_pool") as { entries: { lastCheck?: unknown }[] };
    pool.entries[0].lastCheck = null;
    const setting = { proxyId: "proxy_synthetic_1", nativeSupported: true, status: "saved", revision: "0" };
    let reads = 0;
    host.__TAURI_INTERNALS__.invoke = (name, args, options) => name === "get_local_proxy_pool" ? Promise.resolve(pool)
      : name === "get_application_connection" ? reads++ === 0 ? Promise.resolve(setting) : new Promise((resolve) => {
        host.finishConnectionRead = () => resolve(setting);
      }) : invoke(name, args, options);
  });
  const connection = await openClaudeConnection(page);
  const icon = connection.locator(".application-connection-status");
  await expect(icon).toHaveClass(/is-unverified/);
  await icon.click();
  await expect(icon).toHaveClass(/is-pending/);
  await expect(icon).toHaveAttribute("aria-busy", "true");
  await expect(connection.locator(".relay-option-trigger")).toBeDisabled();
  await page.evaluate(() => (window as unknown as { finishConnectionRead: () => void }).finishConnectionRead());
  await expect(icon).toHaveClass(/is-unverified/);
  await expect(icon).not.toHaveClass(/is-ready/);
  await icon.hover();
  await expect(page.getByRole("tooltip")).toHaveText("Saved in the application. Restart it to apply.");
  await expect(connection.locator(".relay-option-trigger")).toBeEnabled();
  await expect(connection.locator("p, small, [role=alert]")).toHaveCount(0);
  expect((await calls(page)).filter(({ command }) => command === "check_local_stored_proxy")).toEqual([]);
});

test("unsupported Claude configurations disable saving without offering a service", async ({ page }) => {
  await installTauriMock(page, { locale: "en", claudeConnectionSupported: false });
  await page.goto("/");
  const connection = await openClaudeConnection(page);
  await expect(connection.locator(".relay-option-trigger")).toBeDisabled();
  const icon = connection.locator(".application-connection-status");
  await expect(icon).toHaveClass(/is-error/);
  await icon.hover();
  await expect(page.getByRole("tooltip")).toHaveText("This Claude Desktop configuration does not support the selected connection.");
  await expect(connection.locator("p, small, [role=alert]")).toHaveCount(0);
  expect((await calls(page)).filter(({ command }) => command === "set_application_connection")).toEqual([]);
});

test("legacy selections stay visible and can be cleared without a native adapter", async ({ page }) => {
  await installTauriMock(page, { locale: "en" });
  await page.goto("/");
  await overrideCommand(page, { command: "get_application_connection", result: {
    proxyId: "proxy_synthetic_1", nativeSupported: false, status: "legacy", revision: "old",
  } });
  await overrideCommand(page, { command: "set_application_connection", record: true, result: {
    proxyId: null, nativeSupported: false, status: "unsupported", revision: "new",
  } });
  const connection = await openClaudeConnection(page);
  await expect(connection.locator(".application-connection-status")).toHaveClass(/is-error/);
  await chooseOption(page, connection, "Connection", "");
  await expect(connection.getByRole("button", { name: "Connection: As usual" })).toBeDisabled();
  expect((await calls(page)).filter(({ command }) => command === "set_application_connection").at(-1)?.args)
    .toEqual({ application: "claude-desktop", proxyId: null, expectedRevision: "old" });
});

test("upstream protocol only offers backend choices and retains the saved value after failure", async ({ page }) => {
  await installTauriMock(page, { locale: "en", mode: "local", populated: true });
  await page.goto("/");
  await overrideCommand(page, { command: "get_application_upstream_protocol", result: {
    policy: "responses", available: ["auto", "responses"], reconnectRequired: true,
  } });
  await overrideCommand(page, { command: "set_application_upstream_protocol", failTimes: 1 });
  await openApplications(page);
  const control = page.locator('[data-application-protocol="chatgpt"]');
  await expect(control.getByText("Reconnect this application to the pool to apply the selection.")).toBeVisible();
  await control.getByRole("button", { name: "Request format: OpenAI Responses" }).click();
  await expect(page.getByRole("option")).toHaveCount(2);
  await page.getByRole("option", { name: "Automatic", exact: true }).click();
  await expect(control.getByRole("alert")).toContainText("Could not save the request format");
  await expect(control.getByRole("button", { name: "Request format: OpenAI Responses" })).toBeEnabled();
  await overrideCommand(page, { command: "get_application_upstream_protocol", result: {
    policy: "auto", available: ["auto"], reconnectRequired: false,
  } });
  await control.getByRole("button", { name: "Retry", exact: true }).click();
  await expect(control.getByRole("button", { name: "Request format: Automatic" })).toBeEnabled();
  await expect(control.getByRole("alert")).toHaveCount(0);
});

for (const { width, theme } of [390, 1280].flatMap((width) =>
  (["dark", "light"] as const).map((theme) => ({ width, theme })))) {
  test(`upstream protocol follows account with matching outlined selectors at ${width}px in ${theme}`, async ({ page }) => {
    await page.setViewportSize({ width, height: 900 });
    await installTauriMock(page, { locale: "ru", mode: "local", populated: true, theme });
    await page.goto("/");
    await openApplications(page, true);
    const settings = page.locator(".integration-chatgpt-settings");
    const control = page.locator('[data-application-protocol="chatgpt"]');
    const reference = settings.locator(".codex-memory-model-menu .relay-option-trigger").first();
    await expect(reference).toBeEnabled();
    const referenceStyle = await reference.evaluate((element) => {
      const style = getComputedStyle(element);
      return { color: style.borderTopColor, width: style.borderTopWidth, radius: style.borderTopLeftRadius, background: style.backgroundColor };
    });
    expect(referenceStyle.color).not.toBe("rgba(0, 0, 0, 0)");
    expect(referenceStyle.width).toBe("1px");
    expect(await settings.locator("h2").allTextContents()).toEqual([
      "Аккаунт ChatGPT", "Формат запросов", "Фоновые задачи ChatGPT",
      "Извлечение памяти", "Сводки памяти", "WebSocket для ChatGPT",
    ]);
    for (const trigger of [
      settings.locator(".oauth-binding-account-control .relay-option-trigger"),
      control.locator(".relay-option-trigger"),
    ]) {
      await expect(trigger).toHaveCSS("border-top-color", referenceStyle.color);
      await expect(trigger).toHaveCSS("border-top-width", referenceStyle.width);
      await expect(trigger).toHaveCSS("border-top-style", "solid");
      await expect(trigger).toHaveCSS("border-top-left-radius", referenceStyle.radius);
      await expect(trigger).toHaveCSS("background-color", referenceStyle.background);
    }
    await control.scrollIntoViewIfNeeded();
    await expect(control.getByRole("button", { name: /^Формат запросов:/ })).toBeEnabled();
    await expect(control.locator("header p")).toHaveText("Автоматически — Relay сам выбирает формат для поставщика модели. Вручную — использует только выбранный.");
    expect(await control.evaluate((element) => element.scrollWidth <= element.clientWidth)).toBe(true);
    await page.screenshot({ path: `output/playwright/application-protocol-ru-${width}-${theme}.png` });
  });
}

async function calls(page: Page) {
  return page.evaluate(() => (window as unknown as {
    __TAURI_TEST_INVOKES__: InvokeCall[];
  }).__TAURI_TEST_INVOKES__);
}

async function openApplications(page: Page, russian = false) {
  await page.getByRole("button", { name: russian ? "Доступ" : "Access", exact: true }).click();
  await page.getByRole("tab", { name: russian ? "Приложения" : "Applications", exact: true }).click();
}

async function openClaudeConnection(page: Page, russian = false) {
  await openApplications(page, russian);
  await selectApplication(page, applications[1]);
  return page.locator('[data-application-connection="claude-desktop"]');
}

async function selectApplication(page: Page, application: typeof applications[number]) {
  await page.locator(".application-workspace .integration-picker").getByRole("tab", { name: application.name, exact: true }).click();
  const form = page.locator(`.application-launch-path[data-application="${application.id}"]`);
  await expect(form.getByRole("textbox")).toBeEnabled();
  return form;
}

async function overrideCommand(page: Page, override: { command: string; failTimes?: number; result?: unknown; record?: boolean }) {
  await page.evaluate((override) => {
    const internals = (window as unknown as {
      __TAURI_INTERNALS__: { invoke: (command: string, args?: unknown, options?: unknown) => Promise<unknown> };
    }).__TAURI_INTERNALS__;
    const invoke = internals.invoke.bind(internals);
    let remaining = override.failTimes ?? 0;
    internals.invoke = (command, args, options) => {
      if (command !== override.command) return invoke(command, args, options);
      if (override.record) {
        (window as unknown as { __TAURI_TEST_INVOKES__: unknown[] }).__TAURI_TEST_INVOKES__.push({ command, args });
      }
      if (remaining > 0) {
        remaining -= 1;
        return Promise.reject("Synthetic application setting failure");
      }
      return "result" in override ? Promise.resolve(override.result) : invoke(command, args, options);
    };
  }, override);
}

for (const application of applications) {
  test(`${application.name} path editor keeps the folder picker and Save`, async ({ page }) => {
    await installTauriMock(page, { mode: "local", locale: "en", populated: true, gatewayRunning: false });
    await page.goto("/");
    await openApplications(page);
    const form = await selectApplication(page, application);
    expect(await form.evaluate((element) => element.parentElement?.firstElementChild === element)).toBe(true);
    const input = form.getByRole("textbox", { name: "Application path", exact: true });
    const picker = form.getByRole("button", { name: "Browse application", exact: true });
    await expect(input).toHaveValue("");
    await expect(input).toHaveAttribute("placeholder", application.detected);
    await expect(form.getByRole("button")).toHaveCount(2);
    await expect(picker).toHaveText("");
    await expect(picker.locator("svg")).toHaveCount(1);
    await expect(form.locator("p")).toHaveCount(0);
    await expect(form.getByRole("button", { name: "Save", exact: true })).toBeDisabled();
    await expect(form.getByRole("button", { name: "Automatic", exact: true })).toHaveCount(0);

    await input.fill(`  ${application.path}  `);
    await form.getByRole("button", { name: "Save", exact: true }).click();
    await expect(form.getByRole("status")).toHaveText("Saved.");
    await expect(input).toHaveValue(application.path);
    expect((await calls(page)).filter((call) => call.command === "set_application_launch_path").map((call) => call.args))
      .toEqual([{ application: application.id, path: application.path }]);

    await form.getByRole("button", { name: "Browse application", exact: true }).click();
    await expect(input).toHaveValue(application.detected);
    expect((await calls(page)).filter((call) => call.command === "pick_application_launch_path").at(-1)?.args)
      .toEqual({ application: application.id, path: application.path });
    await expect(form.getByRole("button", { name: "Save", exact: true })).toBeEnabled();
    expect((await calls(page)).filter((call) => call.command === "set_application_launch_path")).toHaveLength(1);
    await input.press("Enter");
    await expect(form.getByRole("status")).toHaveText("Saved.");
    expect((await calls(page)).filter((call) => call.command === "set_application_launch_path").at(-1)?.args)
      .toEqual({ application: application.id, path: application.detected });

    await input.fill("");
    await form.getByRole("button", { name: "Save", exact: true }).click();
    await expect(form.getByRole("status")).toHaveText("Saved.");
    await expect(input).toHaveValue("");
    await expect(form.getByRole("button")).toHaveCount(2);
    await expect(form.getByRole("button", { name: "Automatic", exact: true })).toHaveCount(0);
    expect((await calls(page)).filter((call) => call.command === "set_application_launch_path").at(-1)?.args)
      .toEqual({ application: application.id, path: null });
    expect((await calls(page)).filter((call) => /^(launch_|restart_|connect_|attach_|restore_)/.test(call.command))).toEqual([]);
  });

  test(`${application.name} opens the picker at the detected path or unsaved draft`, async ({ page }) => {
    await installTauriMock(page, { mode: "local", locale: "en", populated: true });
    await page.goto("/");
    await openApplications(page);
    const form = await selectApplication(page, application);
    const picker = form.getByRole("button", { name: "Browse application", exact: true });
    await picker.click();
    await expect(form.getByRole("textbox")).toHaveValue(application.detected);
    expect((await calls(page)).filter((call) => call.command === "pick_application_launch_path").at(-1)?.args)
      .toEqual({ application: application.id, path: application.detected });

    await form.getByRole("textbox").fill(`  ${application.path}  `);
    await picker.click();
    await expect(form.getByRole("textbox")).toHaveValue(application.detected);
    expect((await calls(page)).filter((call) => call.command === "pick_application_launch_path").at(-1)?.args)
      .toEqual({ application: application.id, path: application.path });
    expect((await calls(page)).filter((call) => call.command === "set_application_launch_path")).toEqual([]);
  });
}

test("paths remain independent when switching application tabs and Accounts stays separate", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true });
  await page.goto("/");
  await openApplications(page);
  for (const application of applications) {
    const form = await selectApplication(page, application);
    await form.getByRole("textbox").fill(application.path);
    await form.getByRole("button", { name: "Save", exact: true }).click();
    await expect(form.getByRole("status")).toHaveText("Saved.");
  }
  for (const application of applications) {
    const form = await selectApplication(page, application);
    await expect(form.getByRole("textbox")).toHaveValue(application.path);
    if (application.id === "chatgpt") {
      await expect(page.locator(".oauth-binding-settings")).toBeVisible();
      await expect(page.locator(".codex-feature-control")).toHaveCount(2);
    }
    if (application.id === "claude-desktop") {
      const panel = page.getByRole("tabpanel", { name: "Claude Desktop", exact: true });
      await expect(panel).not.toContainText("configLibrary");
      await expect(panel.getByRole("button", { name: /^(Connect|Launch|Disconnect and restore)$/ })).toHaveCount(0);
    }
  }
  expect((await calls(page)).filter((call) => call.command.includes("claude_desktop"))).toEqual([]);
  await page.getByRole("tab", { name: "Accounts", exact: true }).click();
  await expect(page.locator(".application-launch-path")).toHaveCount(0);
});

test("a cancelled native picker preserves an unsaved draft", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true });
  await page.goto("/");
  await overrideCommand(page, { command: "pick_application_launch_path", result: null });
  await openApplications(page);
  const form = await selectApplication(page, applications[1]);
  await form.getByRole("textbox").fill(applications[1].path);
  await form.getByRole("button", { name: "Browse application", exact: true }).click();
  await expect(form.getByRole("textbox")).toHaveValue(applications[1].path);
  await expect(form.getByRole("button", { name: "Save", exact: true })).toBeEnabled();
  await expect(form.getByRole("alert")).toHaveCount(0);
  expect((await calls(page)).filter((call) => call.command === "set_application_launch_path")).toEqual([]);
});

test("launch-path load errors are explicit and retryable", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true });
  await page.goto("/");
  await overrideCommand(page, { command: "get_application_launch_path", failTimes: Number.MAX_SAFE_INTEGER });
  await openApplications(page);
  const form = page.locator('.application-launch-path[data-application="chatgpt"]');
  await expect(form.getByRole("alert")).toContainText("Could not load the launch path.");
  await expect(form.getByRole("textbox")).toBeDisabled();
  await expect(form.getByRole("button", { name: "Browse application", exact: true })).toBeDisabled();
  await overrideCommand(page, { command: "get_application_launch_path", result: { path: null, detectedPath: applications[0].detected } });
  await form.getByRole("button", { name: "Retry", exact: true }).click();
  await expect(form.getByRole("textbox")).toBeEnabled();
  await expect(form.getByRole("alert")).toHaveCount(0);
});

test("save errors retain the draft without replacing the saved path", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true });
  await page.goto("/");
  await openApplications(page);
  const form = await selectApplication(page, applications[1]);
  await form.getByRole("textbox").fill(applications[1].path);
  await form.getByRole("button", { name: "Save", exact: true }).click();
  await expect(form.getByRole("status")).toHaveText("Saved.");
  await overrideCommand(page, { command: "set_application_launch_path", failTimes: 1 });
  const draft = "C:\\Synthetic CLI\\claude.exe";
  await form.getByRole("textbox").fill(draft);
  await form.getByRole("button", { name: "Save", exact: true }).click();
  await expect(form.getByRole("alert")).toContainText("Could not use this path.");
  await expect(form.getByRole("textbox")).toHaveValue(draft);
  await expect(form.getByRole("status")).toHaveCount(0);
  await expect(form.getByRole("button", { name: "Save", exact: true })).toBeEnabled();
  await selectApplication(page, applications[2]);
  await selectApplication(page, applications[1]);
  await expect(form.getByRole("textbox")).toHaveValue(applications[1].path);
});

test("native picker errors leave the current draft editable", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true });
  await page.goto("/");
  await overrideCommand(page, { command: "pick_application_launch_path", failTimes: 1 });
  await openApplications(page);
  const form = await selectApplication(page, applications[2]);
  await form.getByRole("textbox").fill(applications[2].path);
  await form.getByRole("button", { name: "Browse application", exact: true }).click();
  await expect(form.getByRole("alert")).toContainText("Could not use this path.");
  await expect(form.getByRole("textbox")).toHaveValue(applications[2].path);
  await expect(form.getByRole("textbox")).toBeEnabled();
  expect((await calls(page)).filter((call) => call.command === "set_application_launch_path")).toEqual([]);
});

for (const mode of ["local"] as const) {
  test(`${mode} mode keeps application launch preferences on this computer`, async ({ page }) => {
    await installTauriMock(page, { mode, locale: "en", populated: true });
    await page.goto("/");
    await openApplications(page);
    for (const application of applications) {
      const form = await selectApplication(page, application);
      await expect(form.getByRole("button")).toHaveCount(2);
      await expect(form.locator("p")).toHaveCount(0);
      await form.getByRole("textbox").fill(application.path);
      await form.getByRole("button", { name: "Save", exact: true }).click();
      await expect(form.getByRole("status")).toHaveText("Saved.");
    }
    expect((await calls(page)).filter((call) => /^(set_application_launch_path|update_remote_|set_remote_|connect_claude_|attach_)/.test(call.command)))
      .toEqual(applications.map((application) => ({ command: "set_application_launch_path", args: { application: application.id, path: application.path } })));
    expect((await calls(page)).filter((call) => call.command.includes("claude_desktop"))).toEqual([]);
  });
}

test("overview keeps Access available without loading launch preferences", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true });
  await page.goto("/");
  await expect(page.getByRole("heading", { name: "Overview", exact: true })).toBeVisible();
  await expect(page.getByRole("button", { name: "Access", exact: true })).toBeVisible();
  expect((await calls(page)).filter((call) => call.command.includes("application_launch_path"))).toEqual([]);
});

for (const detection of ["missing", "packaged"] as const) {
  test(`${detection} automatic detection does not expose a profile or shell command as a launch path`, async ({ page }) => {
    await installTauriMock(page, { mode: "local", locale: "en", populated: true });
    await page.goto("/");
    await overrideCommand(page, { command: "get_application_launch_path", result: {
      path: null,
      detectedPath: detection === "missing" ? null : "shell:AppsFolder\\Anthropic.Claude_test!App",
    } });
    await openApplications(page);
    const form = await selectApplication(page, applications[1]);
    await expect(form.getByRole("textbox")).toHaveValue("");
    await expect(form.getByRole("textbox")).toHaveAttribute("placeholder", detection === "missing"
      ? "Select an application" : "Installed Windows application");
    await expect(form.getByRole("button", { name: "Browse application", exact: true })).toBeEnabled();
    expect((await calls(page)).filter((call) => call.command === "set_application_launch_path")).toEqual([]);
  });
}

for (const locale of ["en", "ru"] as const) {
  for (const scenario of [
    { width: 1160, theme: "light", expanded: false },
    { width: 390, theme: "dark", expanded: false },
    { width: 390, theme: "dark", expanded: true },
  ] as const) {
    test(`application paths fit ${locale} at ${scenario.width}px with sidebar ${scenario.expanded ? "expanded" : "collapsed"}`, async ({ page }) => {
      const russian = locale === "ru";
      await page.setViewportSize({ width: scenario.width, height: 900 });
      await installTauriMock(page, { mode: "local", locale, theme: scenario.theme, populated: true });
      await page.goto("/");
      await openApplications(page, russian);
      if (scenario.expanded) await page.getByRole("button", { name: russian ? "Развернуть боковую панель" : "Expand sidebar", exact: true }).click();
      for (const application of applications) {
        const form = await selectApplication(page, application);
        await expect(form.getByRole("button")).toHaveCount(2);
        await expect(form.getByRole("button", { name: russian ? "Выбрать приложение" : "Browse application", exact: true })).toBeVisible();
        await expect(form.locator("p")).toHaveCount(0);
        await form.getByRole("textbox", { name: russian ? "Путь приложения" : "Application path", exact: true }).fill(application.path);
        await form.getByRole("button", { name: russian ? "Сохранить" : "Save", exact: true }).click();
        await expect(form.getByRole("status")).toBeVisible();
        expect(await form.evaluate((element) => element.scrollWidth <= element.clientWidth + 1)).toBe(true);
        expect(await form.locator("input, button").evaluateAll((elements) => elements.every((element) => {
          const box = element.getBoundingClientRect();
          const form = element.closest("form")!.getBoundingClientRect();
          return box.left >= form.left && box.right <= form.right + 1 && box.right <= innerWidth + 1;
        }))).toBe(true);
        const connection = page.locator(`[data-application-connection="${application.id}"]`);
        if (application.id === "claude-desktop") {
          await expect(connection.getByRole("button", { name: russian ? "Подключение: Как обычно" : "Connection: As usual" })).toBeEnabled();
          expect(await connection.evaluate((element) => element.scrollWidth <= element.clientWidth + 1)).toBe(true);
        } else {
          await expect(connection).toHaveCount(0);
        }
        await page.screenshot({ path: `output/playwright/launch-path-${application.id}-${locale}-${scenario.width}-${scenario.expanded ? "expanded" : "collapsed"}.png` });
      }
    });
  }
}
