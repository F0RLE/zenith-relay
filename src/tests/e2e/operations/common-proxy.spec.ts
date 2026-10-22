import type { Page } from "@playwright/test";
import type { ProxyPoolSummary, RuntimeSnapshot } from "../../../src/features/relay/api/types";
import { expect, test } from "../../bun-playwright";
import { installTauriMock } from "../tauri-mock";

async function openProxies(page: Page) {
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await page.getByRole("tab", { name: "Proxies", exact: true }).click();
}

async function proxyState(page: Page) {
  return page.evaluate(async () => {
    const internals = (window as unknown as {
      __TAURI_INTERNALS__: { invoke: (command: string) => Promise<unknown> };
    }).__TAURI_INTERNALS__;
    return {
      runtime: await internals.invoke("get_local_runtime_state") as RuntimeSnapshot,
      pool: await internals.invoke("get_local_proxy_pool") as ProxyPoolSummary,
    };
  });
}

function storedEntries(pool: ProxyPoolSummary) {
  return pool.entries.map(({ id, endpoint, assignedAccountIds }) => ({ id, endpoint, assignedAccountIds }));
}

for (const required of [false, true]) {
  test(`clearing the common proxy is confirmed and preserves explicit routes with required=${required}`, async ({ page }, testInfo) => {
    if (required) await page.setViewportSize({ width: 840, height: 720 });
    await installTauriMock(page, {
      locale: "en", mode: "local", populated: true, accountCount: 3, proxyCount: 3,
      accountProxyRequired: required, commonProxyId: "proxy_synthetic_2",
    });
    await openProxies(page);
    const clear = page.getByRole("button", { name: "Clear common proxy", exact: true });
    await expect(clear).toBeVisible();
    const before = await proxyState(page);
    const explicit = before.runtime.accounts.filter(account => account.proxyMode === "account");
    const inherited = before.runtime.accounts.filter(account => account.proxyMode === "common");
    expect(explicit.length).toBeGreaterThan(0);
    expect(inherited.length).toBeGreaterThan(0);

    const common = page.locator(".proxy-storage-row").filter({ hasText: "proxy-2.example.test" });
    await common.locator("summary").click();
    await common.getByRole("menuitem", { name: "Delete", exact: true }).click();
    await expect(page.locator(".proxy-storage-error")).toContainText("clear the common proxy here");

    await clear.click();
    const confirmation = page.getByRole("dialog", { name: "Confirm action" });
    await expect(confirmation).toContainText("direct access is blocked");
    await confirmation.getByRole("button", { name: "Cancel", exact: true }).click();
    await expect(confirmation).toHaveCount(0);
    expect((await proxyState(page)).runtime.gateway.commonProxyConfigured).toBe(true);
    expect(await page.evaluate(() => (window as unknown as {
      __TAURI_TEST_INVOKES__: Array<{ command: string }>;
    }).__TAURI_TEST_INVOKES__.filter(call => call.command === "set_local_common_proxy"))).toEqual([]);

    await clear.click();
    await expect(confirmation).toBeVisible();
    await page.screenshot({ path: testInfo.outputPath("common-proxy-confirm.png") });
    await confirmation.getByRole("button", { name: "Clear common proxy", exact: true }).click();
    await expect(confirmation).toHaveCount(0);
    await expect(clear).toHaveCount(0);
    await expect(page.locator(".proxy-storage-error")).toHaveCount(0);
    await expect(page.locator(".proxy-storage-row")).toHaveCount(3);
    const after = await proxyState(page);
    expect(after.runtime.gateway.commonProxyConfigured).toBe(false);
    expect(after.runtime.gateway.commonProxyId).toBeNull();
    expect(after.runtime.gateway.accountProxyRequired).toBe(required);
    expect(storedEntries(after.pool)).toEqual(storedEntries(before.pool));
    for (const account of explicit) {
      const current = after.runtime.accounts.find(candidate => candidate.id === account.id)!;
      expect([current.proxyMode, current.proxyId, current.proxyAvailable])
        .toEqual([account.proxyMode, account.proxyId, account.proxyAvailable]);
    }
    for (const account of inherited) {
      expect(after.runtime.accounts.find(candidate => candidate.id === account.id)?.proxyAvailable).toBe(!required);
    }
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    const changes = await page.evaluate(() => (window as unknown as {
      __TAURI_TEST_INVOKES__: Array<{ command: string; args: unknown }>;
    }).__TAURI_TEST_INVOKES__.filter(call => call.command === "set_local_common_proxy"));
    expect(changes).toEqual([{ command: "set_local_common_proxy", args: { input: { proxyUrl: null } } }]);

    await common.locator("summary").click();
    await common.getByRole("menuitem", { name: "Delete", exact: true }).click();
    await confirmation.getByRole("button", { name: "Confirm", exact: true }).click();
    await expect(common).toHaveCount(0);
    await expect(page.locator(".proxy-storage-row")).toHaveCount(2);
  });
}

