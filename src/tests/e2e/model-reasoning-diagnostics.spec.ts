import { expect, test, type Page } from "../bun-playwright";
import { emitTauriEvent, installTauriMock } from "./tauri-mock";

async function openRules(page: Page, locale: "en" | "ru") {
  await page.goto("/");
  await page.getByRole("button", { name: locale === "ru" ? "Пул" : "Pool", exact: true }).click();
  await page.getByRole("tab", { name: locale === "ru" ? "Правила моделей" : "Model Rules", exact: true }).click();
}

async function mutationCommands(page: Page) {
  return page.evaluate(() => (window as unknown as {
    __TAURI_TEST_INVOKES__: Array<{ command: string }>;
  }).__TAURI_TEST_INVOKES__.map(({ command }) => command).filter((command) => /^(set_|refresh_|probe_)/.test(command)));
}

for (const locale of ["ru", "en"] as const) {
  for (const width of [320, 1160]) {
    test(`missing-level diagnostics are readable and read-only in ${locale} at ${width}px`, async ({ page }, testInfo) => {
      await page.setViewportSize({ width, height: 844 });
      await installTauriMock(page, {
        mode: "local", populated: true, locale, theme: width === 320 ? "light" : "dark",
        serverModelOrder: ["synthetic-unknown", "synthetic-toggle", "synthetic-budget", "synthetic-adaptive", "synthetic-supported", "synthetic-unsupported", "synthetic-mismatch"],
        modelMetadata: {
          "synthetic-toggle": { catalogReasoningMethod: "toggle" },
          "synthetic-budget": { catalogReasoningMethod: "budget_tokens" },
          "synthetic-adaptive": { catalogReasoningMethod: "adaptive" },
          "synthetic-supported": { catalogReasoning: true },
          "synthetic-unsupported": { catalogReasoning: false },
          "synthetic-mismatch": { catalogReasoning: true, catalogReasoningEffortLevels: ["low", "high"] },
        },
      });
      await openRules(page, locale);
      const before = await mutationCommands(page);
      const expected = locale === "ru" ? {
        unknown: "Это не означает, что размышление не поддерживается",
        toggle: "включение и выключение", budget: "бюджетом токенов", adaptive: "адаптивное размышление",
        supported: "список уровней не передан", unsupported: "явно сообщает", mismatch: "Причина расхождения",
      } : {
        unknown: "This does not mean reasoning is unsupported",
        toggle: "on/off control", budget: "token-budget control", adaptive: "adaptive reasoning",
        supported: "no confirmed level list", unsupported: "explicitly reports", mismatch: "does not report why",
      };
      for (const [suffix, text] of Object.entries(expected)) {
        const row = page.locator(`[data-model-id="synthetic-${suffix}"]`);
        await expect(row.locator("[data-model-reasoning-edit]")).toHaveCount(0);
        await row.locator("[data-model-reasoning-diagnostics]").click();
        const dialog = page.getByRole("dialog");
        await expect(dialog.getByRole("status")).toContainText(text);
        await expect(dialog.getByRole("checkbox")).toHaveCount(0);
        expect(await dialog.evaluate((element) => {
          const body = element.querySelector(".relay-dialog-body")!;
          return body.scrollWidth <= body.clientWidth + 1;
        })).toBe(true);
        if (suffix === "unknown") await dialog.screenshot({ path: testInfo.outputPath("unknown.png"), animations: "disabled" });
        await page.keyboard.press("Escape");
      }
      expect(await mutationCommands(page)).toEqual(before);
    });
  }
}

