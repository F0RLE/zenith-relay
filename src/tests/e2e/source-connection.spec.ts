import { expect, test, type Page } from "../bun-playwright";
import type { SourceConnectionDiagnostics } from "../../src/features/relay/api/types";
import { installTauriMock } from "./tauri-mock";

const diagnostics: SourceConnectionDiagnostics = {
  wireApi: "responses",
  http: { status: "responded", elapsedMs: 12, httpStatus: 200, httpVersion: "HTTP/2.0", websocketUpgraded: null },
  websocket: { status: "responded", elapsedMs: 18, httpStatus: 101, httpVersion: null, websocketUpgraded: true },
};

async function installConnectionCheck(page: Page, result: SourceConnectionDiagnostics, delayed = false) {
  await page.evaluate(({ result, delayed }) => {
    const mock = window as unknown as {
      __TAURI_INTERNALS__: { invoke: (command: string, args?: unknown, options?: unknown) => Promise<unknown> };
      __TAURI_TEST_INVOKES__: Array<{ command: string; args: unknown }>;
      finishConnectionCheck?: () => void;
    };
    const original = mock.__TAURI_INTERNALS__.invoke.bind(mock.__TAURI_INTERNALS__);
    mock.__TAURI_INTERNALS__.invoke = async (command, args, options) => {
      if (command !== "check_local_source_connection") return original(command, args, options);
      mock.__TAURI_TEST_INVOKES__.push({ command, args });
      if (delayed) await new Promise<void>((resolve) => { mock.finishConnectionCheck = resolve; });
      return result;
    };
  }, { result, delayed });
}

async function openSource(page: Page) {
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await page.getByRole("tab", { name: "Sources", exact: true }).click();
  await page.locator(".source-card").first().getByRole("button", { name: "Edit", exact: true }).click();
  return page.getByRole("dialog", { name: "Edit source", exact: true });
}

test("source connection check reports real transport observations without saving settings", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true });
  await page.goto("/");
  await installConnectionCheck(page, diagnostics);
  const dialog = await openSource(page);
  await dialog.getByRole("button", { name: "Check connection", exact: true }).click();
  const result = dialog.locator('p[role="status"]');
  await expect(result).toHaveText("HTTP: 200 · 12 ms; WebSocket: 101 · 18 ms");
  const calls = await page.evaluate(() => (window as unknown as {
    __TAURI_TEST_INVOKES__: Array<{ command: string; args: unknown }>;
  }).__TAURI_TEST_INVOKES__);
  expect(calls.filter(({ command }) => command === "check_local_source_connection")).toHaveLength(1);
  expect(calls.some(({ command }) => command === "update_local_source")).toBe(false);
  const address = dialog.getByLabel("API address", { exact: true });
  await address.fill("https://different.example/v1");
  await expect(dialog.getByRole("button", { name: "Check connection", exact: true })).toBeDisabled();
  await expect(result).toHaveCount(0);
});

test("an old connection result cannot describe a new draft", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true });
  await page.goto("/");
  await installConnectionCheck(page, diagnostics, true);
  const dialog = await openSource(page);
  await dialog.getByRole("button", { name: "Check connection", exact: true }).click();
  await expect.poll(() => page.evaluate(() => Boolean((window as unknown as { finishConnectionCheck?: unknown }).finishConnectionCheck))).toBe(true);
  await dialog.getByLabel("API address", { exact: true }).fill("https://new-draft.example/v1");
  await page.evaluate(() => (window as unknown as { finishConnectionCheck: () => void }).finishConnectionCheck());
  await expect(dialog.getByRole("button", { name: "Check connection", exact: true })).toBeDisabled();
  await expect(dialog.locator('p[role="status"]')).toHaveCount(0);
});

for (const theme of ["light", "dark"] as const) {
  test(`failed WebSocket remains separate from HTTP and fits ${theme} mobile`, async ({ page }, testInfo) => {
    await page.setViewportSize({ width: 390, height: 844 });
    await installTauriMock(page, { mode: "local", locale: "en", theme, populated: true });
    await page.goto("/");
    await installConnectionCheck(page, {
      ...diagnostics,
      websocket: { ...diagnostics.websocket!, httpStatus: 403, websocketUpgraded: false },
    });
    const dialog = await openSource(page);
    await dialog.getByRole("button", { name: "Check connection", exact: true }).click();
    await expect(dialog.locator('p[role="status"]')).toHaveText("HTTP: 200 · 12 ms; WebSocket: API access denied · HTTP 403 · 18 ms");
    expect(await dialog.evaluate((element) => element.scrollWidth <= element.clientWidth + 1)).toBe(true);
    await page.screenshot({ path: testInfo.outputPath("source-connection.png"), fullPage: true });
  });
}
