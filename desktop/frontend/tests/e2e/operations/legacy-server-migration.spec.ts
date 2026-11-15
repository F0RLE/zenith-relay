import { expect, test } from "../../bun-playwright";
import { installTauriMock } from "../tauri-mock";

for (const locale of ["en", "ru"] as const) {
  test(`${locale} saved personal-server mode opens Computer without remote commands`, async ({ page }) => {
    await installTauriMock(page, { storedMode: "remote", locale, populated: true, gatewayRunning: true });
    await page.goto("/");
    await expect.poll(() => page.evaluate(() => localStorage.getItem("relay.mode"))).toBe("local");
    await page.locator(".mode-picker-trigger").click();
    await expect(page.getByRole("menuitemradio")).toHaveText(locale === "ru"
      ? ["На устройстве", "На сервере"] : ["Computer", "Relay Hosted"]);
    await page.keyboard.press("Escape");
    await page.getByRole("button", { name: locale === "ru" ? "Подключения" : "Connections", exact: true }).click();
    await expect(page.locator('.relay-page[data-view="accounts"]')).toBeVisible();
    await expect(page.getByRole("tab", { name: locale === "ru" ? "Удалённый сервер" : "Remote Server", exact: true })).toHaveCount(0);
    const commands = await page.evaluate(() => (window as unknown as {
      __TAURI_TEST_INVOKES__: Array<{ command: string }>;
    }).__TAURI_TEST_INVOKES__.map(({ command }) => command));
    expect(commands).toContain("get_local_runtime_state");
    expect(commands.filter((command) => /^(?:get_remote_|connect_remote_|prepare_remote_|execute_remote_|stop_local_gateway|delete_local_|move_local_accounts_to_remote)/.test(command))).toEqual([]);
  });
}

test("personal-server setup is retired without changing the Hosted placeholder", async ({ page }) => {
  await installTauriMock(page, { storedMode: "remote", locale: "en", onboarding: false });
  await page.goto("/");
  await page.getByRole("button", { name: "Get started", exact: true }).click();
  await expect(page.locator(".mode-options button")).toHaveCount(2);
  await expect(page.getByRole("button", { name: /On your server/ })).toHaveCount(0);
  await page.getByRole("button", { name: "Continue", exact: true }).click();
  await expect(page.getByRole("heading", { name: "Add connections to your pool", exact: true })).toBeVisible();
  await expect(page.getByLabel("Server address", { exact: true })).toHaveCount(0);
  await expect(page.getByLabel("Management token", { exact: true })).toHaveCount(0);
});
