import { expect, test, type Page } from "../../bun-playwright";
import type { LocalNetworkAccess } from "../../../src/features/relay/api/commands";
import { installTauriMock } from "../tauri-mock";
import { openGatewayApi } from "./helpers";

type NetworkWindow = Window & {
  __NETWORK_TEST__: { next: LocalNetworkAccess; fail: boolean; hold: boolean; reads: number; release?: () => void };
  __TAURI_INTERNALS__: { invoke: (command: string, args?: unknown, options?: unknown) => Promise<unknown> };
};

async function openNetwork(page: Page) {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, gatewayRunning: false });
  await page.clock.install();
  await page.goto("/");
  await page.evaluate(() => {
    const target = window as NetworkWindow;
    const state = target.__NETWORK_TEST__ = { next: { enabled: false, addresses: [] }, fail: false, hold: false, reads: 0 } as NetworkWindow["__NETWORK_TEST__"];
    const invoke = target.__TAURI_INTERNALS__.invoke.bind(target.__TAURI_INTERNALS__);
    target.__TAURI_INTERNALS__.invoke = async (command, args, options) => {
      if (command === "get_local_network_access") {
        state.reads++;
        const next = structuredClone(state.next);
        if (state.hold) await new Promise<void>((resolve) => { state.release = resolve; });
        if (state.fail) throw new Error("Synthetic network read failure");
        return next;
      }
      const result = await invoke(command, args, options);
      if (command === "update_local_network_access") state.next = result as LocalNetworkAccess;
      return result;
    };
  });
  await openGatewayApi(page);
  await expect(page.locator(".gateway-network-access input")).toBeEnabled();
}

test("network addresses refresh on focus, reconnect and visible polling without restarting API", async ({ page }) => {
  await openNetwork(page);
  const network = page.locator(".gateway-network-access");
  await page.evaluate(() => {
    (window as NetworkWindow).__NETWORK_TEST__.next = { enabled: true, addresses: [
      { interface: "Ethernet", baseUrl: "http://192.168.1.20:14998/v1" },
      { interface: "VPN", baseUrl: "http://10.8.0.2:14998/v1" },
    ] };
    window.dispatchEvent(new Event("focus"));
  });
  await expect(network.locator("code")).toHaveText(["http://192.168.1.20:14998/v1", "http://10.8.0.2:14998/v1"]);
  await expect(network.locator("small")).toHaveText(["Ethernet", "VPN"]);
  await network.locator(".gateway-api-field").first().getByRole("button").nth(1).click();
  await expect.poll(() => page.evaluate(() => navigator.clipboard.readText())).toBe("http://10.8.0.2:14998/v1");
  await page.setViewportSize({ width: 390, height: 844 });
  expect(await network.evaluate((node) => node.scrollWidth <= node.clientWidth)).toBe(true);
  await network.screenshot({ path: "output/playwright/network-interfaces-mobile.png" });

  await page.evaluate(() => {
    (window as NetworkWindow).__NETWORK_TEST__.next.addresses = [];
    window.dispatchEvent(new Event("online"));
  });
  await expect(network).toContainText("No private IPv4 address detected");
  await page.evaluate(() => {
    (window as NetworkWindow).__NETWORK_TEST__.next.addresses = [{ interface: "Wi-Fi", baseUrl: "http://192.168.2.20:14998/v1" }];
  });
  await page.clock.fastForward(15_000);
  await expect(network.locator("code")).toHaveText("http://192.168.2.20:14998/v1");
  const reads = await page.evaluate(() => {
    Object.defineProperty(document, "hidden", { configurable: true, value: true });
    return (window as NetworkWindow).__NETWORK_TEST__.reads;
  });
  await page.clock.fastForward(30_000);
  expect(await page.evaluate(() => (window as NetworkWindow).__NETWORK_TEST__.reads)).toBe(reads);
  await page.evaluate(() => {
    Object.defineProperty(document, "hidden", { configurable: true, value: false });
    (window as NetworkWindow).__NETWORK_TEST__.next.addresses = [];
    document.dispatchEvent(new Event("visibilitychange"));
  });
  await expect(network).toContainText("No private IPv4 address detected");
  const mutations = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string }> }).__TAURI_TEST_INVOKES__
    .filter(({ command }) => ["update_local_network_access", "start_local_gateway", "stop_local_gateway"].includes(command)));
  expect(mutations).toEqual([]);
});

test("network refresh recovers from read failure and never overlaps a pending read", async ({ page }) => {
  await openNetwork(page);
  const network = page.locator(".gateway-network-access");
  await page.evaluate(() => {
    (window as NetworkWindow).__NETWORK_TEST__.hold = true;
    window.dispatchEvent(new Event("focus"));
  });
  const reads = await page.evaluate(() => (window as NetworkWindow).__NETWORK_TEST__.reads);
  await page.clock.fastForward(45_000);
  await page.evaluate(() => { window.dispatchEvent(new Event("online")); });
  expect(await page.evaluate(() => (window as NetworkWindow).__NETWORK_TEST__.reads)).toBe(reads);
  await page.evaluate(() => {
    const state = (window as NetworkWindow).__NETWORK_TEST__;
    state.fail = true;
    state.hold = false;
    state.release!();
  });
  await expect(network).toContainText("Could not read network settings");
  await expect(network.getByRole("checkbox")).toBeDisabled();
  await page.evaluate(() => {
    (window as NetworkWindow).__NETWORK_TEST__.fail = false;
    window.dispatchEvent(new Event("online"));
  });
  await expect(network.getByRole("checkbox")).toBeEnabled();
  await expect(network).not.toContainText("Could not read network settings");
});

test("stale network read cannot overwrite a successful sharing change", async ({ page }) => {
  await openNetwork(page);
  const sharing = page.locator(".gateway-network-access").getByRole("checkbox");
  await page.evaluate(() => {
    (window as NetworkWindow).__NETWORK_TEST__.hold = true;
    window.dispatchEvent(new Event("focus"));
  });
  await sharing.click();
  await page.evaluate(() => { (window as NetworkWindow).__NETWORK_TEST__.hold = false; });
  await page.getByRole("dialog").getByRole("button", { name: "Enable", exact: true }).click();
  await expect(sharing).toBeChecked();
  await page.evaluate(() => { (window as NetworkWindow).__NETWORK_TEST__.release!(); });
  await expect(sharing).toBeChecked();
  await expect(page.locator(".gateway-network-details code")).toHaveText("http://192.168.1.10:14998/v1");
});
