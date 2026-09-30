import { expect, test, type Page } from "../bun-playwright";
import { installTauriMock, type MockOptions } from "./tauri-mock";

async function openApi(page: Page, options: MockOptions = {}) {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, ...options });
  await page.goto("/");
  await page.getByRole("button", { name: "API", exact: true }).click();
  await expect(page.getByRole("tabpanel", { name: "API", exact: true })).toBeVisible();
}

function keyCommands(page: Page) {
  return page.evaluate(() => (window as unknown as {
    __TAURI_TEST_INVOKES__: Array<{ command: string }>;
  }).__TAURI_TEST_INVOKES__.filter(({ command }) => /^(reveal|rotate)_(local|remote)_gateway_api_key$/.test(command)).map(({ command }) => command));
}

for (const mode of ["local", "remote"] as const) {
  test(`${mode} API copies credentials only on request and confirms key replacement`, async ({ page }) => {
    await openApi(page, { mode });
    const tab = page.getByRole("tabpanel", { name: "API", exact: true });
    const address = await tab.locator(".gateway-api-address").innerText();
    await tab.getByRole("button", { name: "Copy address", exact: true }).click();
    await expect.poll(() => page.evaluate(() => navigator.clipboard.readText())).toBe(address);
    expect(await keyCommands(page)).toEqual([]);
    const copyKey = tab.getByRole("button", { name: "Copy key", exact: true });
    await copyKey.click();
    await expect(copyKey).toBeEnabled();
    const prefix = mode === "local" ? "zlr" : "zrs";
    await expect.poll(() => page.evaluate(() => navigator.clipboard.readText())).toBe(`${prefix}_synthetic_${mode}_gateway_key`);
    expect(await keyCommands(page)).toEqual([`reveal_${mode}_gateway_api_key`]);
    await expect(tab).not.toContainText("synthetic");

    const menu = tab.locator(".relay-action-menu summary");
    await menu.click();
    await page.getByRole("menuitem", { name: "Reissue API key" }).click();
    const confirmation = page.getByRole("dialog", { name: "Reissue API key" });
    await confirmation.getByRole("button", { name: "Cancel", exact: true }).click();
    expect(await keyCommands(page)).toEqual([`reveal_${mode}_gateway_api_key`]);
    await menu.click();
    await page.getByRole("menuitem", { name: "Reissue API key" }).click();
    await confirmation.getByRole("button", { name: "Reissue API key", exact: true }).click();
    await expect.poll(() => keyCommands(page)).toEqual([`reveal_${mode}_gateway_api_key`, `rotate_${mode}_gateway_api_key`]);
    await expect(copyKey).toBeEnabled();
    await expect.poll(() => page.evaluate(() => navigator.clipboard.readText())).toBe(`${prefix}_synthetic_rotated_gateway_key`);
    await expect(tab).not.toContainText("synthetic");
  });
}

for (const running of [true, false]) {
  test(`API port changes are explicit and validated while ${running ? "running" : "stopped"}`, async ({ page }) => {
    await openApi(page, { gatewayRunning: running });
    const tab = page.getByRole("tabpanel", { name: "API", exact: true });
    const port = tab.getByRole("spinbutton", { name: "Port", exact: true });
    const save = tab.getByRole("button", { name: running ? "Save and restart" : "Save", exact: true });
    const portCalls = () => page.evaluate(() => (window as unknown as {
      __TAURI_TEST_INVOKES__: Array<{ command: string; args: { port?: number } }>;
    }).__TAURI_TEST_INVOKES__.filter(({ command }) => command === "update_local_gateway_port").map(({ args }) => args.port));
    await expect(save).toBeDisabled();
    await port.press("Enter");
    for (const value of ["", "1023", "65536", "14998.5"]) {
      await port.fill(value);
      await expect(save).toBeDisabled();
      await port.press("Enter");
    }
    expect(await portCalls()).toEqual([]);
    await port.fill("15001");
    await expect(save).toBeEnabled();
    await expect(tab.locator(".gateway-api-address")).toHaveText("http://127.0.0.1:14998/v1");
    if (running) await expect(tab.getByText("Saving will restart the API at the new address.")).toBeVisible();
    await port.press("Enter");
    await expect(tab.locator(".gateway-api-address")).toHaveText("http://127.0.0.1:15001/v1");
    await expect(save).toBeDisabled();
    expect(await portCalls()).toEqual([15001]);
    await expect(tab.getByRole("heading", { name: running ? "API is running" : "API is stopped" })).toBeVisible();
  });
}

