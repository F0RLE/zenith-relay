import { expect, test } from "../bun-playwright";
import { installTauriMock, type MockOptions } from "./tauri-mock";
import { chooseOption } from "./operations/helpers";

// Synthetic rates exercise distinct model thresholds; these are not live quotes.
const prices: MockOptions["sourceDetectedModelPrices"] = {
  "gpt-6-astra": {
    inputMicroUsdPerMillion: 2_000_000, outputMicroUsdPerMillion: 8_000_000,
    cachedInputMicroUsdPerMillion: 200_000,
    cacheWriteMicroUsdPerMillion: 870_000,
    flex: { input: 1_000_000, output: 4_000_000, cacheRead: 0, cacheWrite: 410_000 },
    priority: { input: 4_000_000, output: 16_000_000 },
    above272k: {
      standard: { input: 4_000_000, output: 12_000_000 },
      flex: { input: 2_000_000, output: 6_000_000 },
      priority: { input: 8_000_000, output: 24_000_000 },
    },
  },
  "gpt-5.4-mini": { inputMicroUsdPerMillion: 750_000, outputMicroUsdPerMillion: 4_500_000 },
  "gemini-2.5-pro": {
    inputMicroUsdPerMillion: 1_250_000, outputMicroUsdPerMillion: 10_000_000,
    above200k: { standard: { input: 2_500_000, output: 15_000_000 } },
  },
};
const modelMetadata: MockOptions["modelMetadata"] = {
  "gpt-6-astra": { catalogProvider: "openai", catalogFamily: "gpt", catalogName: "GPT-6 Astra" },
  "gpt-5.4-mini": { catalogProvider: "openai", catalogFamily: "gpt", catalogName: "GPT-5.4 mini" },
  "gemini-2.5-pro": { catalogProvider: "google", catalogFamily: "gemini", catalogName: "Gemini 2.5 Pro" },
};

test("source editor keeps provider prices and does not invent price schedules", async ({ page }) => {
  await page.setViewportSize({ width: 860, height: 960 });
  await installTauriMock(page, {
    mode: "local", locale: "en", theme: "dark", populated: true,
    serverModelOrder: ["gpt-6.1-sol"],
    modelMetadata: { "gpt-6.1-sol": { catalogProvider: "openai", catalogFamily: "gpt", catalogName: "Synthetic Sol" } },
    sourceDetectedModelPrices: { "gpt-6.1-sol": {
      inputMicroUsdPerMillion: 80_000, outputMicroUsdPerMillion: 400_000, cacheWriteMicroUsdPerMillion: 100_000,
    } },
  });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await page.getByRole("tab", { name: "Sources", exact: true }).click();
  await page.getByRole("button", { name: "Edit", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "Edit source", exact: true });
  await dialog.getByRole("tab", { name: "Pricing", exact: true }).click();
  await expect(dialog.getByText("USD / 1M tokens", { exact: true })).toBeVisible();
  await dialog.locator(".source-price-group > summary").click();
  const row = dialog.locator(".source-price-row").filter({ hasText: "gpt-6.1-sol" });
  await expect(row.getByRole("textbox", { name: "Input token price for gpt-6.1-sol", exact: true })).toHaveAttribute("placeholder", "0.08");
  await expect(row.locator("code")).toHaveAttribute("data-relay-tooltip", "gpt-6.1-sol\nAPI source price");
  await expect(row.locator(".source-price-tier, .source-price-context")).toHaveCount(0);
  await expect(row.locator(".source-price-schedule")).toHaveCount(0);
  await dialog.screenshot({ path: "output/playwright/source-pricing-evidence-860.png" });
});

