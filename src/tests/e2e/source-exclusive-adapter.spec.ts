import { expect, test } from "../bun-playwright";
import { installTauriMock } from "./tauri-mock";

test("source setup keeps one exclusive adapter for all manual models", async ({ page }) => {
  await installTauriMock(page, { mode: "zenith", locale: "en", populated: false, readyConnected: false });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await page.getByRole("button", { name: "Add source", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "Add source" });
  await dialog.getByRole("radio", { name: /Custom API/ }).click();
  await dialog.locator(".source-model-mode label").filter({ hasText: "Manual" }).click();
  await dialog.getByLabel("Model identifier").fill("claude-3-7-sonnet");
  await dialog.getByLabel("Model identifier").press("Enter");
  await dialog.getByLabel("Model identifier").fill("gpt-5.4");
  await dialog.getByLabel("Model identifier").press("Enter");
  await dialog.locator(".source-add-adapters > summary").click();
  await expect(dialog.locator(".source-route-simple-options")).toBeVisible();
  await expect(dialog.locator(".source-route-matrix")).toHaveCount(0);
  const adapterPicker = dialog.getByRole("radiogroup", { name: "Protocol" });
  await expect(adapterPicker.getByRole("radio")).toHaveCount(3);
  const messages = adapterPicker.getByRole("radio", { name: /Messages/ });
  const responses = adapterPicker.getByRole("radio", { name: /Responses/ });
  const gemini = adapterPicker.getByRole("radio", { name: /Google/ });
  await messages.click();
  await expect(messages).toHaveAttribute("aria-checked", "true");
  await expect(responses).toHaveAttribute("aria-checked", "false");
  await responses.click();
  await expect(responses).toHaveAttribute("aria-checked", "true");
  await expect(messages).toHaveAttribute("aria-checked", "false");
  await gemini.click();
  await expect(gemini).toHaveAttribute("aria-checked", "true");
  await expect(responses).toHaveAttribute("aria-checked", "false");
  await expect(messages).toHaveAttribute("aria-checked", "false");
  await dialog.getByLabel("Name").fill("Gemini source");
  await dialog.getByLabel("API address").fill("https://api.example.test/v1");
  await dialog.getByLabel("Upstream API key").fill("sk-exclusive-test");
  await dialog.getByRole("button", { name: "Save", exact: true }).click();
  const input = await page.evaluate(() => {
    const calls = (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: { input?: Record<string, unknown> } }> }).__TAURI_TEST_INVOKES__;
    return calls.findLast((call) => call.command === "create_local_source")?.args.input;
  });
  expect(input).toMatchObject({
    protocolBindings: [{ wireApi: "gemini", adapter: "native", modelIds: ["claude-3-7-sonnet", "gpt-5.4"] }],
    models: ["claude-3-7-sonnet", "gpt-5.4"],
  });
});

test("source editor exposes native Gemini separately from the Responses bridge", async ({ page }) => {
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
  const sourceRow = page.locator(".source-table tbody tr").first();
  await sourceRow.getByRole("button", { name: "Edit", exact: true }).click();

  const dialog = page.getByRole("dialog", { name: "Edit source" });
  await dialog.getByRole("tab", { name: "Models and formats", exact: true }).click();
  const matrix = dialog.locator(".source-route-matrix");
  await expect(matrix.locator('[data-wire-api="gemini"]')).toHaveCount(1);
  await expect(matrix.locator('[data-wire-api="gemini"] strong')).toHaveText("Gemini");
  await expect(matrix.locator('[data-wire-api="gemini"] input')).toBeChecked();
  await expect(matrix.locator(".source-route-bridge-heading")).toHaveCount(0);
  await page.screenshot({ path: "output/playwright/native-gemini-route-en-1160x760.png" });

  await dialog.getByRole("tab", { name: "Adapters", exact: true }).click();
  await expect(matrix.locator('[data-wire-api="gemini"]')).toHaveCount(0);
  await expect(matrix.locator(".source-route-bridge-heading")).toHaveCount(2);
  await expect(matrix.locator(".source-route-bridge-heading").nth(1)).toContainText("Responses → Gemini");
  await page.screenshot({ path: "output/playwright/native-gemini-adapters-en-1160x760.png" });
});
