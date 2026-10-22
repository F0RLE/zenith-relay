import { expect, test, type Page } from "../../bun-playwright";
import { installTauriMock } from "../tauri-mock";
import { openGatewayApi } from "./helpers";

const calls = (page: Page) => page.evaluate(() => (window as unknown as {
  __TAURI_TEST_INVOKES__: Array<{ command: string; args: Record<string, unknown> }>;
}).__TAURI_TEST_INVOKES__);

for (const locale of ["en", "ru"] as const) {
  test(`${locale} mode picker separates Hosted from the computer without stopping its API`, async ({ page }) => {
    await installTauriMock(page, { mode: "local", locale, populated: true, gatewayRunning: true });
    await page.goto("/");
    const computer = locale === "ru" ? "На устройстве" : "Computer";
    const hosted = locale === "ru" ? "На сервере" : "Relay Hosted";
    await page.locator(".mode-picker-trigger").click();
    await expect(page.getByRole("menuitemradio")).toHaveText([computer, hosted]);
    await page.getByRole("menuitemradio", { name: hosted, exact: true }).click();
    await expect(page.getByRole("heading", { name: hosted, exact: true })).toBeVisible();
    await expect(page.getByText(locale === "ru" ? "В разработке" : "In development", { exact: true })).toBeVisible();
    await expect(page.locator(".sidebar-main-nav button")).toHaveCount(0);
    await expect(page.locator(".sidebar-nav-utilities button")).toHaveCount(0);
    await expect.poll(() => page.evaluate(() => localStorage.getItem("relay.mode"))).toBe("hosted");
    expect((await calls(page)).some(({ command }) => command === "stop_local_gateway" || command.startsWith("get_remote_") || command === "execute_remote_server_action")).toBe(false);
    await page.screenshot({ path: `output/playwright/hosted-${locale}.png` });

    await page.getByRole("button", { name: locale === "ru" ? "Использовать это устройство" : "Use this computer", exact: true }).click();
    await expect(page.getByRole("button", { name: locale === "ru" ? "Остановить API" : "Stop API", exact: true })).toBeVisible();
  });
}

test("Hosted startup does not load either runtime", async ({ page }) => {
  await installTauriMock(page, { mode: "hosted", locale: "en", populated: true });
  await page.goto("/");
  await expect(page.getByText("In development", { exact: true })).toBeVisible();
  await expect(page.getByRole("button", { name: "Open Hosted demo", exact: true })).toHaveCount(0);
  expect((await calls(page)).some(({ command }) => command === "get_local_runtime_state" || command.startsWith("get_remote_"))).toBe(false);
});

test("network sharing is opt-in, confirmed, and has a separate key", async ({ page }) => {
  await page.context().grantPermissions(["clipboard-read", "clipboard-write"], { origin: "http://127.0.0.1:1420" });
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, gatewayRunning: false });
  await page.goto("/");
  await openGatewayApi(page);
  const sharing = page.getByRole("checkbox", { name: "Allow local network access", exact: true });
  await expect(sharing).not.toBeChecked();
  await expect(sharing).toBeEnabled();
  const portSettings = await page.locator(".gateway-api-settings").boundingBox();
  const networkSettings = await page.locator(".gateway-network-access").boundingBox();
  expect(portSettings && networkSettings && networkSettings.y >= portSettings.y + portSettings.height).toBeTruthy();
  const localAddress = await page.locator(".gateway-api-endpoint code").innerText();
  await page.getByRole("button", { name: "Copy key", exact: true }).click();
  const localKey = await page.evaluate(() => navigator.clipboard.readText());
  await sharing.click();
  const dialog = page.getByRole("dialog", { name: "Allow local network access", exact: true });
  await expect(dialog).toContainText("HTTP traffic is not encrypted");
  await dialog.getByRole("button", { name: "Cancel", exact: true }).click();
  await expect(sharing).not.toBeChecked();
  expect((await calls(page)).filter(({ command }) => command === "update_local_network_access")).toHaveLength(0);

  await sharing.click();
  await dialog.getByRole("button", { name: "Enable", exact: true }).click();
  await expect(sharing).toBeChecked();
  await expect(page.locator(".gateway-network-details code")).toHaveText("http://192.168.1.10:14998/v1");
  await expect(page.locator(".gateway-api-endpoint code")).toHaveText(localAddress);
  await page.getByRole("button", { name: "Copy network API key", exact: true }).click();
  await expect.poll(() => page.evaluate(() => navigator.clipboard.readText())).not.toBe(localKey);
  await page.locator(".gateway-network-details summary").click();
  await page.getByRole("menuitem", { name: "Reissue API key", exact: true }).click();
  await page.getByRole("dialog", { name: "Network API key", exact: true }).getByRole("button", { name: "Reissue API key", exact: true }).click();
  await expect.poll(async () => (await calls(page)).filter(({ command }) => command === "rotate_local_gateway_api_key").length).toBe(1);
  await page.getByRole("button", { name: "Copy key", exact: true }).click();
  await expect.poll(() => page.evaluate(() => navigator.clipboard.readText())).toBe(localKey);
  const fields = await page.locator(".gateway-network-details .gateway-api-value").evaluateAll((nodes) => nodes.map((node) => {
    const { top, bottom, left, right } = node.getBoundingClientRect();
    return { top, bottom, left, right };
  }));
  expect(fields).toHaveLength(2);
  expect(fields[0].right <= fields[1].left || fields[0].bottom <= fields[1].top).toBe(true);
  await page.screenshot({ path: "output/playwright/computer-network-api.png" });

  await page.setViewportSize({ width: 390, height: 844 });
  expect(await page.locator(".gateway-network-details").evaluate((node) => node.scrollWidth <= node.clientWidth)).toBe(true);

  await sharing.click();
  await expect(sharing).not.toBeChecked();
  const mutations = (await calls(page)).filter(({ command }) => command === "update_local_network_access");
  expect(mutations.map(({ args }) => args)).toEqual([
    { enabled: true, expectedEnabled: false }, { enabled: false, expectedEnabled: true },
  ]);
  expect((await calls(page)).find(({ command }) => command === "rotate_local_gateway_api_key")?.args).toEqual({ network: true });
});

