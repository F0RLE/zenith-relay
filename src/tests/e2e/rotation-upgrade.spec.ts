import { expect, test } from "../bun-playwright";
import type { RuntimeSnapshot } from "../../src/features/relay/api/types";
import { installTauriMock } from "./tauri-mock";
import { openPoolRotation } from "./pool-rotation";

for (const mode of ["local"] as const) {
  test(`${mode} rotation is immediately editable without a migration dialog or gateway stop`, async ({ page }) => {
    await installTauriMock(page, { mode, locale: "en", populated: true, gatewayRunning: true, chatgptRetryUntilAvailable: true });
    await page.goto("/");
    const dialog = await openPoolRotation(page);
    await expect(dialog.getByRole("radio", { name: "Automatic", exact: true })).toBeEnabled();
    await expect(dialog.getByRole("checkbox")).toHaveCount(0);
    await expect(dialog.getByText(/migration|rollback|confirmation/i)).toHaveCount(0);
    await dialog.getByRole("radio", { name: "Round robin", exact: true }).click();
    await dialog.getByRole("button", { name: "Close", exact: true }).last().click();
    const saved = await page.evaluate(async () => (window as unknown as { __TAURI_INTERNALS__: { invoke: (command: string) => Promise<RuntimeSnapshot> } }).__TAURI_INTERNALS__.invoke("get_local_runtime_state"));
    expect(saved.gateway).toMatchObject({ poolRouting: { version: 3, mode: "round_robin" }, running: true, chatgptRetryUntilAvailable: true });
    const stops = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: any }> }).__TAURI_TEST_INVOKES__.filter((call) => call.command === "stop_local_gateway" || ["stop_gateway", "preview_rotation_migration", "apply_rotation_migration"].includes(call.args?.input?.action?.type)));
    expect(stops).toHaveLength(0);
  });
}
