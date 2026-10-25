import { expect, test } from "../bun-playwright";
import { installTauriMock } from "./tauri-mock";
import { setUsageEventOverrides } from "./request-details-fixture";

for (const scenario of [
  { locale: "ru", theme: "dark", width: 360 },
  { locale: "en", theme: "light", width: 1160 },
] as const) {
  test(`image usage remains clear without invented costs: ${scenario.locale}`, async ({ page }) => {
    await page.setViewportSize({ width: scenario.width, height: 900 });
    await installTauriMock(page, { ...scenario, mode: "local", populated: true });
    await page.goto("/");
    await setUsageEventOverrides(page, {
      requestedModel: "gpt-image-test", resolvedModel: "gpt-image-test",
      requestedReasoningEffort: null, effectiveReasoningEffort: null,
      cachedInputTokens: null, cacheWriteInputTokens: null, reasoningTokens: null,
      inputTokens: 20, outputTokens: 8, totalTokens: 28,
      apiEquivalent: { microUsd: 0, pricedTokens: 0, unpricedTokens: 28 },
      apiEquivalentBreakdown: {}, toolUse: null,
      imageUsage: { inputTextTokens: 3, inputImageTokens: 17, outputImageTokens: 8, generatedImages: 1, inputDetailsConsistent: false },
    });
    const ru = scenario.locale === "ru";
    await page.getByRole("button", { name: ru ? "Использование" : "Usage", exact: true }).click();
    await page.getByRole("button", { name: `${ru ? "Сведения о запросе" : "Request details"}: req_synthetic_local`, exact: true }).click();
    const dialog = page.getByRole("dialog");
    await expect(dialog.getByText(ru ? "Готовых изображений" : "Images completed", { exact: true })).toBeVisible();
    await expect(dialog.getByText(ru ? "Вход: текст" : "Text input", { exact: true })).toBeVisible();
    await expect(dialog.getByText(ru ? "Вход: изображения" : "Image input", { exact: true })).toBeVisible();
    await expect(dialog.locator(".request-token-total dd")).toHaveText("28");
    await expect(dialog.locator(".request-details-cost-value")).toHaveCount(0);
    await expect(dialog.locator("p.form-note")).toHaveCount(0);
    await expect.poll(() => dialog.evaluate((element) => element.scrollWidth <= element.clientWidth + 1)).toBe(true);
    await dialog.screenshot({ path: `output/playwright/image-usage-${scenario.locale}.png` });
  });
}
