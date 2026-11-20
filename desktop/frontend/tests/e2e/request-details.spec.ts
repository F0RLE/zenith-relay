import { expect, test, type Page } from "../bun-playwright";
import { installTauriMock } from "./tauri-mock";
import { completedUsage, noOutputUsage, receiptCacheShare, setUsageEventOverrides } from "./request-details-fixture";

const desktopModelAliases = [
  "claude-sonnet-5-relay-a1b2-c3d4-e5f6-7890",
  "claude-fable-5-relay-a1b2-c3d4-e5f6-7890",
  `claude-fable-5-zenith-${Array(32).fill("ab").join("-")}`,
];

for (const mode of ["local"] as const) {
  for (const alias of desktopModelAliases) {
    test(`${mode} keeps ${alias.split("-relay-")[0]} model aliases in technical details`, async ({ page }) => {
      await page.setViewportSize({ width: 390, height: 900 });
      await installTauriMock(page, {
        mode, locale: "en", populated: true,
        usageRequestedModel: alias, usageResolvedModel: "provider/model",
      });
      await page.goto("/");
      await setUsageEventOverrides(page, { ...completedUsage, wireApi: "messages" });
      await page.getByRole("button", { name: "Usage", exact: true }).click();
      await page.getByRole("button", { name: `Request details: req_synthetic_${mode}`, exact: true }).click();
      const dialog = page.getByRole("dialog", { name: "Request details" });
      await expect(dialog.getByRole("heading", { name: "provider/model", exact: true })).toBeVisible();
      await expect(dialog.locator(".request-details-parameters")).toHaveCount(0);
      await expect(dialog.getByText(alias, { exact: true })).toBeHidden();
      await dialog.locator(".request-technical-details > summary").click();
      await expect(dialog.locator(".request-details-parameters")).not.toContainText(alias);
      await expect(dialog.getByText(alias, { exact: true })).toBeVisible();
      await dialog.getByRole("button", { name: "Copy model ID", exact: true }).click();
      await expect.poll(() => page.evaluate(() => navigator.clipboard.readText())).toBe(alias);
      expect(await dialog.locator(".relay-dialog-body, .request-details-list dd")
        .evaluateAll((elements) => elements.every((element) => element.scrollWidth <= element.clientWidth + 1))).toBe(true);
    });
  }
}

test("unresolved Desktop alias does not impersonate a known model", async ({ page }) => {
  const alias = desktopModelAliases[1];
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, usageRequestedModel: alias });
  await page.goto("/");
  await setUsageEventOverrides(page, { ...noOutputUsage, resolvedModel: null, success: false, httpStatus: 404 });
  await page.getByRole("button", { name: "Usage", exact: true }).click();
  await page.getByRole("button", { name: "Request details: req_synthetic_local", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "Request details" });
  await expect(dialog.locator(".request-details-identity h3")).toHaveText("Unknown");
  await expect(dialog.getByText(alias, { exact: true })).toBeHidden();
  await dialog.locator(".request-technical-details > summary").click();
  await expect(dialog.getByText(alias, { exact: true })).toBeVisible();
  await expect(dialog.getByText("Sent to source", { exact: true })).toHaveCount(0);
});

async function openRequest(page: Page, theme: "dark" | "light" = "dark", failure = true) {
  await installTauriMock(page, {
    mode: "local", locale: "ru", theme, populated: true, diagnosticDebug: true, usageFailure: failure, usageCandidateKind: "source",
    usageRequestedModel: "gpt-6.1-sol", usageResolvedModel: "gpt-6.1-sol",
    usageAttemptOutcome: failure ? "retry_selection" : "completed",
    usageUpstreamError: failure ? {
      httpStatus: 503, code: "model_not_available", errorType: null, requestId: "req_provider_synthetic",
      message: "Synthetic provider: the selected model is temporarily unavailable.",
      redacted: false, truncated: false,
    } : undefined,
  });
  await page.goto("/");
  await setUsageEventOverrides(page, failure ? {
    ...noOutputUsage, latencyMs: 237, httpStatus: 503, errorCategory: "upstream_model_unavailable",
    requestedReasoningEffort: "max", effectiveReasoningEffort: "max",
    errorOrigin: "provider",
    toolUse: {
      clientToolCount: 80, forwardedToolCount: 80, toolChoice: "auto",
      toolCallCount: 0, textOutput: false, terminalOutput: "unknown",
    },
  } : completedUsage);
  await page.getByRole("button", { name: "Использование", exact: true }).click();
  await page.getByRole("button", { name: "Сведения о запросе: req_synthetic_local" }).click();
  return page.getByRole("dialog", { name: "Сведения о запросе" });
}

