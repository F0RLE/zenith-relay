import { expect, test, type Page } from "../bun-playwright";
import type { RuntimeSnapshot } from "../../src/features/relay/api/types";
import { installTauriMock } from "./tauri-mock";

async function openRotation(page: Page) {
  await page.getByRole("button", { name: "Pool", exact: true }).click();
  await page.getByRole("button", { name: "Pool rotation settings", exact: true }).click();
  return page.getByRole("dialog", { name: "Pool rotation", exact: true });
}

for (const mode of ["local", "remote"] as const) {
  test(`${mode} rotation is immediately editable without a migration dialog or gateway stop`, async ({ page }) => {
    await installTauriMock(page, { mode, locale: "en", populated: true, gatewayRunning: true, chatgptRetryUntilAvailable: true });
    await page.goto("/");
    const dialog = await openRotation(page);
    await expect(dialog.getByRole("radio", { name: "Automatic", exact: true })).toBeEnabled();
    await expect(dialog.getByRole("checkbox")).toHaveCount(0);
    await expect(dialog.getByText(/migration|rollback|confirmation/i)).toHaveCount(0);
    await dialog.getByRole("radio", { name: "In order", exact: true }).click();
    await dialog.getByRole("button", { name: "Close", exact: true }).last().click();
    const saved = await page.evaluate(async (mode) => (window as unknown as { __TAURI_INTERNALS__: { invoke: (command: string) => Promise<RuntimeSnapshot> } }).__TAURI_INTERNALS__.invoke(mode === "local" ? "get_local_runtime_state" : "get_remote_server_state"), mode);
    expect(saved.gateway).toMatchObject({ poolRouting: { version: 2, mode: "in_order" }, running: true, chatgptRetryUntilAvailable: true });
    const stops = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: any }> }).__TAURI_TEST_INVOKES__.filter((call) => call.command === "stop_local_gateway" || ["stop_gateway", "preview_rotation_migration", "apply_rotation_migration"].includes(call.args?.input?.action?.type)));
    expect(stops).toHaveLength(0);
  });
}

test("old servers cannot receive current rotation settings", async ({ page }) => {
  await installTauriMock(page, { mode: "remote", locale: "en", rotationVersion: 1, remoteFeatures: ["sources", "accounts", "runtime_routing"] });
  await page.goto("/");
  const dialog = await openRotation(page);
  for (const option of await dialog.getByRole("radio").all()) await expect(option).toBeDisabled();
  const writes = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: any }> }).__TAURI_TEST_INVOKES__.filter((call) => call.command === "execute_remote_server_action" && ["set_routing_policy", "preview_rotation_migration", "apply_rotation_migration"].includes(call.args.input?.action?.type)));
  expect(writes).toHaveLength(0);
});
