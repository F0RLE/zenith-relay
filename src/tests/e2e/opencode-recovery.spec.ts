import { expect, test, type Page } from "../bun-playwright";
import { installTauriMock } from "./tauri-mock";

async function createSnapshot(page: Page) {
  await installTauriMock(page, { locale: "en", populated: true, gatewayRunning: false });
  await page.goto("/");
  await page.getByRole("navigation", { name: "Maintenance", exact: true })
    .getByRole("button", { name: "Recovery", exact: true }).click();
  await page.getByRole("tab", { name: "OpenCode", exact: true }).click();
  const panel = page.locator(".profile-recovery-opencode");
  await expect(panel).toContainText("No snapshots");
  await panel.getByLabel("Snapshot name", { exact: true }).fill("  Before OpenCode  ");
  await panel.getByRole("button", { name: "Create snapshot", exact: true }).click();
  await expect(panel.locator("tbody")).toContainText("Before OpenCode");
  return panel;
}

test("OpenCode restore consumes its snapshot only after confirmation with the API stopped", async ({ page }) => {
  const panel = await createSnapshot(page);
  await expect(panel.getByRole("button", { name: "Create snapshot", exact: true })).toBeDisabled();
  await panel.getByRole("button", { name: "Restore snapshot", exact: true }).click();
  const confirmation = page.getByRole("dialog", { name: "Restore OpenCode config", exact: true });
  await confirmation.getByRole("button", { name: "No", exact: true }).click();
  await expect(panel.locator("tbody")).toContainText("Before OpenCode");
  await panel.getByRole("button", { name: "Restore snapshot", exact: true }).click();
  await confirmation.getByRole("button", { name: "Yes", exact: true }).click();
  await expect(confirmation).toHaveCount(0);
  await expect(panel).toContainText("No snapshots");
  const calls = await page.evaluate(() => (window as unknown as {
    __TAURI_TEST_INVOKES__: Array<{ command: string; args?: unknown }>;
  }).__TAURI_TEST_INVOKES__);
  expect(calls.filter((call) => call.command === "create_opencode_snapshot")).toEqual([
    { command: "create_opencode_snapshot", args: { name: "Before OpenCode" } },
  ]);
  expect(calls.filter((call) => call.command === "restore_opencode_config")).toHaveLength(1);
  expect(calls.map((call) => call.command)).not.toContain("start_local_gateway");
  expect(calls.map((call) => call.command)).not.toContain("restart_opencode_app");
  expect(calls.map((call) => call.command)).not.toContain("connect_opencode_to_local_gateway");
});

for (const failure of ["invalid_state", "conflict", "recovery_required"]) {
  test(`a failed OpenCode restore (${failure}) keeps its snapshot and confirmation available`, async ({ page }) => {
    const panel = await createSnapshot(page);
    await page.evaluate((code) => {
      const internals = (window as unknown as {
        __TAURI_INTERNALS__: { invoke: (command: string, args?: unknown, options?: unknown) => Promise<unknown> };
      }).__TAURI_INTERNALS__;
      const invoke = internals.invoke.bind(internals);
      internals.invoke = (command, args, options) => command === "restore_opencode_config"
        ? Promise.reject({ code, message: "Synthetic OpenCode restore failure" })
        : invoke(command, args, options);
    }, failure);
    await panel.getByRole("button", { name: "Restore snapshot", exact: true }).click();
    const confirmation = page.getByRole("dialog", { name: "Restore OpenCode config", exact: true });
    await confirmation.getByRole("button", { name: "Yes", exact: true }).click();
    await expect(page.locator(".global-feedback.error")).toBeVisible();
    await expect(confirmation).toBeVisible();
    await expect(panel.locator("tbody")).toContainText("Before OpenCode");
    await confirmation.getByRole("button", { name: "No", exact: true }).click();
    await expect(panel.getByRole("button", { name: "Restore snapshot", exact: true })).toBeEnabled();
  });
}