for (const theme of ["dark", "light"] as const) {
  for (const width of [390, 1160]) {
    test(`failed request exposes the message and parameters in ${theme} ${width}`, async ({ page }) => {
      await page.setViewportSize({ width, height: 900 });
      const dialog = await openRequest(page, theme);
      await expect(dialog.getByRole("heading", { name: "Модель временно недоступна" })).toBeVisible();
      await expect(dialog.getByText("HTTP 503", { exact: true })).toBeVisible();
      await expect(dialog.getByText("237 мс", { exact: true })).toBeVisible();
      await expect(dialog.getByRole("tab")).toHaveCount(0);
      await expect(dialog.getByText("req_synthetic_local", { exact: true })).toBeHidden();
      await expect(dialog.getByText("model_not_available", { exact: true })).toBeHidden();
      await expect(dialog.locator(".request-details-parameters, .request-token-details")).toHaveCount(0);
      await expect(receiptCacheShare(dialog)).toHaveCount(0);
      await expect(dialog.locator(".request-technical-details .form-note")).toBeHidden();
      await expect(dialog.locator(".request-execution")).toHaveCount(0);
      const bounds = await dialog.boundingBox();
      expect(bounds!.height).toBeLessThan(650);
      await page.mouse.move(0, 0);
      await expect(dialog.locator(".request-upstream-message pre")).toBeVisible();
      await dialog.screenshot({ path: `output/playwright/request-report-error-${theme}-${width}.png` });
      await dialog.getByRole("button", { name: "Копировать сообщение провайдера", exact: true }).click();
      await expect.poll(() => page.evaluate(() => navigator.clipboard.readText()))
        .toBe("Provider: Synthetic provider: the selected model is temporarily unavailable.");
      await dialog.locator(".request-technical-details > summary").click();
      await expect(dialog.locator(".request-id-list")).toBeVisible();
      await expect(dialog.getByText("Responses", { exact: true })).toBeVisible();
      await expect(dialog.getByText("HTTP", { exact: true })).toBeVisible();
      await expect(dialog.getByText("Максимальное", { exact: true })).toBeVisible();
      await expect(dialog.getByText("req_provider_synthetic", { exact: true })).toBeVisible();
      await expect(dialog.getByText("model_not_available", { exact: true })).toBeVisible();
      await expect(dialog.getByText("Инструменты клиента", { exact: true })).toHaveCount(0);
      await dialog.getByRole("button", { name: "Копировать ID запроса", exact: true }).click();
      await expect.poll(() => page.evaluate(() => navigator.clipboard.readText())).toBe("req_synthetic_local");
      expect(await dialog.locator(".relay-dialog-body, .request-details-list dd, .request-upstream-message")
        .evaluateAll((elements) => elements.every((element) => element.scrollWidth <= element.clientWidth + 1))).toBe(true);
      await page.mouse.move(0, 0);
      await dialog.screenshot({ path: `output/playwright/request-report-error-expanded-${theme}-${width}.png` });
    });

    test(`successful request shows measured cache share and support IDs in ${theme} ${width}`, async ({ page }) => {
      await page.setViewportSize({ width, height: 1000 });
      const dialog = await openRequest(page, theme, false);
      await expect(dialog.locator('.request-details-status[data-status="success"]')).toHaveAccessibleName("Успешно");
      await expect(dialog.locator('.request-details-status[data-status="success"]')).toHaveText("");
      await expect(dialog.locator(".request-details-error")).toHaveCount(0);
      await expect(dialog.locator(".request-execution")).toHaveCount(0);
      await expect(dialog.getByText("640 мс", { exact: true })).toBeVisible();
      await expect(dialog.locator(".request-details-metrics dt")).toHaveText([
        "Время ответа", "Первый вывод", "Скорость генерации", "Вход из кэша",
      ]);
      await expect(dialog.locator(".request-details-parameters")).toHaveCount(0);
      await expect(dialog.getByText("Обычные входные", { exact: true })).toBeVisible();
      await expect(dialog.getByText("Прочитано из кэша", { exact: true })).toBeVisible();
      await expect(dialog.getByText("Видимый вывод", { exact: true })).toBeVisible();
      await expect(dialog.getByText("Рассуждения", { exact: true })).toBeVisible();
      await expect(dialog.getByRole("table", { name: "Токены и стоимость" })).toBeVisible();
      await expect(dialog.locator(".request-ledger tfoot th")).toHaveText("Итого");
      await expect(dialog.locator(".request-token-total")).toHaveText(/19\s340/);
      await expect(dialog.locator(".request-tool-details")).toBeHidden();
      await expect(dialog.getByText("req_synthetic_local", { exact: true })).toBeHidden();
      expect(await dialog.locator(".relay-dialog-body, .request-details-parameters > div, .request-details-list dd")
        .evaluateAll((elements) => elements.every((element) => element.scrollWidth <= element.clientWidth + 1))).toBe(true);
      await page.mouse.move(0, 0);
      await dialog.screenshot({ path: `output/playwright/request-report-success-${theme}-${width}.png` });
      await dialog.locator(".relay-dialog-body").hover();
      await page.mouse.wheel(0, 1000);
      await expect(dialog.locator(".request-technical-details > summary")).toBeInViewport({ ratio: 1 });
      await dialog.locator(".request-technical-details > summary").click();
      await expect(receiptCacheShare(dialog).locator("dd")).toHaveText(/83,4\s*%/);
      const support = dialog.locator(".request-id-list");
      await expect(support).toContainText("req_synthetic_local");
      expect(await support.locator("dd")
        .evaluateAll((elements) => elements.every((element) => element.scrollWidth <= element.clientWidth + 1))).toBe(true);
      await dialog.screenshot({ path: `output/playwright/request-report-technical-${theme}-${width}.png` });
      await dialog.locator(".request-technical-details > summary").click();
      await expect(support).toHaveCount(0);
    });
  }
}

