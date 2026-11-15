import { expect, test } from "../bun-playwright";
import { installTauriMock } from "./tauri-mock";
import { setUsageEventOverrides } from "./request-details-fixture";

test("per-image fees remain visible with unknown image token counts", async ({ page }) => {
  const price = { microUsd: 40_000, pricedTokens: 0, unpricedTokens: 12 };
  const imagePrice = { ...price, unpricedTokens: 0 };
  await installTauriMock(page, { mode: "local", locale: "en", populated: true,
    usageSummaryTotals: { apiEquivalent: price },
  });
  await page.goto("/");
  await expect(page.locator(".overview-chart.cost .overview-chart-summary")).toHaveText("≈$0.04");
  await setUsageEventOverrides(page, {
    requestedModel: "gpt-image-counted", resolvedModel: "gpt-image-counted",
    inputTokens: 12, outputTokens: null, totalTokens: 12,
    cachedInputTokens: null, cacheWriteInputTokens: null, reasoningTokens: null,
    apiEquivalent: price,
    apiEquivalentBreakdown: { output: imagePrice, image: { outputImage: imagePrice } },
    imageUsage: { generatedImages: 1 },
  });
  await page.getByRole("button", { name: "Usage", exact: true }).click();
  await expect(page.locator('.usage-request-table tbody [data-column="equivalent"]')).toHaveText("≈$0.04");
  await page.getByRole("button", { name: "Request details: req_synthetic_local", exact: true }).click();
  const dialog = page.getByRole("dialog");
  await expect(dialog.locator(".request-details-cost-value")).toHaveText("≈$0.04");
  const output = dialog.locator(".request-details-token-child").filter({ hasText: "Image output" });
  await expect(output.locator(".request-token-count")).toHaveText("—");
  await expect(output.locator(".request-token-cost")).toHaveText("≈$0.04");
});

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
    await expect(page.locator('.usage-request-table tbody [data-column="speed"]')).toHaveText("-");
    await expect(page.locator('.usage-request-table tbody [data-column="timing"]')).not.toContainText("/");
    await page.getByRole("button", { name: `${ru ? "Сведения о запросе" : "Request details"}: req_synthetic_local`, exact: true }).click();
    const dialog = page.getByRole("dialog");
    await expect(dialog.getByText(ru ? "Готовых изображений" : "Images completed", { exact: true })).toBeVisible();
    await expect(dialog.getByText(ru ? "Вход: текст" : "Text input", { exact: true })).toBeVisible();
    await expect(dialog.getByText(ru ? "Вход: изображения" : "Image input", { exact: true })).toBeVisible();
    await expect(dialog.locator(".request-token-total dd")).toHaveText("28");
    await expect(dialog.locator(".request-details-cost-value")).toHaveCount(0);
    await expect(dialog.getByText(ru ? "Первый ответ" : "First response", { exact: true })).toHaveCount(0);
    await expect(dialog.locator("p.form-note")).toHaveCount(0);
    await expect.poll(() => dialog.evaluate((element) => element.scrollWidth <= element.clientWidth + 1)).toBe(true);
    await dialog.screenshot({ path: `output/playwright/image-usage-${scenario.locale}.png` });
  });
}

test("priced image shows server-calculated modality costs and only total timing", async ({ page }) => {
  await page.setViewportSize({ width: 1160, height: 900 });
  await installTauriMock(page, { locale: "en", theme: "dark", mode: "local", populated: true });
  await page.goto("/");
  const price = (microUsd: number, pricedTokens: number) => ({ microUsd, pricedTokens, unpricedTokens: 0 });
  await setUsageEventOverrides(page, {
    requestedModel: "gpt-image-test", resolvedModel: "gpt-image-test",
    requestedReasoningEffort: null, effectiveReasoningEffort: null,
    cachedInputTokens: null, cacheWriteInputTokens: null, reasoningTokens: null,
    inputTokens: 45, outputTokens: 515, totalTokens: 560, latencyMs: 24_550, ttftMs: 100,
    apiEquivalent: price(15_675, 560),
    apiEquivalentBreakdown: { input: price(225, 45), output: price(15_450, 515),
      uncachedInput: price(225, 45), cacheRead: null, cacheWrite: null,
      image: { inputText: price(225, 45), inputImage: price(0, 0), outputImage: price(15_450, 515), outputText: price(0, 0), svgInput: null, svgOutput: null } },
    imageUsage: { inputTextTokens: 45, inputImageTokens: 0, outputImageTokens: 515, outputTextTokens: 0, generatedImages: 1 },
  });
  await page.getByRole("button", { name: "Usage", exact: true }).click();
  await expect(page.locator('.usage-request-table tbody [data-column="timing"]')).not.toContainText("/");
  await expect(page.locator('.usage-request-table tbody [data-column="equivalent"]')).toContainText("$");
  await page.getByRole("button", { name: "Request details: req_synthetic_local", exact: true }).click();
  const dialog = page.getByRole("dialog");
  await expect(dialog.locator(".request-details-cost-value")).toContainText("0.015675");
  await expect(dialog.locator(".request-details-metrics")).not.toContainText("First response");
  await expect(dialog.locator(".request-details-token-child").filter({ hasText: "Text input" })).toContainText("0.000225");
  await expect(dialog.locator(".request-details-token-child").filter({ hasText: "Image output" })).toContainText("0.01545");
  await expect.poll(() => dialog.evaluate(element => element.scrollWidth <= element.clientWidth + 1)).toBe(true);
  await dialog.screenshot({ path: "output/playwright/image-usage-priced.png" });
});
