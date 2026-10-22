import { expect, test } from "../bun-playwright";
import { installTauriMock } from "./tauri-mock";

test("source editor keeps discovery and routing controls internal", async ({ page }) => {
  await installTauriMock(page, { locale: "en", mode: "local", populated: true,
    sourceProtocolConfig: { revision: 2, capabilities: [] } });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await page.getByRole("tab", { name: "Sources", exact: true }).click();
  await page.getByRole("button", { name: "Edit", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "Edit source" });
  await expect(dialog.getByRole("tab", { name: "General", exact: true })).toBeVisible();
  await expect(dialog.locator(".source-protocol-availability")).toHaveCount(0);
  await expect(dialog.locator(".source-probe-controls")).toHaveCount(0);
  await expect(dialog.locator(".source-add-adapters")).toHaveCount(0);
  await expect(dialog.getByLabel("API address", { exact: true })).toBeVisible();
});

test("deleting one API source preserves the other sources", async ({ page }) => {
  await installTauriMock(page, { locale: "en", populated: true, sourceCount: 3 });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await page.getByRole("tab", { name: "Sources", exact: true }).click();
  await expect(page.locator(".source-card")).toHaveCount(3);
  const source = page.locator(".source-card[data-source-id='source_synthetic']");
  await source.locator('summary[aria-haspopup="menu"]').click();
  await source.getByRole("menuitem", { name: "Delete", exact: true }).click();
  await page.getByRole("dialog", { name: "Confirm action" }).getByRole("button", { name: "Confirm", exact: true }).click();
  await expect(source).toHaveCount(0);
  await expect(page.locator(".source-card")).toHaveCount(2);
  await expect(page.locator(".source-card[data-source-id='source_synthetic_2']")).toContainText("Backup API 1");
  await expect(page.locator(".source-card[data-source-id='source_synthetic_3']")).toContainText("Backup API 2");
  const deletions = await page.evaluate(() => (window as unknown as {
    __TAURI_TEST_INVOKES__: Array<{ command: string; args: { sourceId?: string } }>;
  }).__TAURI_TEST_INVOKES__.filter((call) => call.command === "delete_local_source"));
  expect(deletions).toHaveLength(1);
  expect(deletions[0]?.args.sourceId).toBe("source_synthetic");
});

test("native Messages source can launch OpenCode while direct ChatGPT is disabled", async ({ page }) => {
  await installTauriMock(page, { locale: "en", populated: true,
    sourceProtocolBindings: [{ wireApi: "messages", adapter: "native", modelIds: ["gpt-5.4", "gpt-5.4-mini"] }] });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await page.getByRole("tab", { name: "Sources", exact: true }).click();
  const sourceRow = page.locator(".source-card").filter({ hasText: "Example compatible API" });
  await sourceRow.getByRole("button", { name: "Launch", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "Where do you want to launch this source?" });
  await expect(dialog.getByRole("button", { name: "ChatGPT", exact: true })).toBeDisabled();
  await dialog.getByRole("button", { name: "OpenCode", exact: true }).click();
  await expect.poll(() => page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string }> }).__TAURI_TEST_INVOKES__.some((call) => call.command === "launch_opencode_source"))).toBe(true);
});

for (const resolvedRoutes of [[], [{ wireApi: "responses", adapter: "native", modelIds: [] }]] as const) {
  test(`source launch respects empty backend routes (${resolvedRoutes.length} bindings)`, async ({ page }) => {
    await installTauriMock(page, { locale: "en", populated: true,
      sourceResolvedProtocolBindings: resolvedRoutes.map(binding => ({ ...binding, modelIds: [...binding.modelIds] })),
    });
    await page.goto("/");
    await page.getByRole("button", { name: "Connections", exact: true }).click();
    await page.getByRole("tab", { name: "Sources", exact: true }).click();
    const source = page.locator(".source-card[data-source-id='source_synthetic']");
    await expect(source.getByRole("button", { name: "Edit", exact: true })).toBeEnabled();
    await expect(source.getByRole("button", { name: "Launch", exact: true })).toBeDisabled();
  });
}

test("adding an unknown source to the pool completes without format selection", async ({ page }) => {
  await installTauriMock(page, { locale: "en", mode: "local", populated: false });
  await page.goto("/");
  await page.getByRole("button", { name: "Pool", exact: true }).click();
  await page.getByRole("button", { name: "Add member", exact: true }).first().click();
  await page.getByRole("dialog", { name: "Add connections to pool" }).getByRole("button", { name: "Add API source" }).click();
  const add = page.getByRole("dialog", { name: "Add API source" });
  await add.getByRole("radio", { name: /Custom API/ }).click();
  await add.getByLabel("Name", { exact: true }).fill("Manual API");
  await add.getByLabel("API address").fill("https://manual.example.test/v1");
  await add.getByLabel("Upstream API key", { exact: true }).fill("synthetic-key");
  await add.getByRole("button", { name: "Add source", exact: true }).click();
  await expect(page.getByRole("dialog", { name: "Add API source" })).toBeHidden();
  await expect(page.locator(".pool-member-card").filter({ hasText: "Manual API" })).toBeVisible();
  const calls = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string }> }).__TAURI_TEST_INVOKES__);
  expect(calls.filter((call) => call.command === "set_local_pool_membership")).toHaveLength(1);
  expect(calls.filter((call) => call.command === "probe_local_source")).toHaveLength(0);
});

for (const width of [1160, 390]) {
  test(`model rules render the backend catalog without legacy route controls at ${width}px`, async ({ page }) => {
    await installTauriMock(page, { locale: "en", populated: true, modelProtocolRoutes: {
      "gpt-5.4": [{ clientWireApi: "responses", upstreamWireApi: "messages", features: { text: "confirmed", function_tools: "unsupported" }, reasoningEfforts: ["low", "high"] }],
    } });
    await page.setViewportSize({ width, height: 844 });
    await page.goto("/");
    await page.getByRole("button", { name: "Pool", exact: true }).click();
    await page.getByRole("tab", { name: "Model Rules", exact: true }).click();
    const modelRow = page.locator("[data-model-id='gpt-5.4']");
    await expect(modelRow).toBeVisible();
    await expect(modelRow).toContainText("gpt-5.4");
    await expect(modelRow.locator(".model-rule-actions")).toBeVisible();
    await expect(page.getByRole("dialog", { name: "Model compatibility" })).toHaveCount(0);
    await expect(page.locator(".source-protocol-availability")).toHaveCount(0);
    await expect(page.locator(".source-add-adapters")).toHaveCount(0);
    await page.screenshot({ path: `output/playwright/model-rules-${width}.png` });
  });
}
