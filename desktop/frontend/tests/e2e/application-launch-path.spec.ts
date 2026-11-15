import { expect, test, type Page } from "../bun-playwright";
import { installTauriMock } from "./tauri-mock";

const applications = [
  { id: "chatgpt", name: "ChatGPT", path: "D:\\Apps\\ChatGPT\\ChatGPT.exe", detected: "C:\\Program Files\\ChatGPT\\ChatGPT.exe" },
  { id: "claude-desktop", name: "Claude Desktop", path: "D:\\Apps\\Claude\\Claude.exe", detected: "C:\\Users\\Test\\AppData\\Local\\Claude\\Claude.exe" },
  { id: "opencode", name: "OpenCode", path: "D:\\Apps\\OpenCode\\OpenCode.exe", detected: "C:\\Users\\Test\\AppData\\Local\\Programs\\OpenCode\\OpenCode.exe" },
] as const;

type InvokeCall = { command: string; args: Record<string, unknown> };

async function calls(page: Page) {
  return page.evaluate(() => (window as unknown as {
    __TAURI_TEST_INVOKES__: InvokeCall[];
  }).__TAURI_TEST_INVOKES__);
}

async function openApplications(page: Page, russian = false) {
  await page.getByRole("button", { name: russian ? "Доступ" : "Access", exact: true }).click();
  await page.getByRole("tab", { name: russian ? "Приложения" : "Applications", exact: true }).click();
}

async function selectApplication(page: Page, application: typeof applications[number]) {
  await page.locator(".application-workspace .integration-picker").getByRole("tab", { name: application.name, exact: true }).click();
  const form = page.locator(`.application-launch-path[data-application="${application.id}"]`);
  await expect(form.getByRole("textbox")).toBeEnabled();
  return form;
}

async function overrideCommand(page: Page, override: { command: string; failTimes?: number; result?: unknown }) {
  await page.evaluate((override) => {
    const internals = (window as unknown as {
      __TAURI_INTERNALS__: { invoke: (command: string, args?: unknown, options?: unknown) => Promise<unknown> };
    }).__TAURI_INTERNALS__;
    const invoke = internals.invoke.bind(internals);
    let remaining = override.failTimes ?? 0;
    internals.invoke = (command, args, options) => {
      if (command !== override.command) return invoke(command, args, options);
      if (remaining > 0) {
        remaining -= 1;
        return Promise.reject("Synthetic application setting failure");
      }
      return "result" in override ? Promise.resolve(override.result) : invoke(command, args, options);
    };
  }, override);
}

for (const application of applications) {
  test(`${application.name} starts with only an application path, folder picker and Save`, async ({ page }) => {
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
        await page.screenshot({ path: `output/playwright/launch-path-${application.id}-${locale}-${scenario.width}-${scenario.expanded ? "expanded" : "collapsed"}.png` });
      }
    });
  }
}
