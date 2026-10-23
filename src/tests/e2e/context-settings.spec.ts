import { expect, test } from "../bun-playwright";
import { installTauriMock } from "./tauri-mock";
import { chooseOption } from "./operations/helpers";

test("model pricing edits long Flex without changing the application context mode", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, sourceDetectedModelPrices: {
    "gpt-5.4": {
      inputMicroUsdPerMillion: 1_000_000, outputMicroUsdPerMillion: 2_000_000,
      flex: { input: 500_000, output: 1_000_000 },
      above272k: {
        standard: { input: 2_000_000, output: 4_000_000 },
        flex: { input: 1_000_000, output: 2_000_000 },
      },
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
});

for (const mode of ["local"] as const) {
  test(`${mode} pool model rules preserve published Short/Long windows`, async ({ page }) => {
    await installTauriMock(page, {
      mode,
      locale: "en",
      populated: true,
      modelSpeed: { "gpt-5.4": "standard" },
      modelReasoning: { "gpt-5.4": ["low", "medium", "high"] },
      modelContext: {
        "gpt-5.4": { configurable: true, contextLimit: 900_000,
          windows: { defaultTokens: 200_000, maximumTokens: 900_000 }, policy: { mode: "short" } },
        "gpt-5.4-mini": { configurable: true },
      },
    });
    await page.goto("/");
    await page.getByRole("button", { name: "Pool", exact: true }).click();
    await page.getByRole("tab", { name: "Model Rules", exact: true }).click();

    const row = page.locator('[data-model-id="gpt-5.4"]');
    await expect(row.locator(".model-context-toggle")).toBeVisible();
    await expect(row.locator(".model-context-toggle [role=radio][aria-checked=true]")).toHaveAttribute("aria-label", "Short context");
    await expect(row.locator(".pool-context-token-value")).toHaveText("200,000");
    await expect(page.locator(".model-context-notice")).toHaveCount(0);
    await expect(row.locator(".pool-context-budget small")).toHaveCount(0);
    await expect(row.locator(".pool-context-budget")).toHaveAttribute("data-relay-tooltip", "Reference maximum: 900,000 tokens. Account-specific limits still apply.");
    const controlWidth = (await row.locator(".model-context-toggle").boundingBox())!.width;

    await row.getByRole("radio", { name: "Long context", exact: true }).click();
    {
      await expect.poll(() => page.evaluate(() => (window as unknown as {
        __TAURI_TEST_INVOKES__: Array<{ command: string; args: { input?: { modelId?: string; contextPolicy?: unknown } } }>;
      }).__TAURI_TEST_INVOKES__.findLast((call) => call.command === "set_local_model_context")?.args.input))
        .toEqual({ modelId: "gpt-5.4", contextPolicy: { mode: "extended" } });
    }
    await expect(row.getByRole("radio", { name: "Long context", exact: true })).toHaveAttribute("aria-checked", "true");
    await expect(row.locator(".pool-context-token-value")).toHaveText("900,000");
    expect((await row.locator(".model-context-toggle").boundingBox())!.width).toBe(controlWidth);
    await expect(row.locator(".model-context-toggle")).toHaveAttribute("aria-busy", "false");

    await row.getByRole("radio", { name: "Custom limit", exact: true }).click();
    const tokens = row.getByRole("spinbutton", { name: "Context limit, tokens", exact: true });
    expect((await row.locator(".model-context-toggle").boundingBox())!.width).toBe(controlWidth);
    await expect(tokens).toHaveAttribute("max", "900000");
    await tokens.fill("240000");
    // Typing applies the policy while the field still owns focus.
    await expect.poll(() => page.evaluate(() => {
      const calls = (window as unknown as {
        __TAURI_TEST_INVOKES__: Array<{ command: string; args: { input?: {
          modelId?: string; contextPolicy?: unknown;
          action?: { type?: string }; payload?: { contextPolicy?: unknown };
        } } }>;
      }).__TAURI_TEST_INVOKES__;
      return calls.findLast((call) => call.command === "set_local_model_context")?.args.input?.contextPolicy;
    })).toEqual({ mode: "custom", tokens: 240000 });
    await expect(tokens).toBeFocused();
    await expect(row.locator(".model-context-toggle")).toHaveAttribute("aria-busy", "false");
    await row.screenshot({ path: `output/playwright/model-rules-custom-${mode}.png` });
    await tokens.fill("24000");
    await tokens.press("Escape");
    await expect(tokens).toHaveValue("240000");
    await tokens.fill("900001");
    await expect(tokens).toHaveAttribute("aria-invalid", "true");
    await tokens.press("Escape");
    await expect(tokens).toHaveValue("240000");
    await row.getByRole("radio", { name: "Short context", exact: true }).click();
    await expect(row.locator(".model-context-toggle")).toHaveAttribute("aria-busy", "false");
    await expect(tokens).toHaveCount(0);
    await expect(row.locator(".pool-context-token-value")).toHaveText("200,000");
    await row.screenshot({ path: `output/playwright/model-rules-${mode}.png` });
  });
}

for (const theme of ["dark", "light"] as const) {
test(`model controls fit Russian labels and custom context at desktop and narrow widths (${theme})`, async ({ page }) => {
  await installTauriMock(page, {
    mode: "local",
    locale: "ru",
    theme,
    populated: true,
    modelSpeed: { "gpt-5.4": "fast" },
    modelReasoning: { "gpt-5.4": ["low", "high"] },
    modelContext: {
      "gpt-5.4": { configurable: true, contextLimit: 1_050_000, policy: { mode: "custom", tokens: 240000 } },
    },
  });
  await page.goto("/");
  await page.getByRole("button", { name: "Пул", exact: true }).click();
  await page.getByRole("tab", { name: "Правила моделей", exact: true }).click();
  const row = page.locator('[data-model-id="gpt-5.4"]');
  for (const width of [1160, 760, 390, 360]) {
    await page.setViewportSize({ width, height: 760 });
    await page.mouse.move(0, 0);
    const controls = row.locator(".pool-speed-control, .pool-context-control");
    await expect(controls).toHaveCount(2);
    for (const control of await controls.all()) {
      const box = await control.boundingBox();
      const rowBox = await row.boundingBox();
      expect(box).not.toBeNull();
      expect(rowBox).not.toBeNull();
      expect(box!.x).toBeGreaterThanOrEqual(rowBox!.x);
      expect(box!.x + box!.width).toBeLessThanOrEqual(rowBox!.x + rowBox!.width);
    }
    const tokens = row.getByRole("spinbutton", { name: "Лимит контекста, токенов", exact: true });
    await expect(tokens).toHaveValue("240000");
    const preview = row.locator(".pool-context-token-preview");
    await expect(preview).toHaveText("240\u00a0000");
    await expect(preview).toBeVisible();
    const controlBox = (await row.locator(".pool-context-control").boundingBox())!;
    const inputBox = (await tokens.boundingBox())!;
    const optionBox = (await row.getByRole("radio", { name: "Свой лимит", exact: true }).boundingBox())!;
    expect(inputBox.y).toBeGreaterThanOrEqual(controlBox.y);
    expect(inputBox.y + inputBox.height).toBeLessThanOrEqual(controlBox.y + controlBox.height);
    expect(inputBox.height).toBe(optionBox.height);
    expect(inputBox.y).toBe(optionBox.y);
    expect(await tokens.evaluate((input) => getComputedStyle(input).backgroundColor)).toBe("rgba(0, 0, 0, 0)");
    await page.locator(".model-rules").screenshot({ path: `output/playwright/model-rules-ru-${theme}-${width}.png` });
    await tokens.focus();
    await expect(preview).toBeHidden();
    await expect(tokens).toHaveValue("240000");
    await tokens.press("Tab");
    await expect(preview).toBeVisible();
  }
});
}

for (const mode of ["local"] as const) {
  test(`${mode} context saves serialize rapid changes and keep the final choice`, async ({ page }) => {
    await installTauriMock(page, { mode, locale: "en", populated: true, modelContextDelayMs: 1500,
      modelContext: { "gpt-5.4": { configurable: true, contextLimit: 1_000_000 } } });
    await page.goto("/");
    await page.getByRole("button", { name: "Pool", exact: true }).click();
    await page.getByRole("tab", { name: "Model Rules", exact: true }).click();
    const row = page.locator('[data-model-id="gpt-5.4"]');
    await row.getByRole("radio", { name: "Long context", exact: true }).click();
    await row.getByRole("radio", { name: "Custom limit", exact: true }).click();
    await row.getByRole("radio", { name: "Short context", exact: true }).click();
    await expect.poll(() => page.evaluate(() => (window as unknown as {
      __TAURI_TEST_INVOKES__: Array<{ command: string; args: { input?: { action?: { type?: string } } } }>;
    }).__TAURI_TEST_INVOKES__.filter((call) => call.command === "set_local_model_context"
      || call.args.input?.action?.type === "set_model_context").length)).toBe(2);
    await expect(row.locator(".model-context-toggle")).toHaveAttribute("aria-busy", "false");
    await expect(row.getByRole("radio", { name: "Short context", exact: true })).toBeChecked();
    await expect(row.locator(".pool-context-token-value")).toHaveText("250,000");
  });
}

test("context editor keeps deferred Codex reload quiet and does not guess unknown windows", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, catalogRefreshWarning: "deferred",
    modelContext: { "gpt-5.4": { configurable: true } } });
  await page.goto("/");
  await page.getByRole("button", { name: "Pool", exact: true }).click();
  await page.getByRole("tab", { name: "Model Rules", exact: true }).click();
  await expect(page.locator(".model-context-status")).toHaveCount(0);
  await expect(page.locator(".model-context-notice")).toHaveCount(0);
  const row = page.locator('[data-model-id="gpt-5.4"]');
  await expect(row.locator(".pool-context-token-value")).toHaveText("Unknown");
  await expect(row.locator(".pool-context-budget")).toHaveAttribute("data-relay-tooltip", "Maximum not published");
});

