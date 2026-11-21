import { expect, test } from "../bun-playwright";
import { installTauriMock } from "./tauri-mock";

test("source recovery selector preserves a saved value outside the quick presets", async ({ page }) => {
  await installTauriMock(page, {
    mode: "local", locale: "en", populated: true, sourceRecoveryDelaySeconds: 120,
  });
  await page.goto("/");
  await page.getByRole("button", { name: "Pool", exact: true }).click();
  await page.getByRole("button", { name: "Pool member policy: Example compatible API", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "Pool member policy", exact: true });
  await dialog.getByRole("tab", { name: "Settings", exact: true }).click();
  const selector = dialog.getByRole("button", { name: "Recovery check: After 2 min", exact: true });
  await expect(selector).toBeVisible();
  await selector.click();
  const selected = page.getByRole("option", { name: "After 2 min", exact: true });
  await expect(selected).toHaveAttribute("aria-selected", "true");
  await expect(page.getByRole("option")).toHaveCount(7);
  await page.locator('[role="option"][data-value="60"]').click();
  await dialog.getByRole("button", { name: /^Recovery check:/ }).click();
  await selected.click();
  await expect(selector).toBeVisible();
  await dialog.getByRole("button", { name: /^Save/ }).click();
  await expect(dialog).toHaveCount(0);
  const updates = await page.evaluate(() => (window as unknown as {
    __TAURI_TEST_INVOKES__: Array<{ command: string; args: { input?: { recoveryDelaySeconds?: number } } }>;
  }).__TAURI_TEST_INVOKES__.filter((call) => call.command === "update_local_source"));
  expect(updates).toHaveLength(1);
  expect(updates[0]?.args.input?.recoveryDelaySeconds).toBe(120);
});