test("known levels keep diagnostics collapsed and reflect a saved empty policy", async ({ page }, testInfo) => {
  await installTauriMock(page, {
    mode: "local", populated: true, locale: "en", theme: "dark",
    serverModelOrder: ["synthetic-known", "gpt-image-synthetic"],
    modelReasoning: { "synthetic-known": ["low", "high"] },
    modelMetadata: {
      "synthetic-known": {
        catalogReasoning: true, catalogReasoningMethod: "effort", catalogSourceModelId: "synthetic/known",
        catalogReasoningEffortLevels: ["low", "high"],
        catalogCapabilities: { reasoningSource: "models_dev", reasoningEffortLevels: ["low", "high"], reasoningModes: [], serviceTiers: [], inputModalities: ["text"], outputModalities: ["text"] },
      },
    },
  });
  await openRules(page, "en");
  await expect(page.locator('[data-model-id="gpt-image-synthetic"] [data-model-reasoning-diagnostics]')).toHaveCount(0);
  await page.locator('[data-model-id="synthetic-known"] [data-model-reasoning-edit]').click();
  const dialog = page.getByRole("dialog");
  const details = dialog.locator(".model-reasoning-diagnostics");
  await expect(details).not.toHaveAttribute("open");
  const before = await mutationCommands(page);
  await details.locator("summary").click();
  await expect(details.getByText("models_dev", { exact: true })).toBeVisible();
  await expect(details.getByText("synthetic/known", { exact: true })).toBeVisible();
  expect(await mutationCommands(page)).toEqual(before);
  await dialog.getByRole("checkbox", { name: "Low", exact: true }).click();
  await dialog.getByRole("checkbox", { name: "High", exact: true }).click();
  await expect(dialog.getByRole("status")).toContainText("All known levels are excluded");
  await expect(dialog.getByRole("checkbox", { name: "Low", exact: true })).not.toBeChecked();
  await dialog.screenshot({ path: testInfo.outputPath("policy.png"), animations: "disabled" });
  await page.keyboard.press("Escape");
  await page.locator('[data-model-id="synthetic-known"] [data-model-reasoning-edit]').click();
  for (const option of await dialog.getByRole("checkbox").all()) await expect(option).not.toBeChecked();
});

test("an open diagnostic follows new metadata without reopening or saving policy", async ({ page }) => {
  await installTauriMock(page, { mode: "local", populated: true, locale: "en", serverModelOrder: ["synthetic-refresh"] });
  await openRules(page, "en");
  await page.locator('[data-model-id="synthetic-refresh"] [data-model-reasoning-diagnostics]').click();
  const dialog = page.getByRole("dialog");
  await expect(dialog.getByRole("status")).toContainText("This does not mean reasoning is unsupported");
  const before = await mutationCommands(page);
  // The mock IPC delivers a newer synthetic snapshot through the normal event path.
  await page.evaluate(() => {
    const tauri = (window as unknown as {
      __TAURI_INTERNALS__: { invoke: (command: string, args?: unknown) => Promise<unknown> };
    }).__TAURI_INTERNALS__;
    const invoke = tauri.invoke.bind(tauri);
    tauri.invoke = async (command, args) => {
      const result = await invoke(command, args);
      if (command === "get_local_runtime_state") {
        const runtime = result as { gateway: { models: Array<{ id: string }> } };
        const model = runtime.gateway.models.find((model) => model.id === "synthetic-refresh");
        Object.assign(model!, {
          catalogReasoning: true, catalogReasoningMethod: "effort", catalogReasoningEffortLevels: ["low", "high"],
          catalogSourceModelId: "synthetic/refresh", reasoningSupportedLevels: ["low", "high"],
          reasoningAllowedLevels: ["high"], reasoningLevels: ["high"], reasoningConfigurable: true,
        });
      }
      return result;
    };
  });
  await emitTauriEvent(page, "zenith-state-changed", null);
  await expect(dialog.getByRole("checkbox")).toHaveCount(2);
  await expect(dialog.getByRole("checkbox", { name: "High", exact: true })).toBeChecked();
  await expect(dialog.getByRole("checkbox", { name: "Low", exact: true })).not.toBeChecked();
  await dialog.locator(".model-reasoning-diagnostics summary").click();
  await expect(dialog.getByRole("status")).toContainText("Levels are known");
  await expect(dialog.getByText("synthetic/refresh", { exact: true })).toBeVisible();
  expect(await mutationCommands(page)).toEqual(before);
});