for (const scenario of [
  { name: "no key support", features: ["local_gateway"], running: true, copy: false, rotate: false },
  { name: "copy without rotation", features: ["local_gateway", "profile_attach"], running: true, copy: true, rotate: false },
  { name: "rotation without profile attach", features: ["local_gateway", "profile_key_rotation"], running: true, copy: false, rotate: false },
  { name: "stopped server", features: ["local_gateway", "profile_attach", "profile_key_rotation"], running: false, copy: false, rotate: false },
]) {
  test(`remote API respects capabilities: ${scenario.name}`, async ({ page }) => {
    await openApi(page, { mode: "remote", remoteFeatures: scenario.features, gatewayRunning: scenario.running });
    const tab = page.getByRole("tabpanel", { name: "API", exact: true });
    await expect(tab.getByRole("spinbutton", { name: "Port" })).toHaveCount(0);
    await expect(tab.getByRole("button", { name: "Copy key", exact: true })).toBeEnabled({ enabled: scenario.copy });
    await tab.locator(".relay-action-menu summary").click();
    await expect(page.getByRole("menuitem", { name: "Reissue API key" })).toBeEnabled({ enabled: scenario.rotate });
    expect(await keyCommands(page)).toEqual([]);
  });
}

test("a failed port update preserves the current address and the editable draft", async ({ page }) => {
  await openApi(page);
  await page.evaluate(() => {
    const internals = (window as unknown as { __TAURI_INTERNALS__: { invoke: (command: string, args?: unknown, options?: unknown) => Promise<unknown> } }).__TAURI_INTERNALS__;
    const invoke = internals.invoke.bind(internals);
    let failNextSave = true;
    internals.invoke = async (command, args, options) => {
      if (command === "update_local_gateway_port" && failNextSave) {
        failNextSave = false;
        throw { code: "gateway_unavailable", message: "Synthetic port conflict" };
      }
      return invoke(command, args, options);
    };
  });
  const tab = page.getByRole("tabpanel", { name: "API", exact: true });
  const port = tab.getByRole("spinbutton", { name: "Port", exact: true });
  const save = tab.getByRole("button", { name: "Save and restart", exact: true });
  await port.fill("15001");
  await save.click();
  await expect(page.locator(".global-feedback.error")).toBeVisible();
  await expect(port).toHaveValue("15001");
  await expect(tab.locator(".gateway-api-address")).toHaveText("http://127.0.0.1:14998/v1");
  await expect(save).toBeEnabled();
  await port.fill("15002");
  await save.click();
  await expect(tab.locator(".gateway-api-address")).toHaveText("http://127.0.0.1:15002/v1");
  await expect(save).toBeDisabled();
});

for (const theme of ["light", "dark"] as const) {
  for (const width of [1160, 840, 390]) {
    test(`API connection layout ${theme} ${width}`, async ({ page }) => {
      await page.setViewportSize({ width, height: 800 });
      await openApi(page, { locale: "ru", theme, accountCount: 4 });
      const tab = page.getByRole("tabpanel", { name: "API", exact: true });
      await expect(tab.getByRole("button", { name: "Копировать адрес", exact: true })).toBeInViewport();
      await expect(tab.getByRole("button", { name: "Копировать ключ", exact: true })).toBeInViewport();
      expect(await tab.evaluate((element) => Array.from(element.querySelectorAll("div, form, code, input, button, h3, p")).every((node) => {
        if (!node.getClientRects().length) return true;
        const rect = node.getBoundingClientRect();
        return node.scrollWidth <= node.clientWidth + 1 && rect.left >= 0 && rect.right <= innerWidth;
      }))).toBe(true);
      await expect(tab.getByRole("spinbutton", { name: "Порт", exact: true })).toBeInViewport();
      await page.locator(".gateway-page").screenshot({ path: `output/playwright/gateway-api-${theme}-${width}.png` });
      await tab.locator(".relay-action-menu summary").click();
      await expect(page.getByRole("menuitem", { name: "Перевыпустить API-ключ" })).toBeInViewport();
      const menu = tab.getByRole("menu");
      expect(await menu.evaluate((element) => {
        const rect = element.getBoundingClientRect();
        return rect.left >= 0 && rect.right <= innerWidth;
      })).toBe(true);
      await page.locator(".gateway-page").screenshot({ path: `output/playwright/gateway-api-menu-${theme}-${width}.png` });
    });
  }
}
