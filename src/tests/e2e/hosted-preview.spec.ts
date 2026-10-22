import { expect, test } from "../bun-playwright";
import { installTauriMock } from "./tauri-mock";

for (const locale of ["ru", "en"] as const) {
  test(`${locale} development launcher opens the isolated Hosted demo without touching runtimes`, async ({ page }) => {
    test.skip(!test.info().config.configFile?.endsWith("playwright.hosted-preview.config.ts"), "The preview action is development-only.");
    await installTauriMock(page, { mode: "hosted", locale, populated: true });
    await page.goto("/");
    const button = page.getByRole("button", { name: locale === "ru" ? "Открыть демо Hosted" : "Open Hosted demo", exact: true });
    await expect(button).toBeVisible();
    await button.click();
    const commands = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args?: unknown }> }).__TAURI_TEST_INVOKES__);
    expect(commands.filter(call => call.command === "open_hosted_preview")).toHaveLength(1);
    expect(commands.some(call => call.command === "get_local_runtime_state" || call.command === "stop_local_gateway" || call.command.startsWith("get_remote_"))).toBe(false);
    await expect(page.getByRole("button", { name: locale === "ru" ? "Использовать это устройство" : "Use this computer", exact: true })).toBeVisible();
  });
}

test("failed preview launch shows recovery, not a successful launch", async ({ page }) => {
  test.skip(!test.info().config.configFile?.endsWith("playwright.hosted-preview.config.ts"), "The preview action is development-only.");
  await installTauriMock(page, { mode: "hosted", locale: "ru" });
  await page.goto("/");
  await page.evaluate(() => {
    const internals = (window as unknown as { __TAURI_INTERNALS__: { invoke: (command: string, ...args: unknown[]) => Promise<unknown> } }).__TAURI_INTERNALS__;
    const original = internals.invoke.bind(internals);
    internals.invoke = (command, ...args) => command === "open_hosted_preview" ? Promise.reject(new Error("synthetic preview unavailable")) : original(command, ...args);
  });
  await page.getByRole("button", { name: "Открыть демо Hosted", exact: true }).click();
  await expect(page.getByRole("alert")).toContainText("bun run dev");
  await expect(page.getByRole("button", { name: "Открыть демо Hosted", exact: true })).toBeEnabled();
});