for (const configured of [false, true]) {
  test(`common proxy recovery is available without stored entries only when configured=${configured}`, async ({ page }) => {
    await installTauriMock(page, { locale: "en", mode: "local", populated: false, proxyCount: 0, commonProxyConfigured: configured });
    await openProxies(page);
    await expect(page.getByText("Proxy storage is empty", { exact: true })).toBeVisible();
    const clear = page.getByRole("button", { name: "Clear common proxy", exact: true });
    await expect(clear).toHaveCount(configured ? 1 : 0);
    if (!configured) return;
    await clear.click();
    await page.getByRole("dialog", { name: "Confirm action" }).getByRole("button", { name: "Clear common proxy", exact: true }).click();
    await expect(clear).toHaveCount(0);
    expect((await proxyState(page)).pool.entries).toEqual([]);
  });
}

test("a rejected common proxy change keeps the saved configuration and entries", async ({ page }) => {
  await installTauriMock(page, { locale: "en", mode: "local", populated: true, proxyCount: 1 });
  await openProxies(page);
  const before = await proxyState(page);
  await page.evaluate(() => {
    const internals = (window as unknown as {
      __TAURI_INTERNALS__: { invoke: (command: string, ...args: unknown[]) => Promise<unknown> };
    }).__TAURI_INTERNALS__;
    const invoke = internals.invoke.bind(internals);
    internals.invoke = (command, ...args) => command === "set_local_common_proxy"
      ? Promise.reject({ code: "io", message: "Synthetic common proxy write failed" })
      : invoke(command, ...args);
  });
  const clear = page.getByRole("button", { name: "Clear common proxy", exact: true });
  await clear.click();
  await page.getByRole("dialog", { name: "Confirm action" }).getByRole("button", { name: "Clear common proxy", exact: true }).click();
  const feedback = page.locator(".global-feedback.error");
  await expect(feedback).toBeVisible();
  await feedback.getByRole("button", { name: "Show details", exact: true }).click();
  const details = page.getByRole("dialog", { name: "Error details" });
  await expect(details.locator("pre")).toContainText("Synthetic common proxy write failed");
  await details.locator("footer").getByRole("button", { name: "Close", exact: true }).click();
  await expect(clear).toBeEnabled();
  const after = await proxyState(page);
  expect(after.runtime.gateway.commonProxyConfigured).toBe(true);
  expect(storedEntries(after.pool)).toEqual(storedEntries(before.pool));
});

test("the common proxy control and confirmation fit the compact Russian layout", async ({ page }, testInfo) => {
  await page.setViewportSize({ width: 840, height: 560 });
  await installTauriMock(page, { locale: "ru", mode: "local", populated: true, proxyCount: 3 });
  await page.goto("/");
  await page.getByRole("button", { name: "Подключения", exact: true }).click();
  await page.getByRole("tab", { name: "Прокси", exact: true }).click();
  const clear = page.getByRole("button", { name: "Сбросить общий прокси", exact: true });
  await expect(clear).toBeVisible();
  expect(await clear.evaluate(element => {
    const bounds = element.getBoundingClientRect();
    return bounds.left >= 0 && bounds.right <= innerWidth && element.scrollWidth <= element.clientWidth;
  })).toBe(true);
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  await page.screenshot({ path: testInfo.outputPath("common-proxy-ru-compact.png") });
  await clear.click();
  const confirmation = page.getByRole("dialog");
  await expect(confirmation).toContainText("если он разрешён");
  await expect(confirmation.getByRole("button", { name: "Сбросить общий прокси", exact: true })).toBeInViewport();
  await confirmation.getByRole("button", { name: "Отмена", exact: true }).click();
  expect((await proxyState(page)).runtime.gateway.commonProxyConfigured).toBe(true);
});
