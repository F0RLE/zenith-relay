import { expect, test, type Locator, type Page } from "../bun-playwright";
import { installTauriMock, type MockOptions } from "./tauri-mock";

async function openConnections(page: Page) {
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await expect(page.locator("#splash-screen")).toHaveCount(0);
}

async function expectControlsFit(controls: Locator) {
  expect(await controls.evaluate((element) => {
    const panel = element.getBoundingClientRect();
    const boxes = [...element.querySelectorAll<HTMLElement>("input, button, summary")]
      .filter((item) => item.getClientRects().length)
      .map((item) => item.getBoundingClientRect());
    return panel.left >= 0 && panel.right <= innerWidth
      && element.scrollWidth <= element.clientWidth + 1
      && boxes.every((box) => box.width > 0 && box.left >= panel.left && box.right <= panel.right
        && box.top >= panel.top && box.bottom <= panel.bottom)
      && boxes.every((box, index) => boxes.slice(index + 1).every((other) =>
        box.right <= other.left || other.right <= box.left || box.bottom <= other.top || other.bottom <= box.top));
  })).toBe(true);
}

async function calls(page: Page, command: string) {
  return page.evaluate((name) => (window as unknown as {
    __TAURI_TEST_INVOKES__: Array<{ command: string; args: Record<string, unknown> }>;
  }).__TAURI_TEST_INVOKES__.filter((call) => call.command === name), command);
}

for (const locale of ["ru", "en"] as const) {
  for (const theme of ["dark", "light"] as const) {
    for (const width of [1160, 840, 390]) {
      test(`connection toolbars fit and retain summaries ${locale} ${theme} ${width}`, async ({ page }) => {
        const russian = locale === "ru";
        await installTauriMock(page, { locale, theme, populated: true, sourceCount: 3, proxyCount: 3, accountCount: 3 });
        await page.setViewportSize({ width, height: 844 });
        await page.goto("/");
        await page.getByRole("button", { name: russian ? "Подключения" : "Connections", exact: true }).click();
        await expect(page.locator("#splash-screen")).toHaveCount(0);
        for (const view of ["sources", "proxies"] as const) {
          const source = view === "sources";
          await page.getByRole("tab", { name: source ? russian ? "Источники API" : "Sources" : russian ? "Прокси" : "Proxies", exact: true }).click();
          const controls = page.locator(source ? ".sources-controls" : ".proxy-controls");
          const counts = controls.locator(".relay-status-summary strong");
          await expect(counts).toHaveText(source ? ["3", "3", "0", "0"] : ["3", "2", "1"]);
          await expectControlsFit(controls);
          await page.mouse.move(1, 1);
          await page.screenshot({ path: `output/playwright/toolbar-${view}-${locale}-${theme}-${width}.png`, animations: "disabled" });
          const cards = page.locator(source ? ".source-card" : ".proxy-storage-row");
          await cards.first().locator(".account-select-button").click();
          await expect(controls.getByRole("checkbox")).toHaveJSProperty("indeterminate", true);
          await expect(counts).toHaveText(source ? ["3", "3", "0", "0"] : ["3", "2", "1"]);
          await expectControlsFit(controls);
          if (width === 390) {
            await page.mouse.move(1, 1);
            await page.screenshot({ path: `output/playwright/toolbar-${view}-selected-${locale}-${theme}-${width}.png`, animations: "disabled" });
          }
          await controls.getByRole("button", { name: russian ? "Снять выделение" : "Clear selection", exact: true }).click();
          await expect(controls.getByRole("checkbox")).not.toBeChecked();
          await expect(controls.getByRole("checkbox")).toHaveJSProperty("indeterminate", false);
        }
      });
    }
  }
}

