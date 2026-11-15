import { expect, test } from "../bun-playwright";
import { installTauriMock } from "./tauri-mock";

type TestWindow = Window & {
  __TAURI_INTERNALS__: { invoke: (command: string, args?: unknown, options?: unknown) => Promise<unknown> };
  __INSTALL_PENDING__?: boolean;
  __FAIL_INSTALL__?: () => void;
  __CHECKS_DURING_INSTALL__?: number;
  __NEW_UPDATE__?: boolean;
  __CHECK_PENDING__?: boolean;
  __RELEASE_CHECK__?: () => void;
};

test("a discovery started before installation cannot hide the installer or leave Check busy", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, updateVersion: "1.1.6" });
  await page.goto("/");
  await page.getByRole("button", { name: "Open update 1.1.6", exact: true }).click();
  await page.evaluate(() => {
    const host = window as TestWindow;
    const invoke = host.__TAURI_INTERNALS__.invoke.bind(host.__TAURI_INTERNALS__);
    let delayNextCheck = true;
    host.__TAURI_INTERNALS__.invoke = (command, args, options) => {
      if (command === "plugin:updater|check" && delayNextCheck) {
        delayNextCheck = false;
        host.__CHECK_PENDING__ = true;
        return new Promise((resolve) => { host.__RELEASE_CHECK__ = () => resolve(null); });
      }
      if (command === "plugin:updater|download_and_install") {
        host.__INSTALL_PENDING__ = true;
        return new Promise((_, reject) => { host.__FAIL_INSTALL__ = () => reject(new Error("Synthetic install stopped")); });
      }
      return invoke(command, args, options);
    };
  });
  await page.evaluate(() => window.dispatchEvent(new Event("focus")));
  await expect.poll(() => page.evaluate(() => (window as TestWindow).__CHECK_PENDING__)).toBe(true);
  const dialog = page.getByRole("dialog", { name: "Update 1.1.6", exact: true });
  await dialog.getByRole("button", { name: "Update", exact: true }).click();
  await expect.poll(() => page.evaluate(() => (window as TestWindow).__INSTALL_PENDING__)).toBe(true);
  await page.evaluate(() => (window as TestWindow).__RELEASE_CHECK__?.());
  await expect(dialog.locator(".update-progress")).toBeVisible();
  await page.evaluate(() => (window as TestWindow).__FAIL_INSTALL__?.());
  await expect(dialog.getByRole("alert")).toBeVisible();
  await dialog.locator("header").getByRole("button", { name: "Close", exact: true }).click();
  await page.getByRole("button", { name: "Settings", exact: true }).click();
  await expect(page.locator(".settings-group").filter({ hasText: "Application" }).getByRole("button", { name: "Check", exact: true })).toBeEnabled();
});

test("foreground update checks cannot replace a version being installed", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, updateVersion: "1.1.6" });
  await page.goto("/");
  await page.getByRole("button", { name: "Open update 1.1.6", exact: true }).click();
  await page.evaluate(() => {
    const host = window as TestWindow;
    const invoke = host.__TAURI_INTERNALS__.invoke.bind(host.__TAURI_INTERNALS__);
    host.__CHECKS_DURING_INSTALL__ = 0;
    host.__TAURI_INTERNALS__.invoke = (command, args, options) => {
      if (command === "plugin:updater|download_and_install") {
        host.__INSTALL_PENDING__ = true;
        return new Promise((_, reject) => { host.__FAIL_INSTALL__ = () => reject(new Error("Synthetic install stopped")); });
      }
      if (command === "plugin:updater|check" && host.__INSTALL_PENDING__) {
        host.__CHECKS_DURING_INSTALL__! += 1;
        return Promise.resolve(null);
      }
      return invoke(command, args, options);
    };
  });
  const dialog = page.getByRole("dialog", { name: "Update 1.1.6", exact: true });
  await dialog.getByRole("button", { name: "Update", exact: true }).click();
  await expect.poll(() => page.evaluate(() => (window as TestWindow).__INSTALL_PENDING__)).toBe(true);
  await page.evaluate(() => window.dispatchEvent(new Event("focus")));
  await page.waitForTimeout(100);
  expect(await page.evaluate(() => (window as TestWindow).__CHECKS_DURING_INSTALL__)).toBe(0);
  await expect(dialog.locator(".update-progress")).toBeVisible();
  await page.evaluate(() => (window as TestWindow).__FAIL_INSTALL__?.());
  await expect(dialog.getByRole("alert")).toBeVisible();
});

