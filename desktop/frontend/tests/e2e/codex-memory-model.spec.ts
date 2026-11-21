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

test("memory model retry waits for the fresh value before allowing another save", async ({ page }) => {
  await installTauriMock(page, { locale: "en", mode: "local", populated: true, codexMemoryModel: "gpt-5.4-mini" });
  await page.goto("/");
  await page.getByRole("button", { name: "Access", exact: true }).click();
  await page.getByRole("tab", { name: "Applications", exact: true }).click();
  await page.getByRole("tab", { name: "ChatGPT", exact: true }).click();
  const control = page.locator('[data-memory-stage="consolidation"]');
  const model = control.getByRole("button", { name: /^Memory summary model:/ });
  await expect(model).toHaveAttribute("data-value", "gpt-5.4-mini");
  await page.evaluate(() => {
    const mock = window as unknown as {
      __TAURI_INTERNALS__: { invoke: (command: string, args?: unknown, options?: unknown) => Promise<unknown> };
      finishMemoryReload?: () => void;
    };
    const original = mock.__TAURI_INTERNALS__.invoke.bind(mock.__TAURI_INTERNALS__);
    let failed = false;
    mock.__TAURI_INTERNALS__.invoke = async (command, args, options) => {
      if (command === "set_local_codex_memory_model" && !failed) {
        failed = true;
        // Another writer changed the file before this command could commit.
        await original(command, { input: { stage: "consolidation", model: null, expectedModel: "gpt-5.4-mini" } });
        throw { code: "conflict", message: "Synthetic external memory model change" };
      }
      if (command === "get_local_codex_memory_model") {
        await new Promise<void>((resolve) => { mock.finishMemoryReload = resolve; });
      }
      return original(command, args, options);
    };
  });
  await model.click();
  await page.getByRole("option", { name: "gpt-5.4", exact: true }).click();
  await expect(control.getByRole("alert")).toContainText("Memory model was not saved");
  await control.getByRole("button", { name: "Retry", exact: true }).click();
  await expect.poll(() => page.evaluate(() => Boolean((window as unknown as { finishMemoryReload?: unknown }).finishMemoryReload))).toBe(true);
  await expect(model).toBeDisabled();
  await page.evaluate(() => (window as unknown as { finishMemoryReload: () => void }).finishMemoryReload());
  await expect(model).toBeEnabled();
  await expect(model).toHaveAttribute("data-value", "");
  await model.click();
  await page.getByRole("option", { name: "gpt-5.4", exact: true }).click();
  await expect(model).toHaveAttribute("data-value", "gpt-5.4");
  await expect(control.getByRole("alert")).toHaveCount(0);
});

for (const fails of [false, true]) {
  test(`memory policy prevents overlapping saves and settles correctly fail=${fails}`, async ({ page }) => {
    await installTauriMock(page, { locale: "en", mode: "local", populated: true });
    await page.goto("/");
    await page.getByRole("button", { name: "Access", exact: true }).click();
    await page.getByRole("tab", { name: "Applications", exact: true }).click();
    await page.getByRole("tab", { name: "ChatGPT", exact: true }).click();
    await page.evaluate((fails) => {
      const mock = window as unknown as {
        __TAURI_INTERNALS__: { invoke: (command: string, args?: unknown, options?: unknown) => Promise<unknown> };
        finishMemorySave?: () => void;
      };
      const original = mock.__TAURI_INTERNALS__.invoke.bind(mock.__TAURI_INTERNALS__);
      mock.__TAURI_INTERNALS__.invoke = async (command, args, options) => {
        if (command === "set_local_codex_memory_enabled" && (args as { input: { stage: string } }).input.stage === "extraction") {
          await new Promise<void>((resolve) => { mock.finishMemorySave = resolve; });
          if (fails) throw { code: "storage_error", message: "Synthetic memory policy save failure" };
        }
        return original(command, args, options);
      };
    }, fails);
    const toggle = page.locator('[data-memory-stage="extraction"]').getByRole("checkbox");
    const independent = page.locator('[data-memory-stage="consolidation"]').getByRole("checkbox");
    await toggle.uncheck();
    await expect.poll(() => page.evaluate(() => Boolean((window as unknown as { finishMemorySave?: unknown }).finishMemorySave))).toBe(true);
    await expect(toggle).toBeDisabled();
    await expect(independent).toBeEnabled();
    await independent.uncheck();
    await page.evaluate(() => (window as unknown as { finishMemorySave: () => void }).finishMemorySave());
    await expect(toggle).toBeEnabled();
    await expect(toggle).toBeChecked({ checked: fails });
    await expect(independent).not.toBeChecked();
    await page.getByRole("tab", { name: "Claude Desktop", exact: true }).click();
    await page.getByRole("tab", { name: "ChatGPT", exact: true }).click();
    await expect(toggle).toBeChecked({ checked: fails });
    await expect(independent).not.toBeChecked();
  });
}