for (const state of [
  { options: { poolMembers: false }, counts: ["3", "0", "0", "0"] },
  { options: { sourceEnabled: false }, counts: ["3", "3", "0", "3"] },
  { options: { sourceSecretAvailable: false }, counts: ["3", "3", "3", "0"] },
  { options: { sourceOperationalStatus: "unavailable" }, counts: ["3", "3", "3", "0"] },
  { options: { sourceErrorCode: "HTTP 401 synthetic failure" }, counts: ["3", "3", "3", "0"] },
] satisfies Array<{ options: MockOptions; counts: string[] }>) {
  test(`source summary follows card state ${JSON.stringify(state.options)}`, async ({ page }) => {
    await installTauriMock(page, { populated: true, sourceCount: 3, ...state.options });
    await openConnections(page);
    await page.getByRole("tab", { name: "Sources", exact: true }).click();
    const controls = page.locator(".sources-controls");
    await expect(controls.locator(".relay-status-summary strong")).toHaveText(state.counts);
    await controls.getByRole("textbox", { name: "Search", exact: true }).fill("no-synthetic-match");
    await expect(page.locator(".source-card")).toHaveCount(0);
    await expect(page.getByText("No matching results", { exact: true })).toBeVisible();
    await expect(controls.locator(".relay-status-summary strong")).toHaveText(state.counts);
    await expect(controls.getByRole("checkbox")).toBeDisabled();
  });
}

test("source toolbar searches, sorts, updates pool counts and refreshes the complete inventory", async ({ page }) => {
  await installTauriMock(page, { populated: true, sourceCount: 3 });
  await openConnections(page);
  await page.getByRole("tab", { name: "Sources", exact: true }).click();
  const controls = page.locator(".sources-controls");
  const cards = page.locator(".source-card");
  const search = controls.getByRole("textbox", { name: "Search", exact: true });
  await controls.getByRole("button", { name: "Order: Pool order", exact: true }).click();
  await page.getByRole("option", { name: "Name", exact: true }).click();
  await expect(cards.first()).toContainText("Backup API 1");
  await controls.getByRole("button", { name: "Sort Order descending", exact: true }).click();
  await expect(cards.first()).toContainText("Example compatible API");
  await search.fill("Backup API 2");
  await expect(cards).toHaveCount(1);
  await controls.getByRole("checkbox").check();
  await controls.getByRole("button", { name: "Remove from pool", exact: true }).click();
  await expect(cards.first()).toHaveAttribute("data-in-pool", "false");
  await expect(controls.locator(".relay-status-summary strong")).toHaveText(["3", "2", "0", "0"]);
  await controls.getByRole("button", { name: "Clear selection", exact: true }).click();
  await controls.getByRole("button", { name: "Refresh API data", exact: true }).click();
  await expect.poll(async () => (await calls(page, "test_local_source")).length).toBe(3);
  await expect(page.locator(".connection-toolbar-result")).toBeVisible();
  await expect(cards).toHaveCount(1);
  await search.fill("");
  await expect(cards).toHaveCount(3);
});

test("proxy toolbar preserves hidden selections, refreshes and deletes only selected proxies", async ({ page }) => {
  await installTauriMock(page, { populated: true, proxyCount: 3, accountCount: 1, commonProxyConfigured: false });
  await openConnections(page);
  await page.getByRole("tab", { name: "Proxies", exact: true }).click();
  const controls = page.locator(".proxy-controls");
  const cards = page.locator(".proxy-storage-row");
  const search = controls.getByRole("textbox", { name: "Search", exact: true });
  await expect(controls.locator(".relay-status-summary strong")).toHaveText(["3", "3", "0"]);
  await cards.first().locator(".account-select-button").click();
  await search.fill("proxy-2.");
  await expect(cards).toHaveCount(1);
  await controls.getByRole("checkbox").check();
  await expect(controls).toContainText("Selected: 2");
  await controls.getByRole("checkbox").uncheck();
  await expect(controls).toContainText("Selected: 1");
  await search.fill("no-synthetic-match");
  await expect(controls.getByRole("checkbox")).toBeDisabled();
  await expect(controls.locator(".relay-status-summary strong")).toHaveText(["3", "3", "0"]);
  await search.fill("");
  await expect(controls.getByRole("checkbox")).toHaveJSProperty("indeterminate", true);
  const before = (await calls(page, "get_local_proxy_pool")).length;
  await controls.getByRole("button", { name: "Refresh", exact: true }).click();
  await expect.poll(async () => (await calls(page, "get_local_proxy_pool")).length).toBeGreaterThan(before);
  await expect(controls).toContainText("Selected: 1");
  await controls.getByRole("button", { name: "Delete", exact: true }).click();
  await page.getByRole("dialog", { name: "Confirm action" }).getByRole("button", { name: "Confirm", exact: true }).click();
  await expect(cards).toHaveCount(2);
  await expect(controls.locator(".relay-status-summary strong")).toHaveText(["2", "2", "0"]);
  expect((await calls(page, "delete_local_stored_proxy")).at(-1)?.args).toEqual({ proxyId: "proxy_synthetic_1" });
});