test("request disclosures are keyboard accessible and keep focus inside the dialog", async ({ page }) => {
  const dialog = await openRequest(page);
  const close = dialog.getByRole("button", { name: "Закрыть", exact: true });
  const technicalSummary = dialog.locator(".request-technical-details > summary");
  await close.focus();
  await page.keyboard.press("Shift+Tab");
  await expect(technicalSummary).toBeFocused();
  await page.keyboard.press("Enter");
  await expect(dialog.locator(".request-technical-details")).toHaveAttribute("open");
  await expect(dialog.getByRole("button", { name: "Копировать ID запроса", exact: true })).toBeVisible();
  await page.keyboard.press("Tab");
  await expect(dialog.getByRole("button", { name: "Копировать ID запроса", exact: true })).toBeFocused();
  await page.keyboard.press("Tab");
  await expect(dialog.getByRole("button", { name: "Копировать ID запроса провайдера", exact: true })).toBeFocused();
  await page.keyboard.press("Tab");
  await expect(close).toBeFocused();
  await page.keyboard.press("Tab");
  await expect(dialog.getByRole("button", { name: "Сохранить диагностику", exact: true })).toBeFocused();
  await page.keyboard.press("Tab");
  await expect(dialog.getByRole("button", { name: "Копировать сообщение провайдера", exact: true })).toBeFocused();
  await page.keyboard.press("Tab");
  await expect(technicalSummary).toBeFocused();
  await page.keyboard.press("Escape");
  await expect(dialog).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Сведения о запросе: req_synthetic_local" })).toBeFocused();
});
