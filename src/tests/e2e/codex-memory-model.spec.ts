import { expect, test } from "../bun-playwright";
import { installTauriMock } from "./tauri-mock";

for (const width of [1160, 390]) {
  test(`ChatGPT memory model saves and resets without changing background policy at ${width}px`, async ({ page }) => {
    await page.setViewportSize({ width, height: 900 });
    await installTauriMock(page, {
      locale: "en", mode: "local", theme: "dark", populated: true,
      codexMemoryModel: "gpt-5.4-mini", codexBackgroundTasksEnabled: false,
    });
    await page.goto("/");
    await page.getByRole("button", { name: "Access", exact: true }).click();
    await page.getByRole("tab", { name: "Applications", exact: true }).click();
    await page.getByRole("tab", { name: "ChatGPT", exact: true }).click();
    const control = page.locator('[data-memory-stage="consolidation"]');
    const model = control.getByRole("button", { name: /^Memory summary model:/ });
    await expect(model).toHaveAttribute("data-value", "gpt-5.4-mini");
    await expect(page.getByRole("checkbox", { name: "Allow background tasks" })).not.toBeChecked();
    await model.click();
    await page.locator('[role="option"][data-value="gpt-5.4"]').click();
    await expect(model).toHaveAttribute("data-value", "gpt-5.4");
    await page.getByRole("tab", { name: "Claude Desktop", exact: true }).click();
    await page.getByRole("tab", { name: "ChatGPT", exact: true }).click();
    await expect(model).toHaveAttribute("data-value", "gpt-5.4");
    await expect(page.getByRole("checkbox", { name: "Allow background tasks" })).not.toBeChecked();
    expect(await control.evaluate((element) => element.scrollWidth <= element.clientWidth)).toBe(true);
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    await control.scrollIntoViewIfNeeded();
    await page.screenshot({ path: `output/playwright/codex-memory-model-${width}.png` });
    await model.click();
    await page.getByRole("option", { name: "Application default", exact: true }).click();
    await expect(model).toHaveAttribute("data-value", "");
    await page.getByRole("tab", { name: "Claude Desktop", exact: true }).click();
    await page.getByRole("tab", { name: "ChatGPT", exact: true }).click();
    await expect(model).toHaveAttribute("data-value", "");
  });
}

test("ChatGPT memory model keeps the saved choice when native save fails", async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 900 });
  const savedModel = "synthetic-unavailable-memory-model-with-a-long-catalog-id";
  await installTauriMock(page, {
    locale: "en", mode: "local", populated: true, codexMemoryModelFailure: true,
    codexMemoryModel: savedModel,
  });
  await page.goto("/");
  await page.getByRole("button", { name: "Access", exact: true }).click();
  await page.getByRole("tab", { name: "Applications", exact: true }).click();
  await page.getByRole("tab", { name: "ChatGPT", exact: true }).click();
  const control = page.locator('[data-memory-stage="consolidation"]');
  const model = control.getByRole("button", { name: /^Memory summary model:/ });
  await expect(model).toHaveAttribute("data-value", savedModel);
  expect(await control.evaluate((element) => element.scrollWidth <= element.clientWidth)).toBe(true);
  await model.click();
  await page.locator('[role="option"][data-value="gpt-5.4"]').click();
  await expect(control.getByRole("alert")).toContainText("Memory model was not saved");
  await expect(model).toHaveAttribute("data-value", savedModel);
  await control.getByRole("button", { name: "Retry", exact: true }).click();
  await expect(control.getByRole("alert")).toHaveCount(0);
  await expect(model).toBeEnabled();
});