test("a new release does not inherit the previous release's install failure", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, updateVersion: "1.1.6" });
  await page.goto("/");
  await page.getByRole("button", { name: "Open update 1.1.6", exact: true }).click();
  await page.evaluate(() => {
    const host = window as TestWindow;
    const invoke = host.__TAURI_INTERNALS__.invoke.bind(host.__TAURI_INTERNALS__);
    host.__TAURI_INTERNALS__.invoke = async (command, args, options) => {
      if (command === "plugin:updater|download_and_install") throw new Error("Synthetic install failure");
      const value = await invoke(command, args, options);
      return command === "plugin:updater|check" && host.__NEW_UPDATE__
        ? { ...(value as Record<string, unknown>), version: "1.1.7" } : value;
    };
  });
  const previous = page.getByRole("dialog", { name: "Update 1.1.6", exact: true });
  await previous.getByRole("button", { name: "Update", exact: true }).click();
  await expect(previous.getByRole("alert")).toBeVisible();
  await previous.locator("header").getByRole("button", { name: "Close", exact: true }).click();
  await page.evaluate(() => { (window as TestWindow).__NEW_UPDATE__ = true; window.dispatchEvent(new Event("focus")); });
  await page.getByRole("button", { name: "Open update 1.1.7", exact: true }).click();
  await expect(page.getByRole("dialog", { name: "Update 1.1.7", exact: true }).getByRole("alert")).toHaveCount(0);
});

for (const blocked of ["get", "set"] as const) {
  test(`unavailable preference ${blocked} cannot block Overview or update controls`, async ({ page }) => {
    await installTauriMock(page, { mode: "local", locale: "en", populated: true, updateVersion: "1.1.6" });
    await page.addInitScript((blocked) => {
      const keys = ["relay.overviewAnalyticsScope", "relay.skippedUpdate"];
      const getItem = Storage.prototype.getItem;
      const setItem = Storage.prototype.setItem;
      Storage.prototype.getItem = function (key) {
        if (blocked === "get" && keys.includes(key)) throw new DOMException("Synthetic storage failure", "SecurityError");
        return getItem.call(this, key);
      };
      Storage.prototype.setItem = function (key, value) {
        if (blocked === "set" && keys.includes(key)) throw new DOMException("Synthetic storage full", "QuotaExceededError");
        setItem.call(this, key, value);
      };
    }, blocked);
    const errors: string[] = [];
    page.on("pageerror", (error) => errors.push(error.message));
    await page.goto("/");
    await expect(page.getByRole("heading", { name: "Overview", exact: true })).toBeVisible();
    await page.getByRole("button", { name: /Analytics connection:/ }).click();
    await page.getByRole("option", { name: "API · Example compatible API", exact: true }).click();
    await page.getByRole("button", { name: "Open update 1.1.6", exact: true }).click();
    await page.getByRole("dialog", { name: "Update 1.1.6", exact: true }).getByRole("button", { name: "Skip 1.1.6", exact: true }).click();
    await expect(page.getByRole("dialog", { name: "Update 1.1.6", exact: true })).toHaveCount(0);
    await expect(page.getByRole("button", { name: "Open update 1.1.6", exact: true })).toHaveCount(0);
    expect(errors).toEqual([]);
  });
}
