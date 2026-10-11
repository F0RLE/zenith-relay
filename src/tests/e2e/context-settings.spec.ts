import { expect, test } from "../bun-playwright";
import { installTauriMock } from "./tauri-mock";
import { chooseOption, openGatewayApplication } from "./operations/helpers";

for (const theme of ["light", "dark"] as const) {
  test(`ChatGPT context settings fit a Russian narrow ${theme} window`, async ({ page }) => {
    await page.setViewportSize({ width: 390, height: 844 });
    await installTauriMock(page, { mode: "local", locale: "ru", theme, populated: true });
    await page.goto("/");
    await page.getByRole("button", { name: "Доступ", exact: true }).click();
    await page.getByRole("tab", { name: "Приложения", exact: true }).click();
    await page.getByRole("tab", { name: "ChatGPT", exact: true }).click();
    await expect(page.getByRole("tab", { name: "Контекст", exact: true })).toHaveCount(0);
    await chooseOption(page, page, "Режим контекста", "custom");
    const budget = page.getByRole("spinbutton", { name: "Бюджет, токенов" });
    await budget.fill("1000000");
    await budget.press("Enter");
    await expect(budget).toHaveValue("1000000");
    const panels = page.locator(".chatgpt-context-settings");
    await expect(panels).toHaveCount(2);
    for (const panel of await panels.all()) expect(await panel.evaluate((element) => {
      const rect = element.getBoundingClientRect();
      return rect.left >= 0 && rect.right <= innerWidth + 1
        && element.scrollWidth <= element.clientWidth;
    })).toBe(true);
    await page.screenshot({ path: `output/playwright/chatgpt-context-ru-${theme}-390.png` });
  });
}

test("ChatGPT context preferences persist separately and reject an invalid custom budget", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true });
  await page.goto("/");
  await openGatewayApplication(page);
  await expect(page.getByRole("tab", { name: "Context", exact: true })).toHaveCount(0);
  await expect(page.getByRole("tab", { name: "General", exact: true })).toHaveCount(0);
  await page.getByRole("button", { name: /^Context mode:/ }).click();
  await expect(page.getByRole("option")).toHaveText(["Default", "Normal · 272k", "Long · 1M", "Custom"]);
  await page.locator('[role="option"][data-value="short"]').click();
  await expect(page.getByRole("button", { name: "Context mode: Normal · 272k", exact: true })).toBeVisible();
  await expect(page.getByRole("spinbutton", { name: "Budget, tokens" })).toHaveCount(0);
  await chooseOption(page, page, "Context mode", "extended");
  await expect(page.getByRole("button", { name: "Context mode: Long · 1M", exact: true })).toBeVisible();
  await expect.poll(() => page.evaluate(() => (window as unknown as {
    __TAURI_TEST_INVOKES__: Array<{ command: string; args: { input: unknown } }>;
  }).__TAURI_TEST_INVOKES__.filter((call) => call.command === "set_local_chatgpt_context_policy").at(-1)?.args.input))
    .toEqual({ mode: "configured", context: { mode: "extended" }, compaction: { mode: "early" } });
  await chooseOption(page, page, "Context mode", "custom");
  const budget = page.getByRole("spinbutton", { name: "Budget, tokens" });
  await budget.fill("1000000");
  await budget.press("Enter");
  await expect.poll(() => page.evaluate(() => (window as unknown as {
    __TAURI_TEST_INVOKES__: Array<{ command: string; args: { input: unknown } }>;
  }).__TAURI_TEST_INVOKES__.filter((call) => call.command === "set_local_chatgpt_context_policy").at(-1)?.args.input))
    .toEqual({ mode: "configured", context: { mode: "custom", tokens: 1000000 }, compaction: { mode: "early" } });
  await budget.fill("10");
  await budget.press("Tab");
  await expect(budget).toHaveAttribute("aria-invalid", "true");
  await page.getByRole("tab", { name: "OpenCode", exact: true }).click();
  await page.getByRole("tab", { name: "ChatGPT", exact: true }).click();
  await expect(page.getByRole("spinbutton", { name: "Budget, tokens" })).toHaveValue("1000000");
  await page.screenshot({ path: "output/playwright/chatgpt-context-settings.png" });
});

