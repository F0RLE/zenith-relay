import { expect, test } from "../bun-playwright";
import { installTauriMock } from "./tauri-mock";

for (const mode of ["local"] as const) {
  test(`${mode} legacy model protection does not expose a transport switch`, async ({ page }) => {
    await installTauriMock(page, { mode, basisPointsAvailable: true, basisPointsEnabled: true });
    await page.goto("/");
    await page.getByRole("button", { name: "Access", exact: true }).click();
    await expect(page.locator(".gateway-api-connection-panel")).toBeVisible();
    await expect(page.getByRole("checkbox", { name: "Use Basis Points" })).toHaveCount(0);
    await page.getByRole("button", { name: "Connections", exact: true }).click();
    await expect(page.getByRole("tab", { name: "Accounts", exact: true })).toHaveAttribute("aria-selected", "true");
    await expect(page.locator(".model-protection-control")).toHaveCount(0);
  });
}