for (const mode of ["local"] as const) {
  test(`${mode} member pricing keeps tier and model context independent through save and reset`, async ({ page }) => {
    await installTauriMock(page, {
      mode, locale: "en", populated: true,
      serverModelOrder: Object.keys(prices), sourceDetectedModelPrices: prices, modelMetadata,
    });
    await page.goto("/");
    await page.getByRole("button", { name: "Pool", exact: true }).click();
    const open = page.getByRole("button", { name: "Pool member policy: Example compatible API", exact: true });
    await open.click();
    const dialog = page.getByRole("dialog", { name: "Pool member policy", exact: true });
    await dialog.getByRole("tab", { name: "Pricing", exact: true }).click();
    const row = dialog.locator(".source-price-row").filter({ hasText: "gpt-6-astra" });
    const input = row.getByRole("textbox", { name: "Input token price for gpt-6-astra", exact: true });
    const output = row.getByRole("textbox", { name: "Output token price for gpt-6-astra", exact: true });
    const cacheWrite = row.getByRole("textbox", { name: "Cache write price for gpt-6-astra", exact: true });
    await expect(cacheWrite).toHaveAttribute("placeholder", "0.87");
    await expect(row.getByRole("textbox", { name: /30-minute/ })).toHaveCount(0);
    await row.getByRole("button", { name: "gpt-6-astra price tier: Standard", exact: true }).click();
    await expect(page.getByRole("option")).toHaveText(["Standard", "Flex", "Priority"]);
    await page.getByRole("option", { name: "Flex", exact: true }).click();
    await expect(input).toHaveAttribute("placeholder", "1");
    await expect(cacheWrite).toHaveAttribute("placeholder", "0.41");
    await row.getByRole("button", { name: "gpt-6-astra context range: Short · ≤ 272k", exact: true }).click();
    await expect(page.getByRole("option")).toHaveText(["Short · ≤ 272k", "Long · > 272k"]);
    await page.getByRole("option", { name: "Long · > 272k", exact: true }).click();
    await expect(input).toHaveAttribute("placeholder", "2");
    await input.fill("2.75");
    await expect(output).toHaveValue("6");
    await chooseOption(page, row, "gpt-6-astra price tier", "priority");
    await expect(input).toHaveValue("8");
    await chooseOption(page, row, "gpt-6-astra context range", "base");
    await expect(input).toHaveValue("4");
    await chooseOption(page, row, "gpt-6-astra price tier", "flex");
    await expect(input).toHaveValue("1");
    await chooseOption(page, row, "gpt-6-astra context range", "above272k");
    await expect(input).toHaveValue("2.75");
    const flat = dialog.locator(".source-price-row").filter({ hasText: "gpt-5.4-mini" });
    await expect(flat.locator(".source-price-context")).toHaveCount(0);
    await expect(flat.locator(".source-price-schedule")).toHaveCount(0);
    await expect(flat.locator(".source-price-tier")).toHaveCount(0);
    await expect(flat.locator("code")).toHaveAttribute("data-price-source", "provider");
    const gemini = dialog.locator(".source-price-row").filter({ hasText: "gemini-2.5-pro" });
    await gemini.getByRole("button", { name: "gemini-2.5-pro context range: Short · ≤ 200k", exact: true }).click();
    await expect(page.getByRole("option")).toHaveText(["Short · ≤ 200k", "Long · > 200k"]);
    await page.keyboard.press("Escape");
    await expect(dialog).toBeVisible();
    await dialog.getByRole("button", { name: "Save policy", exact: true }).click();
    await expect(dialog).toBeHidden();
    await open.click();
    await dialog.getByRole("tab", { name: "Pricing", exact: true }).click();
    await chooseOption(page, row, "gpt-6-astra price tier", "flex");
    await chooseOption(page, row, "gpt-6-astra context range", "above272k");
    await expect(input).toHaveValue("2.75");
    await row.getByRole("button", { name: /gpt-6-astra/ }).filter({ has: page.locator("svg.lucide-rotate-ccw") }).click();
    await expect(input).toHaveValue("");
    await expect(input).toHaveAttribute("placeholder", "2");
    await expect(output).toHaveAttribute("placeholder", "6");
    await dialog.getByRole("button", { name: "Cancel", exact: true }).click();
  });
}

for (const theme of ["light", "dark"] as const) {
  for (const width of [1200, 390]) {
    test(`separate model price selectors fit Russian ${theme} ${width}`, async ({ page }) => {
      await page.setViewportSize({ width, height: 900 });
      await installTauriMock(page, {
        mode: "local", locale: "ru", theme, populated: true,
        serverModelOrder: Object.keys(prices), sourceDetectedModelPrices: prices, modelMetadata,
      });
      await page.goto("/");
      await page.getByRole("button", { name: "Пул", exact: true }).click();
      await page.getByRole("button", { name: "Правила участника пула: Example compatible API", exact: true }).click();
      const dialog = page.getByRole("dialog", { name: "Правила участника пула", exact: true });
      await dialog.getByRole("tab", { name: "Цены", exact: true }).click();
      const overflow = await dialog.locator(".source-price-row, .source-price-picker").evaluateAll((elements) =>
        elements.some((element) => element.scrollWidth > element.clientWidth + 1));
      expect(overflow).toBe(false);
      await dialog.screenshot({ path: `output/playwright/member-pricing-final-${theme}-${width}.png` });
      await dialog.getByRole("button", { name: "Диапазон контекста gpt-6-astra: Короткий · ≤ 272k", exact: true }).click();
      await expect(page.getByRole("option")).toHaveText(["Короткий · ≤ 272k", "Длинный · > 272k"]);
      const menu = page.getByRole("listbox");
      expect(await menu.evaluate((element) => {
        const rect = element.getBoundingClientRect();
        return rect.left >= 0 && rect.right <= innerWidth && element.scrollWidth <= element.clientWidth;
      })).toBe(true);
      await page.screenshot({ path: `output/playwright/member-pricing-context-${theme}-${width}.png` });
    });
  }
}

