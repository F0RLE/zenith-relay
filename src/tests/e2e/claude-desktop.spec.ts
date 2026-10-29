import { expect, test, type Page } from "../bun-playwright";
import { installTauriMock } from "./tauri-mock";

async function openRecovery(page: Page, russian = false) {
  await page.getByRole("navigation", { name: russian ? "Обслуживание" : "Maintenance", exact: true })
    .getByRole("button", { name: russian ? "Восстановление" : "Recovery", exact: true }).click();
  await page.getByRole("tab", { name: "Claude Desktop", exact: true }).click();
  return page.locator(".profile-recovery-claude-desktop");
}

async function commands(page: Page) {
  return page.evaluate(() => (window as unknown as {
    __TAURI_TEST_INVOKES__: Array<{ command: string }>;
  }).__TAURI_TEST_INVOKES__.map((call) => call.command));
}

test("Pool connects Claude without auto-launch and Recovery restores only after confirmation", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, gatewayRunning: true });
  await page.addInitScript(() => localStorage.setItem("relay.launchApplicationAfterConnect", "0"));
  await page.goto("/");
  await page.getByRole("button", { name: "Pool", exact: true }).click();
  await page.getByRole("button", { name: "Connect", exact: true }).click();
  await page.getByRole("dialog").getByRole("button", { name: "Claude Desktop", exact: true }).click();
  await expect(page.locator(".global-feedback.success")).toContainText("Saved.");
  expect(await commands(page)).not.toContain("launch_claude_desktop");

  const panel = await openRecovery(page);
  await expect(page.getByRole("tab")).toHaveText(["ChatGPT", "Claude Desktop", "OpenCode"]);
  const restore = panel.getByRole("button", { name: "Restore", exact: true });
  await expect(panel.getByPlaceholder("Before changing Claude Desktop")).toBeVisible();
  await expect(panel.getByRole("button", { name: "Create snapshot", exact: true })).toBeDisabled();
  await expect(restore).toBeEnabled();
  await restore.click();
  const confirmation = page.getByRole("dialog", { name: "Restore Claude Desktop config", exact: true });
  await confirmation.getByRole("button", { name: "No", exact: true }).click();
  expect(await commands(page)).not.toContain("restore_claude_desktop");
  await restore.click();
  await confirmation.getByRole("button", { name: "Yes", exact: true }).click();
  await expect(confirmation).toHaveCount(0);
  await expect(panel).toContainText("No snapshots");
  await expect(restore).toHaveCount(0);
  expect((await commands(page)).filter((command) => command === "restore_claude_desktop")).toHaveLength(1);
  expect(await commands(page)).not.toContain("launch_claude_desktop");
});


test("Claude recovery inspection failures are explicit and retryable", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, claudeDesktopConfigured: true });
  await page.goto("/");
  await page.evaluate(() => {
    const state = window as unknown as {
      __FAIL_DESKTOP_STATUS__?: boolean;
      __TAURI_INTERNALS__: { invoke: (command: string, args?: unknown, options?: unknown) => Promise<unknown> };
    };
    state.__FAIL_DESKTOP_STATUS__ = true;
    const invoke = state.__TAURI_INTERNALS__.invoke.bind(state.__TAURI_INTERNALS__);
    state.__TAURI_INTERNALS__.invoke = (command, args, options) =>
      command === "get_claude_desktop_status" && state.__FAIL_DESKTOP_STATUS__
        ? Promise.reject({ code: "profile_restore_blocked", message: "Synthetic configuration conflict" })
        : invoke(command, args, options);
  });
  const panel = await openRecovery(page);
  await expect(panel.getByRole("alert")).toContainText("Some recovery data could not be loaded.");
  await expect(panel.getByRole("button", { name: "Restore", exact: true })).toHaveCount(0);
  await expect(panel.getByRole("button", { name: "Create snapshot", exact: true })).toBeDisabled();
  await page.evaluate(() => { (window as unknown as { __FAIL_DESKTOP_STATUS__: boolean }).__FAIL_DESKTOP_STATUS__ = false; });
  await panel.getByRole("button", { name: "Retry", exact: true }).click();
  await expect(panel.getByRole("alert")).toHaveCount(0);
  await expect(panel.getByRole("button", { name: "Restore", exact: true })).toBeEnabled();
  expect(await commands(page)).not.toContain("restore_claude_desktop");
});

test("Claude recovery remains available with the local API stopped", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, gatewayRunning: false, claudeDesktopConfigured: true });
  await page.goto("/");
  const panel = await openRecovery(page);
  await panel.getByRole("button", { name: "Restore", exact: true }).click();
  await page.getByRole("dialog", { name: "Restore Claude Desktop config", exact: true }).getByRole("button", { name: "Yes", exact: true }).click();
  await expect(panel).toContainText("No snapshots");
  expect(await commands(page)).not.toContain("start_local_gateway");
  expect(await commands(page)).not.toContain("launch_claude_desktop");
});

