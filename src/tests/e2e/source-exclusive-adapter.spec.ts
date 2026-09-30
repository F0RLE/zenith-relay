import { expect, test } from "../bun-playwright";
import { installTauriMock } from "./tauri-mock";

test("source setup saves automatic routing without exposing adapter controls", async ({ page }) => {
  await installTauriMock(page, { mode: "zenith", locale: "en", populated: false, readyConnected: false });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await page.getByRole("button", { name: "Add source", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "Add source" });
  await dialog.getByRole("radio", { name: /Custom API/ }).click();
  await dialog.getByLabel("Name").fill("Automatic API");
  await dialog.getByLabel("API address").fill("https://api.example.test/v1");
  await dialog.getByLabel("Upstream API key").fill("sk-automatic-test");
  await expect(dialog.locator(".source-add-adapters")).toHaveCount(0);
  await expect(dialog.locator(".source-model-mode")).toHaveCount(0);
  await expect(dialog.getByRole("button", { name: "Save", exact: true })).toBeEnabled();
  await dialog.getByRole("button", { name: "Save", exact: true }).click();

  const input = await page.evaluate(() => {
    const calls = (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: { input?: Record<string, unknown> } }> }).__TAURI_TEST_INVOKES__;
    return calls.findLast((call) => call.command === "create_local_source")?.args.input;
  });
  expect(input).toMatchObject({
    name: "Automatic API",
    baseUrl: "https://api.example.test/v1",
    wireApi: "responses",
    protocolBindings: [],
    models: [],
  });
});

test("legacy native protocol bindings remain editable without a routing panel", async ({ page }) => {
  await installTauriMock(page, {
    mode: "local",
    locale: "en",
    populated: true,
    usagePresent: false,
    sourceProtocolBindings: [{
      wireApi: "gemini",
      adapter: "native",
      reasoningMode: "disabled",
      modelIds: ["gpt-5.4", "gpt-5.4-mini"],
    }],
  });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await page.getByRole("tab", { name: "Sources", exact: true }).click();
  await page.locator(".source-table tbody tr").first().getByRole("button", { name: "Edit", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "Edit source" });
  await expect(dialog.locator(".source-add-adapters")).toHaveCount(0);
  await expect(dialog.getByRole("tab", { name: "General", exact: true })).toBeVisible();
  await dialog.getByRole("button", { name: "Save", exact: true }).click();
  const input = await page.evaluate(() => {
    const calls = (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: { input?: Record<string, unknown> } }> }).__TAURI_TEST_INVOKES__;
    return calls.findLast((call) => call.command === "update_local_source")?.args.input;
  });
  expect(input).toMatchObject({
    protocolBindings: [{ wireApi: "gemini", adapter: "native", modelIds: ["gpt-5.4", "gpt-5.4-mini"] }],
  });
});