for (const theme of ["light", "dark"] as const) {
  for (const width of [1160, 390]) {
    test(`ChatGPT memory model visual ${theme} at ${width}px`, async ({ page }) => {
      await page.setViewportSize({ width, height: 900 });
      await installTauriMock(page, {
        locale: "ru", mode: "local", theme, populated: true, codexMemoryModel: "gpt-5.4-mini",
      });
      await page.goto("/");
      await page.getByRole("button", { name: "Доступ", exact: true }).click();
      await page.getByRole("tab", { name: "Приложения", exact: true }).click();
      await page.getByRole("tab", { name: "ChatGPT", exact: true }).click();
      const control = page.locator('[data-memory-stage="consolidation"]');
      const model = control.getByRole("button", { name: /^Модель сводок памяти:/ });
      await expect(model).toHaveAttribute("data-value", "gpt-5.4-mini");
      await expect(model).toBeEnabled();
      await expect(control).not.toContainText("После перезапуска ChatGPT");
      expect(await control.evaluate((element) => element.scrollWidth <= element.clientWidth)).toBe(true);
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
      await page.locator(".integration-chatgpt-settings").screenshot({
        path: `output/playwright/codex-memory-model-ru-${theme}-${width}.png`,
      });
    });
  }
}

test("memory stages have independent models and switches while background tasks remain blocked", async ({ page }) => {
  await installTauriMock(page, {
    locale: "en", mode: "local", populated: true, codexBackgroundTasksEnabled: false,
    codexMemoryModel: "gpt-5.4-mini", codexMemoryExtractionModel: "gpt-5.4",
  });
  await page.goto("/");
  await page.getByRole("button", { name: "Access", exact: true }).click();
  await page.getByRole("tab", { name: "Applications", exact: true }).click();
  await page.getByRole("tab", { name: "ChatGPT", exact: true }).click();
  const extraction = page.locator('[data-memory-stage="extraction"]');
  const consolidation = page.locator('[data-memory-stage="consolidation"]');
  const extractToggle = extraction.getByRole("checkbox");
  const summaryToggle = consolidation.getByRole("checkbox");
  await expect(extractToggle).toBeChecked();
  await expect(summaryToggle).toBeChecked();
  await extractToggle.uncheck();
  await expect(extractToggle).not.toBeChecked();
  await expect(summaryToggle).toBeChecked();
  await summaryToggle.uncheck();
  await extractToggle.check();
  await expect(summaryToggle).not.toBeChecked();
  const model = extraction.getByRole("button", { name: /^Memory extraction model:/ });
  await expect(model).toHaveAttribute("data-value", "gpt-5.4");
  await model.click();
  await page.getByRole("option", { name: "Application default", exact: true }).click();
  await expect(model).toHaveAttribute("data-value", "");
  await expect(consolidation.getByRole("button", { name: /^Memory summary model:/ })).toHaveAttribute("data-value", "gpt-5.4-mini");
  await page.getByRole("tab", { name: "Claude Desktop", exact: true }).click();
  await page.getByRole("tab", { name: "ChatGPT", exact: true }).click();
  await expect(extractToggle).toBeChecked();
  await expect(summaryToggle).not.toBeChecked();
  await expect(model).toHaveAttribute("data-value", "");
  await expect(page.getByRole("checkbox", { name: "Allow background tasks" })).not.toBeChecked();
});

test("failed memory policy save restores the switch without changing its model", async ({ page }) => {
  await installTauriMock(page, { locale: "en", mode: "local", populated: true, codexMemoryToggleFailure: true, codexMemoryExtractionModel: "gpt-5.4" });
  await page.goto("/");
  await page.getByRole("button", { name: "Access", exact: true }).click();
  await page.getByRole("tab", { name: "Applications", exact: true }).click();
  await page.getByRole("tab", { name: "ChatGPT", exact: true }).click();
  const extraction = page.locator('[data-memory-stage="extraction"]');
  await extraction.getByRole("checkbox").click();
  await expect(extraction.getByRole("checkbox")).toBeChecked();
  await expect(extraction.getByRole("button", { name: /^Memory extraction model:/ })).toHaveAttribute("data-value", "gpt-5.4");
});