test("failed custom autosave restores the confirmed value without a retry loop", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, modelContextFailOnce: true,
    modelContext: { "gpt-5.4": { configurable: true, contextLimit: 1_000_000, policy: { mode: "custom", tokens: 240000 } } } });
  await page.goto("/");
  await page.getByRole("button", { name: "Pool", exact: true }).click();
  await page.getByRole("tab", { name: "Model Rules", exact: true }).click();
  const control = page.locator('[data-model-id="gpt-5.4"] .model-context-toggle');
  const tokens = control.getByRole("spinbutton");
  await tokens.fill("250000");
  await expect.poll(() => page.evaluate(() => (window as unknown as {
    __TAURI_TEST_INVOKES__: Array<{ command: string }>;
  }).__TAURI_TEST_INVOKES__.filter((call) => call.command === "set_local_model_context").length)).toBe(1);
  await expect(control).toHaveAttribute("aria-busy", "false");
  await expect(tokens).toBeFocused();
  await expect(tokens).toHaveValue("240000");
  await page.waitForTimeout(800); // More than the debounce: failed drafts must not resubmit.
  expect(await page.evaluate(() => (window as unknown as {
    __TAURI_TEST_INVOKES__: Array<{ command: string }>;
  }).__TAURI_TEST_INVOKES__.filter((call) => call.command === "set_local_model_context").length)).toBe(1);
});

