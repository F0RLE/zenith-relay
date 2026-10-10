import { expect, test } from "../bun-playwright";
import { installTauriMock } from "./tauri-mock";

for (const mode of ["local", "remote"] as const) {
  test(`${mode} legacy model protection does not expose a transport switch`, async ({ page }) => {
    await installTauriMock(page, { mode, basisPointsAvailable: true, basisPointsEnabled: true });
    await page.goto("/");
    await page.getByRole("button", { name: "API", exact: true }).click();
    await expect(page.locator(".gateway-api-connection-panel")).toBeVisible();
    await expect(page.getByRole("checkbox", { name: "Use Basis Points" })).toHaveCount(0);
    await page.getByRole("button", { name: "Connections", exact: true }).click();
    await expect(page.getByRole("tab", { name: "Accounts", exact: true })).toHaveAttribute("aria-selected", "true");
    await expect(page.locator(".model-protection-control")).toHaveCount(0);
  });
}

test("an older server without the transport setting does not get a model protection control", async ({ page }) => {
  await installTauriMock(page, { mode: "remote", basisPointsAvailable: true });
  await page.addInitScript(() => {
    const internals = (window as unknown as { __TAURI_INTERNALS__: { invoke: (command: string, args?: unknown) => Promise<unknown> } }).__TAURI_INTERNALS__;
    const original = internals.invoke.bind(internals);
    internals.invoke = async (command, args) => {
      const result = await original(command, args);
      if (command === "get_remote_server_state") {
        delete (result as { gateway: { basisPointsEnabled?: boolean } }).gateway.basisPointsEnabled;
      }
      return result;
    };
  });
  await page.goto("/");
  await page.getByRole("button", { name: "API", exact: true }).click();
  await expect(page.locator(".gateway-api-connection-panel")).toBeVisible();
  await expect(page.locator(".model-protection-control")).toHaveCount(0);
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await expect(page.getByRole("tab", { name: "Accounts", exact: true })).toHaveAttribute("aria-selected", "true");
  await expect(page.locator(".model-protection-control")).toHaveCount(0);
});