test("failed network setting save never displays sharing as enabled", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true });
  await page.goto("/");
  await openGatewayApi(page);
  await page.evaluate(() => {
    const internals = (window as unknown as { __TAURI_INTERNALS__: { invoke: (command: string, args?: unknown, options?: unknown) => Promise<unknown> } }).__TAURI_INTERNALS__;
    const invoke = internals.invoke.bind(internals);
    internals.invoke = (command, args, options) => command === "update_local_network_access"
      ? Promise.reject({ code: "gateway_unavailable", message: "Synthetic bind failure" })
      : invoke(command, args, options);
  });
  const sharing = page.getByRole("checkbox", { name: "Allow local network access", exact: true });
  await sharing.click();
  await page.getByRole("dialog", { name: "Allow local network access", exact: true }).getByRole("button", { name: "Enable", exact: true }).click();
  await expect(page.locator(".global-feedback.error")).toBeVisible();
  await expect(sharing).not.toBeChecked();
  await expect(page.locator(".gateway-network-details")).toHaveCount(0);
});

test("connection panel keeps port validation and start stop actions", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, gatewayRunning: false });
  await page.goto("/");
  await openGatewayApi(page);
  const port = page.getByRole("spinbutton", { name: "Port", exact: true });
  const save = page.getByRole("button", { name: "Save", exact: true });
  await expect(save).toBeDisabled();
  for (const invalid of ["", "1023", "65536", "14998.5"]) {
    await port.fill(invalid);
    await expect(save).toBeDisabled();
  }
  await port.fill("15432");
  await expect(save).toBeEnabled();
  await port.press("Enter");
  await expect(page.locator(".gateway-api-endpoint code")).toHaveText("http://127.0.0.1:15432/v1");
  await expect(save).toBeDisabled();
  expect((await calls(page)).filter(({ command }) => command === "update_local_gateway_port").map(({ args }) => args)).toEqual([{ port: 15432 }]);

  await page.getByRole("checkbox", { name: "Allow local network access", exact: true }).click();
  await page.getByRole("dialog").getByRole("button", { name: "Enable", exact: true }).click();
  await expect(page.locator(".gateway-network-details code")).toHaveText("http://192.168.1.10:15432/v1");
  await port.fill("15433");
  await port.press("Enter");
  await expect(page.locator(".gateway-network-details code")).toHaveText("http://192.168.1.10:15433/v1");

  const controls = page.locator(".gateway-api-controls");
  await controls.getByRole("button", { name: "Start API", exact: true }).click();
  await expect(page.locator(".gateway-api-status")).toContainText("API is running");
  await controls.getByRole("button", { name: "Stop API", exact: true }).click();
  await expect(controls.getByRole("button", { name: "Start API", exact: true })).toBeVisible();
  expect((await calls(page)).filter(({ command }) => command === "start_local_gateway" || command === "stop_local_gateway").map(({ command }) => command)).toEqual(["start_local_gateway", "stop_local_gateway"]);
});

for (const state of ["load-failed", "address-unavailable"] as const) {
  test(`network connection panel handles ${state}`, async ({ page }) => {
    await installTauriMock(page, { mode: "local", locale: "en", populated: true });
    await page.goto("/");
    await page.evaluate((state) => {
      const internals = (window as unknown as { __TAURI_INTERNALS__: { invoke: (command: string, args?: unknown, options?: unknown) => Promise<unknown> } }).__TAURI_INTERNALS__;
      const invoke = internals.invoke.bind(internals);
      internals.invoke = (command, args, options) => command === "get_local_network_access"
        ? state === "load-failed" ? Promise.reject(new Error("Synthetic network read failure")) : Promise.resolve({ enabled: true, addresses: [] })
        : invoke(command, args, options);
    }, state);
    await openGatewayApi(page);
    const network = page.locator(".gateway-network-access");
    const sharing = network.getByRole("checkbox");
    if (state === "load-failed") {
      await expect(network).toContainText("Could not read network settings");
      await expect(sharing).toBeDisabled();
      await expect(sharing).not.toBeChecked();
      await expect(network.locator(".gateway-network-details")).toHaveCount(0);
    } else {
      await expect(network).toContainText("No private IPv4 address detected");
      await expect(sharing).toBeChecked();
      await expect(network.locator(".gateway-api-field").first().getByRole("button")).toHaveCount(0);
      await expect(network.getByRole("button", { name: "Copy network API key", exact: true })).toBeEnabled();
    }
  });
}