for (const defaultTokens of [undefined, 200_000]) {
  test(`resolved native windows do not borrow the API maximum (${defaultTokens ?? "unknown"})`, async ({ page }) => {
    await installTauriMock(page, { mode: "local", locale: "en", populated: true,
      catalogRefreshWarning: "failed",
      modelContext: { "gpt-5.4": { configurable: true, contextLimit: 1_050_000,
        windows: defaultTokens === undefined ? {} : { defaultTokens } } } });
    await page.goto("/");
    await page.getByRole("button", { name: "Pool", exact: true }).click();
    await page.getByRole("tab", { name: "Model Rules", exact: true }).click();
    await expect(page.locator(".model-context-status")).toHaveCount(0);
    const row = page.locator('[data-model-id="gpt-5.4"]');
    const expected = defaultTokens === undefined ? "Unknown" : "200,000";
    await expect(row.locator(".pool-context-token-value")).toHaveText(expected);
    await row.getByRole("radio", { name: "Long context", exact: true }).click();
    await expect(row.locator(".pool-context-token-value")).toHaveText(expected);
    await row.getByRole("radio", { name: "Custom limit", exact: true }).click();
    const tokens = row.getByRole("spinbutton");
    await expect(tokens).toHaveAttribute("max", "2000000");
    await tokens.fill("1500000");
    await expect(tokens).toHaveAttribute("aria-invalid", "false");
    await page.locator(".model-rules").screenshot({ path: `output/playwright/context-status-${defaultTokens ?? "unknown"}.png` });
  });
}
