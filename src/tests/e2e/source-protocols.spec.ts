import { expect, test } from "../bun-playwright";
import { installTauriMock } from "./tauri-mock";

test("unknown catalog stays unrouted until an explicit generation check", async ({ page }) => {
  await installTauriMock(page, { locale: "en", mode: "local", populated: true,
    sourceProtocolConfig: { mode: "auto", revision: 2, capabilities: [] } });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await page.getByRole("tab", { name: "Sources", exact: true }).click();
  await page.getByRole("button", { name: "Edit", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "Edit source" });
  await expect(dialog.locator('.source-protocol-availability [data-available="true"]')).toHaveCount(0);
  const calls = () => page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string }> }).__TAURI_TEST_INVOKES__.filter((call) => call.command === "probe_local_source").length);
  expect(await calls()).toBe(0);
  const probe = dialog.locator(".source-probe-controls button.relay-button");
  await probe.click();
  await expect(dialog.locator('.source-probe-result[data-status="confirmed"]')).toBeVisible();
  await expect(dialog.locator('.source-protocol-availability [data-available="true"]')).toHaveCount(4);
  expect(await calls()).toBe(1);
  await dialog.getByLabel("API address", { exact: true }).fill("https://changed.example.test/v1");
  await expect(probe).toBeDisabled();
  await expect(dialog.locator(".source-probe-result")).toHaveCount(0);
});

test("native Messages source can launch OpenCode while direct ChatGPT is disabled", async ({ page }) => {
  await installTauriMock(page, { locale: "en", populated: true,
    sourceProtocolBindings: [{ wireApi: "messages", adapter: "native", modelIds: ["gpt-5.4", "gpt-5.4-mini"] }] });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await page.getByRole("tab", { name: "Sources", exact: true }).click();
  await page.getByRole("row").filter({ hasText: "Example compatible API" }).getByRole("button", { name: "Launch", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "Where do you want to launch this source?" });
  await expect(dialog.getByRole("button", { name: "ChatGPT", exact: true })).toBeDisabled();
  await dialog.getByRole("button", { name: "OpenCode", exact: true }).click();
  await expect.poll(() => page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string }> }).__TAURI_TEST_INVOKES__.some((call) => call.command === "launch_opencode_source"))).toBe(true);
});

test("adding an unknown source to the pool completes after manual format selection", async ({ page }) => {
  await installTauriMock(page, { locale: "en", mode: "local", populated: false });
  await page.goto("/");
  await page.getByRole("button", { name: "Pool", exact: true }).click();
  await page.getByRole("button", { name: "Add member", exact: true }).first().click();
  await page.getByRole("dialog", { name: "Add connections to pool" }).getByRole("button", { name: "Add API source" }).click();
  const add = page.getByRole("dialog", { name: "Add API source" });
  await add.getByRole("radio", { name: /Custom API/ }).click();
  await add.getByLabel("Name", { exact: true }).fill("Manual API");
  await add.getByLabel("API address").fill("https://manual.example.test/v1");
  await add.getByLabel("Upstream API key").fill("synthetic-key");
  await add.getByRole("button", { name: "Save", exact: true }).click();
  const edit = page.getByRole("dialog", { name: "Edit source" });
  await expect(edit.locator('.source-protocol-availability [data-available="true"]')).toHaveCount(0);
  await edit.locator(".source-add-adapters > summary").click();
  await edit.getByRole("tab", { name: "Manual routing", exact: true }).click();
  await edit.getByRole("button", { name: "Save", exact: true }).click();
  await expect(edit).toBeHidden();
  await expect(page.locator(".pool-member-card").filter({ hasText: "Manual API" })).toBeVisible();
  const calls = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string }> }).__TAURI_TEST_INVOKES__);
  expect(calls.filter((call) => call.command === "set_local_pool_membership")).toHaveLength(1);
  expect(calls.filter((call) => call.command === "probe_local_source")).toHaveLength(0);
});

for (const width of [1160, 390]) {
  test(`model compatibility renders backend routes without inventing capabilities at ${width}px`, async ({ page }) => {
    await installTauriMock(page, { locale: "en", populated: true, modelProtocolRoutes: {
      "gpt-5.4": [{ clientWireApi: "responses", upstreamWireApi: "messages", features: { text: "confirmed", function_tools: "unsupported" }, reasoningEfforts: ["low", "high"] }],
    } });
    await page.setViewportSize({ width, height: 844 });
    await page.goto("/");
    await page.getByRole("button", { name: "Pool", exact: true }).click();
    await page.getByRole("tab", { name: "Model Rules", exact: true }).click();
    await page.getByRole("button", { name: "View formats and features for gpt-5.4", exact: true }).click();
    const dialog = page.getByRole("dialog", { name: "Model compatibility" });
    await expect(dialog.locator('[data-feature="text"]')).toContainText("Verified");
    await expect(dialog.locator('[data-feature="function_tools"]')).toContainText("Unsupported");
    await expect(dialog.locator('[data-feature="streaming"]')).toContainText("Not verified");
    await expect(dialog.locator(".model-protocol-efforts")).toContainText("High");
    expect(await dialog.evaluate((element) => element.scrollWidth <= element.clientWidth)).toBe(true);
    expect(await dialog.getByRole("tablist").evaluate((element) => {
      const rect = element.getBoundingClientRect();
      return [...element.querySelectorAll("button")].every((button) => {
        const bounds = button.getBoundingClientRect();
        return bounds.left >= rect.left && bounds.right <= rect.right && bounds.top >= rect.top && bounds.bottom <= rect.bottom;
      });
    })).toBe(true);
    await page.screenshot({ path: `output/playwright/model-compatibility-${width}.png` });
    await dialog.getByRole("tab", { name: "Gemini", exact: true }).click();
    await expect(dialog.getByText("No available route for this format")).toBeVisible();
  });
}
