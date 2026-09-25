import { expect, test } from "../bun-playwright";
import { installTauriMock } from "./tauri-mock";

for (const mode of ["local", "remote"] as const) {
  test(`${mode} model protection restores the saved value after a failed update`, async ({ page }) => {
    await installTauriMock(page, { mode, basisPointsAvailable: true });
    await page.goto("/");
    await page.getByRole("button", { name: "API", exact: true }).click();
    await page.evaluate(() => {
      const scope = window as unknown as {
        __TAURI_INTERNALS__: { invoke: (command: string, args?: unknown) => Promise<unknown> };
        __REJECT_MODEL_PROTECTION__: () => void;
      };
      const original = scope.__TAURI_INTERNALS__.invoke.bind(scope.__TAURI_INTERNALS__);
      let fail = true;
      scope.__TAURI_INTERNALS__.invoke = (command, args) => {
        const input = (args as { input?: { basisPointsEnabled?: boolean; payload?: { basisPointsEnabled?: boolean } } })?.input;
        if (fail && (command === "update_local_routing" || command === "execute_remote_server_action")
          && (input?.basisPointsEnabled === true || input?.payload?.basisPointsEnabled === true)) {
          fail = false;
          return new Promise((_, reject) => {
            scope.__REJECT_MODEL_PROTECTION__ = () => reject({ code: "upstream_error", message: "Synthetic save failure" });
          });
        }
        return original(command, args);
      };
    });
    const toggle = page.getByRole("checkbox", { name: "Model substitution protection" });
    await toggle.check();
    await expect(toggle).toBeChecked();
    await expect(toggle).toBeDisabled();
    await page.evaluate(() => (window as unknown as { __REJECT_MODEL_PROTECTION__: () => void }).__REJECT_MODEL_PROTECTION__());
    await expect(page.locator(".global-feedback.error")).toBeVisible();
    await expect(toggle).toBeEnabled();
    await expect(toggle).not.toBeChecked();
    await toggle.check();
    await expect(toggle).toBeEnabled();
    await expect(toggle).toBeChecked();
    await page.getByRole("button", { name: "Connections", exact: true }).click();
    await expect(page.getByRole("tab", { name: "Accounts", exact: true })).toHaveAttribute("aria-selected", "true");
    await expect(toggle).toHaveCount(0);
    await page.getByRole("button", { name: "API", exact: true }).click();
    await expect(toggle).toBeChecked();
  });
}

test("model protection can be configured before adding an eligible account to the pool", async ({ page }) => {
  await installTauriMock(page, { mode: "local", basisPointsAvailable: true, poolMembers: false });
  await page.goto("/");
  await page.getByRole("button", { name: "API", exact: true }).click();
  const toggle = page.getByRole("checkbox", { name: "Model substitution protection" });
  await toggle.check();
  await expect(toggle).toBeEnabled();
  await expect(toggle).toBeChecked();
});

test("a saved model protection setting can still be disabled when no account supports it", async ({ page }) => {
  await installTauriMock(page, { mode: "local", basisPointsAvailable: false, basisPointsEnabled: true });
  await page.goto("/");
  await page.getByRole("button", { name: "API", exact: true }).click();
  const toggle = page.getByRole("checkbox", { name: "Model substitution protection" });
  await expect(toggle).toBeChecked();
  // Successful disabling removes this control when no eligible accounts remain.
  await toggle.click();
  await expect(page.locator(".global-feedback.success")).toBeVisible();
  await expect(page.locator(".model-protection-control")).toHaveCount(0);
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await expect(page.getByRole("tab", { name: "Accounts", exact: true })).toHaveAttribute("aria-selected", "true");
  await expect(page.locator(".model-protection-control")).toHaveCount(0);
});

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

for (const theme of ["light", "dark"] as const) {
  for (const width of [1160, 360]) {
    test(`model protection description fits ${theme} at ${width}px`, async ({ page }, testInfo) => {
      await page.setViewportSize({ width, height: 920 });
      await installTauriMock(page, { mode: "local", locale: "ru", theme, basisPointsAvailable: true });
      await page.goto("/");
      await page.getByRole("button", { name: "API", exact: true }).click();
      const control = page.locator(".model-protection-control");
      await expect(control.getByRole("checkbox", { name: "Защита от подмены модели" })).toBeVisible();
      await expect(control).toContainText("аккаунтах OpenAI");
      expect(await control.evaluate((element) => Array.from(element.querySelectorAll("label, input")).every((node) => {
        const rect = node.getBoundingClientRect();
        return node.scrollWidth <= node.clientWidth + 1 && rect.left >= 0 && rect.right <= innerWidth;
      }))).toBe(true);
      await control.screenshot({ path: testInfo.outputPath("api-setting.png"), animations: "disabled" });
      await page.screenshot({ path: testInfo.outputPath("api.png"), animations: "disabled" });
      await page.getByRole("button", { name: "Подключения", exact: true }).click();
      await expect(page.getByRole("tab", { name: "Учётные записи", exact: true })).toHaveAttribute("aria-selected", "true");
      await expect(page.locator(".account-card").first()).toBeVisible();
      await expect(control).toHaveCount(0);
      await page.screenshot({ path: testInfo.outputPath("connections.png"), animations: "disabled" });
    });
  }
}