for (const width of [1200, 390]) {
  test(`source-owned evidence controls price schedules and mixed cache columns ${width}`, async ({ page }) => {
    await page.setViewportSize({ width, height: 900 });
    const models = ["gpt-6-astra", "claude-hosted", "private-missing"];
    const metadata = { catalogRevision: "synthetic-reference", catalogFetchedAtMs: 1, stale: false };
    await installTauriMock(page, {
      mode: "local", locale: "en", theme: "dark", populated: true,
      serverModelOrder: models,
      modelMetadata: { ...modelMetadata, "claude-hosted": { catalogProvider: "anthropic", catalogFamily: "claude", catalogName: "Synthetic Claude" } },
      sourceResolvedModelPrices: {
        "gpt-6-astra": { ...metadata, source: "liteLlmExact", stale: true, quote: {
          input: 1_700_000, output: 6_400_000,
          priority: { input: 3_400_000 }, above200k: { priority: { output: 8_800_000 } },
        } },
        "claude-hosted": { ...metadata, source: "provider", quote: {
          input: 1_000_000, output: 2_000_000, cacheWrite: 0, cacheWrite5m: 2_500_000, cacheWrite1h: 4_000_000,
        } },
        "private-missing": { ...metadata, source: "unpriced", quote: null },
      },
    });
    await page.goto("/");
    await page.getByRole("button", { name: "Pool", exact: true }).click();
    await page.getByRole("button", { name: "Pool member policy: Example compatible API", exact: true }).click();
    const dialog = page.getByRole("dialog", { name: "Pool member policy", exact: true });
    await dialog.getByRole("tab", { name: "Pricing", exact: true }).click();
    await expect(dialog.getByText("USD / 1M tokens", { exact: true })).toBeVisible();
    const row = dialog.locator(".source-price-row").filter({ hasText: "gpt-6-astra" });
    await expect(row.getByRole("textbox", { name: "Input token price for gpt-6-astra", exact: true })).toHaveAttribute("placeholder", "1.7");
    await expect(row.locator("code")).toHaveAttribute("data-relay-tooltip", "gpt-6-astra\nLiteLLM reference · stale reference");
    await expect(row.locator(".source-price-context")).toHaveCount(0);
    await row.getByRole("button", { name: "gpt-6-astra price tier: Standard", exact: true }).click();
    await expect(page.getByRole("option")).toHaveText(["Standard", "Priority"]);
    await page.getByRole("option", { name: "Priority", exact: true }).click();
    await chooseOption(page, row, "gpt-6-astra context range", "above200k");
    await expect(row.getByRole("textbox", { name: "Input token price for gpt-6-astra", exact: true })).toHaveAttribute("placeholder", "3.4");
    await expect(row.getByRole("textbox", { name: "Output token price for gpt-6-astra", exact: true })).toHaveAttribute("placeholder", "8.8");
    const claude = dialog.locator(".source-price-row").filter({ hasText: "claude-hosted" });
    for (const [label, price] of [["Cache write price", "0"], ["5-minute cache write price", "2.5"], ["1-hour cache write price", "4"]]) {
      await expect(claude.getByRole("textbox", { name: `${label} for claude-hosted`, exact: true })).toHaveAttribute("placeholder", price);
    }
    const missing = dialog.locator(".source-price-row").filter({ hasText: "private-missing" });
    await expect(missing.locator("code")).toHaveAttribute("data-relay-tooltip", "private-missing\nPrice unavailable");
    await expect(missing.getByRole("textbox", { name: "Input token price for private-missing", exact: true })).toHaveAttribute("placeholder", "—");
    await expect(missing.locator(".source-price-tier, .source-price-context")).toHaveCount(0);
    await expect(dialog.locator(".source-price-origin, .source-price-flat, .source-price-picker > span, .source-price-note")).toHaveCount(0);
    expect(await dialog.locator(".source-price-row, .source-price-picker, .source-price-fields").evaluateAll((elements) =>
      elements.some((element) => element.scrollWidth > element.clientWidth + 1))).toBe(false);
    await dialog.screenshot({ path: `output/playwright/member-pricing-evidence-${width}.png` });
    const modelName = row.locator("code");
    await modelName.hover();
    await expect(page.getByRole("tooltip")).toContainText("LiteLLM reference · stale reference");
    await page.mouse.move(0, 0);
    await modelName.focus();
    await page.keyboard.press("Tab");
    await page.keyboard.press("Shift+Tab");
    await expect(modelName).toBeFocused();
    await expect(page.getByRole("tooltip")).toContainText("LiteLLM reference · stale reference");
  });
}
