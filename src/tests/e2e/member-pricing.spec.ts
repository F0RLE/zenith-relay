import { expect, test } from "../bun-playwright";
import { installTauriMock, type MockOptions } from "./tauri-mock";
import { chooseOption } from "./operations/helpers";

// Synthetic rates exercise distinct model thresholds; these are not live quotes.
const prices: MockOptions["sourceDetectedModelPrices"] = {
  "gpt-6-astra": {
    inputMicroUsdPerMillion: 2_000_000, outputMicroUsdPerMillion: 8_000_000,
    cachedInputMicroUsdPerMillion: 200_000,
    flex: { input: 1_000_000, output: 4_000_000, cacheRead: 0 },
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

for (const mode of ["local", "remote"] as const) {
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
    await row.getByRole("button", { name: "gpt-6-astra price tier: Standard", exact: true }).click();
    await expect(page.getByRole("option")).toHaveText(["Standard", "Flex", "Priority"]);
    await page.getByRole("option", { name: "Flex", exact: true }).click();
    await expect(input).toHaveAttribute("placeholder", "1");
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
    await expect(flat.getByText("All sizes", { exact: true })).toBeVisible();
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