test("source refresh disables duplicate actions and reports partial failures", async ({ page }) => {
  await installTauriMock(page, { populated: true, sourceCount: 3 });
  await openConnections(page);
  await page.getByRole("tab", { name: "Sources", exact: true }).click();
  await page.evaluate(() => {
    const internals = (window as unknown as { __TAURI_INTERNALS__: { invoke: (command: string, args?: Record<string, unknown>, options?: unknown) => Promise<unknown> } }).__TAURI_INTERNALS__;
    const invoke = internals.invoke.bind(internals);
    let held = false;
    internals.invoke = async (command, args, options) => {
      if (command === "test_local_source") {
        if (!held) {
          held = true;
          await new Promise<void>((resolve) => window.addEventListener("release-source-refresh", () => resolve(), { once: true }));
        }
        if (args?.sourceId === "source_synthetic_2") throw new Error("synthetic refresh failure");
      }
      return invoke(command, args, options);
    };
  });
  const controls = page.locator(".sources-controls");
  const refresh = controls.getByRole("button", { name: "Refresh API data", exact: true });
  await refresh.click();
  await expect(refresh).toBeDisabled();
  await expect(controls.getByRole("checkbox")).toBeDisabled();
  await page.evaluate(() => window.dispatchEvent(new Event("release-source-refresh")));
  await expect(page.locator(".connection-toolbar-result")).toHaveText("Updated: 2 · Errors: 1 · Skipped: 0");
  await expect(refresh).toBeEnabled();
  await expect(page.locator(".source-card")).toHaveCount(3);
});

test("proxy storage shows loading, a recoverable error, and a working retry", async ({ page }) => {
  await installTauriMock(page, { populated: true, proxyCount: 3 });
  await openConnections(page);
  await page.evaluate(() => {
    const internals = (window as unknown as { __TAURI_INTERNALS__: { invoke: (command: string, args?: unknown, options?: unknown) => Promise<unknown> } }).__TAURI_INTERNALS__;
    const invoke = internals.invoke.bind(internals);
    let held = false;
    internals.invoke = async (command, args, options) => {
      if (command === "get_local_proxy_pool" && !held) {
        held = true;
        await new Promise<void>((resolve) => window.addEventListener("release-proxy-pool", () => resolve(), { once: true }));
        throw new Error("synthetic storage read failure");
      }
      return invoke(command, args, options);
    };
  });
  await page.getByRole("tab", { name: "Proxies", exact: true }).click();
  await expect(page.locator(".center-loading")).toBeVisible();
  await expect(page.locator(".proxy-controls")).toHaveCount(0);
  await page.evaluate(() => window.dispatchEvent(new Event("release-proxy-pool")));
  await expect(page.getByText("Proxy storage is unavailable", { exact: true })).toBeVisible();
  await page.getByRole("button", { name: "Retry", exact: true }).click();
  await expect(page.locator(".proxy-storage-row")).toHaveCount(3);
  await expectControlsFit(page.locator(".proxy-controls"));
  expect((await calls(page, "delete_local_stored_proxy")).length).toBe(0);
});

test("empty connection views keep their actions without empty toolbars", async ({ page }) => {
  await installTauriMock(page, { populated: false });
  await openConnections(page);
  await page.getByRole("tab", { name: "Sources", exact: true }).click();
  await expect(page.getByText("No API sources", { exact: true })).toBeVisible();
  await expect(page.locator(".sources-controls")).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Add source", exact: true })).toBeVisible();
  await page.getByRole("tab", { name: "Proxies", exact: true }).click();
  await expect(page.getByText("Proxy storage is empty", { exact: true })).toBeVisible();
  await expect(page.locator(".proxy-controls")).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Import", exact: true }).first()).toBeVisible();
});