test("a failed Claude restore keeps the confirmation and saved recovery available", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, claudeDesktopConfigured: true });
  await page.goto("/");
  await page.evaluate(() => {
    const internals = (window as unknown as {
      __TAURI_INTERNALS__: { invoke: (command: string, args?: unknown, options?: unknown) => Promise<unknown> };
    }).__TAURI_INTERNALS__;
    const invoke = internals.invoke.bind(internals);
    internals.invoke = (command, args, options) => command === "restore_claude_desktop"
      ? Promise.reject({ code: "profile_restore_blocked", message: "Synthetic changed profile" })
      : invoke(command, args, options);
  });
  const panel = await openRecovery(page);
  await panel.getByRole("button", { name: "Restore", exact: true }).click();
  const confirmation = page.getByRole("dialog", { name: "Restore Claude Desktop config", exact: true });
  await confirmation.getByRole("button", { name: "Yes", exact: true }).click();
  await expect(page.locator(".global-feedback.error")).toBeVisible();
  await expect(confirmation).toBeVisible();
  await expect(confirmation.getByRole("button", { name: "No", exact: true })).toBeEnabled();
  await confirmation.getByRole("button", { name: "No", exact: true }).click();
  await expect(panel.getByRole("button", { name: "Restore", exact: true })).toBeEnabled();
  expect(await commands(page)).not.toContain("launch_claude_desktop");
});


for (const locale of ["en", "ru"] as const) {
  for (const width of [390, 1160]) {
    test(`Claude recovery fits ${locale} at ${width}px`, async ({ page }) => {
      await page.setViewportSize({ width, height: 844 });
      await installTauriMock(page, { mode: "local", locale, theme: "dark", populated: true, claudeDesktopConfigured: true });
      await page.goto("/");
      const panel = await openRecovery(page, locale === "ru");
      await expect(panel.getByRole("button", { name: locale === "ru" ? "Восстановить" : "Restore", exact: true })).toBeEnabled();
      expect(await panel.evaluate((element) => element.scrollWidth <= element.clientWidth + 1)).toBe(true);
      expect(await panel.locator(".profile-recovery-panel").getByRole("button").evaluateAll((elements) => elements.every((element) => {
        const box = element.getBoundingClientRect();
        return box.left >= 0 && box.right <= innerWidth + 1;
      }))).toBe(true);
      await page.screenshot({ path: `output/playwright/claude-recovery-${locale}-${width}.png` });
    });
  }
}

test("Claude uses the shared snapshot form and opens its own recovery folder without connecting", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, gatewayRunning: false });
  await page.goto("/");
  const panel = await openRecovery(page);
  await expect(panel).toContainText("No snapshots");
  const create = panel.getByRole("button", { name: "Create snapshot", exact: true });
  await expect(create).toBeDisabled();
  await panel.getByLabel("Snapshot name", { exact: true }).fill("  Before Claude  ");
  await create.click();
  await expect(panel.locator("tbody")).toContainText("Before Claude");
  await expect(panel.getByLabel("Snapshot name", { exact: true })).toHaveValue("");
  await expect(create).toBeDisabled();
  await page.getByRole("button", { name: "Open backups folder", exact: true }).click();
  const folderCalls = await page.evaluate(() => (window as unknown as {
    __TAURI_TEST_INVOKES__: Array<{ command: string; args: { folder?: string; name?: string } }>;
  }).__TAURI_TEST_INVOKES__.filter((call) => call.command === "open_relay_folder" || call.command === "create_claude_desktop_snapshot"));
  expect(folderCalls).toEqual([
    { command: "create_claude_desktop_snapshot", args: { name: "Before Claude" } },
    { command: "open_relay_folder", args: { folder: "claude_desktop_backups" } },
  ]);
  expect(await commands(page)).not.toContain("connect_claude_desktop");
  expect(await commands(page)).not.toContain("launch_claude_desktop");
  expect(await commands(page)).not.toContain("start_local_gateway");
});

test("a failed Claude snapshot keeps the name and does not show a false recovery point", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true });
  await page.goto("/");
  await page.evaluate(() => {
    const internals = (window as unknown as {
      __TAURI_INTERNALS__: { invoke: (command: string, args?: unknown, options?: unknown) => Promise<unknown> };
    }).__TAURI_INTERNALS__;
    const invoke = internals.invoke.bind(internals);
    internals.invoke = (command, args, options) => command === "create_claude_desktop_snapshot"
      ? Promise.reject({ code: "io", message: "Synthetic snapshot failure" })
      : invoke(command, args, options);
  });
  const panel = await openRecovery(page);
  await panel.getByLabel("Snapshot name", { exact: true }).fill("Before Claude");
  await panel.getByRole("button", { name: "Create snapshot", exact: true }).click();
  await expect(page.locator(".global-feedback.error")).toBeVisible();
  await expect(panel.getByLabel("Snapshot name", { exact: true })).toHaveValue("Before Claude");
  await expect(panel).toContainText("No snapshots");
  await expect(panel.locator("tbody")).toHaveCount(0);
});