test("context and auto compaction save independently and can return to defaults", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "ru", populated: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Доступ", exact: true }).click();
  await page.getByRole("tab", { name: "Приложения", exact: true }).click();
  await page.getByRole("tab", { name: "ChatGPT", exact: true }).click();
  await expect(page.getByRole("button", { name: "Режим контекста: По умолчанию", exact: true })).toBeVisible();
  await expect(page.getByRole("button", { name: "Режим автосжатия: Автоматически", exact: true })).toBeVisible();
  await page.getByRole("button", { name: "Режим контекста: По умолчанию", exact: true }).click();
  await expect(page.getByRole("option")).toHaveText(["По умолчанию", "Обычный · 272к", "Длинный · 1м", "Свой"]);
  await page.locator('[role="option"][data-value="short"]').click();
  await expect(page.getByRole("button", { name: "Режим контекста: Обычный · 272к", exact: true })).toBeVisible();
  await chooseOption(page, page, "Режим автосжатия", "custom");
  const compact = page.getByRole("spinbutton", { name: "Порог автосжатия, токенов", exact: true });
  await compact.fill("150000");
  await compact.press("Enter");
  await expect.poll(() => page.evaluate(() => (window as unknown as {
    __TAURI_TEST_INVOKES__: Array<{ command: string; args: { input: unknown } }>;
  }).__TAURI_TEST_INVOKES__.filter((call) => call.command === "set_local_chatgpt_context_policy").at(-1)?.args.input))
    .toEqual({ mode: "configured", context: { mode: "short" }, compaction: { mode: "custom", tokens: 150000 } });
  await page.screenshot({ path: "output/playwright/chatgpt-context-presets.png" });
  await chooseOption(page, page, "Режим контекста", "client");
  await expect(compact).toHaveValue("150000");
  await chooseOption(page, page, "Режим автосжатия", "client");
  await expect.poll(() => page.evaluate(() => (window as unknown as {
    __TAURI_TEST_INVOKES__: Array<{ command: string; args: { input: unknown } }>;
  }).__TAURI_TEST_INVOKES__.filter((call) => call.command === "set_local_chatgpt_context_policy").at(-1)?.args.input))
    .toEqual({ mode: "configured", context: { mode: "client" }, compaction: { mode: "client" } });
  await expect(compact).toHaveCount(0);
  await chooseOption(page, page, "Режим контекста", "extended");
  await chooseOption(page, page, "Режим автосжатия", "early");
  await expect(page.getByRole("button", { name: "Режим контекста: Длинный · 1м", exact: true })).toBeVisible();
  await page.screenshot({ path: "output/playwright/chatgpt-context-automatic.png" });
});

test("model pricing edits long Flex without changing the application context mode", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, sourceDetectedModelPrices: {
    "gpt-5.4": {
      inputMicroUsdPerMillion: 1_000_000, outputMicroUsdPerMillion: 2_000_000,
      above272k: { standard: { input: 2_000_000, output: 4_000_000 } },
    },
  } });
  await page.goto("/");
  await page.getByRole("button", { name: "Pool", exact: true }).click();
  await page.getByRole("button", { name: "Pool member policy: Example compatible API", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "Pool member policy", exact: true });
  await dialog.getByRole("tab", { name: "Pricing", exact: true }).click();
  const row = dialog.locator(".source-price-row").filter({ hasText: "gpt-5.4" }).first();
  await row.getByRole("textbox", { name: "Input token price for gpt-5.4", exact: true }).fill("1");
  await row.getByRole("textbox", { name: "Output token price for gpt-5.4", exact: true }).fill("2");
  await chooseOption(page, row, "gpt-5.4 price tier", "flex");
  await chooseOption(page, row, "gpt-5.4 context range", "above272k");
  await expect(row.getByRole("button", { name: "gpt-5.4 price tier: Flex", exact: true })).toBeVisible();
  await expect(row.getByRole("button", { name: "gpt-5.4 context range: Long · > 272k", exact: true })).toBeVisible();
  await row.getByRole("textbox", { name: "Input token price for gpt-5.4", exact: true }).fill("1.5");
  await row.getByRole("textbox", { name: "Output token price for gpt-5.4", exact: true }).fill("3");
  await dialog.screenshot({ path: "output/playwright/model-context-prices.png" });
  await dialog.getByRole("button", { name: "Save policy", exact: true }).click();
  await expect.poll(() => page.evaluate(() => (window as unknown as {
    __TAURI_TEST_INVOKES__: Array<{ command: string; args: { input?: { modelPriceOverrides?: Record<string, unknown> } } }>;
  }).__TAURI_TEST_INVOKES__.findLast((call) => call.command === "update_local_source")?.args.input?.modelPriceOverrides?.["gpt-5.4"]))
    .toMatchObject({ inputMicroUsdPerMillion: 1000000, outputMicroUsdPerMillion: 2000000, above272k: { flex: { input: 1500000, output: 3000000 } } });
  expect(await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string }> })
    .__TAURI_TEST_INVOKES__.some((call) => call.command === "set_local_chatgpt_context_policy"))).toBe(false);
});
