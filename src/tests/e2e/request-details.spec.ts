import { expect, test, type Page } from "../bun-playwright";
import { installTauriMock } from "./tauri-mock";
import { completedUsage, noOutputUsage, setUsageEventOverrides } from "./request-details-fixture";

const desktopModelAliases = [
  "claude-sonnet-5-relay-a1b2-c3d4-e5f6-7890",
  "claude-fable-5-relay-a1b2-c3d4-e5f6-7890",
  `claude-fable-5-zenith-${Array(32).fill("ab").join("-")}`,
];

for (const locale of ["en", "ru"] as const) {
  for (const policy of [true, false, undefined]) {
    test(`BPS parallel diagnostics preserve ${String(policy)} policy in ${locale}`, async ({ page }) => {
      await page.setViewportSize({ width: 390, height: 900 });
      await installTauriMock(page, { mode: "local", locale, populated: true, diagnosticDebug: true });
      await page.goto("/");
      await setUsageEventOverrides(page, {
        ...completedUsage,
        routing: {
          reason: "only_eligible", eligibleCandidates: 1, quotaRemainingBasisPoints: null,
          inFlightBefore: 0, dispatchesBefore: 0, endpointKind: "excel_basis_points",
          transport: {
            upstream: "http", mode: "basis_points", responseHeadersMs: 0,
            ...(policy == null ? {} : {
              requestPreparationMs: policy ? 0 : 125,
              requestPreparation: {
                bodyMs: policy ? 0 : 65,
                schemaValidationMs: policy ? 0 : 40,
                catalogMs: policy ? 0 : 25,
                attachmentsMs: policy ? 0 : 30,
              },
              upstreamResponseMs: policy ? 0 : 250,
              clientFirstOutputMs: policy ? 0 : 500,
              clientResponseReadyMs: policy ? 0 : 750,
            }),
          },
        },
        toolUse: {
          clientToolCount: 2, forwardedToolCount: 2, toolChoice: "auto",
          toolCallCount: policy === true ? 2 : 0, textOutput: false, terminalOutput: "tool_call",
          requestedParallelToolCalls: policy,
          upstreamToolCallCount: policy == null ? undefined : 3,
          toolCallsReadyMs: policy === true ? 0 : undefined,
        },
      });
      await page.getByRole("button", { name: locale === "en" ? "Usage" : "Использование", exact: true }).click();
      await page.getByRole("button", {
        name: `${locale === "en" ? "Request details" : "Сведения о запросе"}: req_synthetic_local`, exact: true,
      }).click();
      const dialog = page.getByRole("dialog");
      await dialog.locator(".request-technical-details > summary").click();
      const parallel = dialog.locator("dl > div").filter({ has: page.locator("dt", {
        hasText: locale === "en" ? "Client explicitly allows parallel calls" : "Клиент явно разрешает параллельные вызовы",
      }) });
      if (policy == null) {
        await expect(parallel).toHaveCount(0);
      } else {
        await expect(parallel.locator("dd")).toHaveText(locale === "en" ? (policy ? "Yes" : "No") : (policy ? "Да" : "Нет"));
      }
      const ready = dialog.locator("dl > div").filter({ has: page.locator("dt", {
        hasText: locale === "en" ? "Validated calls ready after dispatch" : "Готовность проверенных вызовов после отправки",
      }) });
      if (policy === true) await expect(ready.locator("dd")).toHaveText(locale === "en" ? "0 ms" : "0 мс");
      else await expect(ready).toHaveCount(0);
      for (const [english, russian, milliseconds, prefix] of [
        ["Preparation before dispatch", "Подготовка до отправки", 125, ""],
        ["Upstream response collected", "Ответ источника собран", 250, "+"],
        ["First Relay output", "Первый вывод Relay", 500, "+"],
        ["Relay response ready", "Ответ Relay готов", 750, "+"],
        ["Request building", "Сборка запроса", 65, ""],
        ["Of which: schema checks", "В т. ч. проверка схем", 40, ""],
        ["Catalog preparation", "Подготовка каталога", 25, ""],
        ["Attachments", "Вложения", 30, ""],
      ] as const) {
        const stage = dialog.locator("dl > div").filter({ has: page.locator("dt", {
          hasText: locale === "en" ? english : russian,
        }) });
        if (policy == null) await expect(stage).toHaveCount(0);
        else await expect(stage.locator("dd")).toHaveText(`${prefix}${policy ? 0 : milliseconds} ${locale === "en" ? "ms" : "мс"}`);
      }
      if (policy === false) {
        await expect(dialog.locator(".request-timing-metrics dd")).toHaveText(
          [0, 250, 500, 750].map((milliseconds) => `+${milliseconds} ${locale === "en" ? "ms" : "мс"}`),
        );
      }
      await expect(dialog.locator(".request-technical-details p, .request-technical-details .form-note")).toHaveCount(0);
      expect(await dialog.locator(".request-details-list dd").evaluateAll((cells) =>
        cells.every((cell) => cell.scrollWidth <= cell.clientWidth + 1))).toBe(true);
    });
  }
}

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
      await expect(dialog.locator(".request-details-parameters")).not.toContainText(alias);
      await expect(dialog.getByText(alias, { exact: true })).toBeHidden();
      await dialog.locator(".request-technical-details > summary").click();
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
      await expect(dialog.getByText("Responses", { exact: true })).toBeVisible();
      await expect(dialog.getByText("HTTP", { exact: true })).toBeVisible();
      await expect(dialog.getByText("Максимальное", { exact: true })).toBeVisible();
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
      await expect(dialog.getByRole("region", { name: "Идентификаторы", exact: true })).toBeVisible();
      await expect(dialog.getByRole("region", { name: "Причина ошибки", exact: true })).toBeVisible();
      await expect(dialog.getByRole("region", { name: "Маршрут", exact: true })).toBeVisible();
      await expect(dialog.getByText("req_provider_synthetic", { exact: true })).toBeVisible();
      await expect(dialog.getByText("model_not_available", { exact: true })).toBeVisible();
      const toolDetails = dialog.getByRole("region", { name: "Инструменты", exact: true });
      await expect(toolDetails).toContainText("Инструменты клиента80");
      await expect(toolDetails).toContainText("Вызовы инструментов0");
      await expect(toolDetails).toContainText("Итоговый выводНеизвестно");
      await dialog.getByRole("button", { name: "Копировать ID запроса", exact: true }).click();
      await expect.poll(() => page.evaluate(() => navigator.clipboard.readText())).toBe("req_synthetic_local");
      expect(await dialog.locator(".relay-dialog-body, .request-details-list dd, .request-upstream-message")
        .evaluateAll((elements) => elements.every((element) => element.scrollWidth <= element.clientWidth + 1))).toBe(true);
      await page.mouse.move(0, 0);
      await dialog.screenshot({ path: `output/playwright/request-report-error-expanded-${theme}-${width}.png` });
    });

    test(`successful request shows tokens and keeps tools in technical details in ${theme} ${width}`, async ({ page }) => {
      await page.setViewportSize({ width, height: 1000 });
      const dialog = await openRequest(page, theme, false);
      await expect(dialog.locator(".request-details-identity")).toContainText("Успешно");
      await expect(dialog.locator(".request-details-error")).toHaveCount(0);
      await expect(dialog.locator(".request-execution")).toHaveCount(0);
      await expect(dialog.getByText("640 мс", { exact: true })).toBeVisible();
      await expect(dialog.locator(".request-details-metrics dt")).toHaveText([
        "Первый вывод", "Время ответа", "Скорость генерации", "Эквивалент API",
      ]);
      await expect(dialog.getByText("Responses", { exact: true })).toBeVisible();
      await expect(dialog.getByText("Обычные входные", { exact: true })).toBeVisible();
      await expect(dialog.getByText("Прочитано из кэша", { exact: true })).toBeVisible();
      await expect(dialog.getByText("Видимый вывод", { exact: true })).toBeVisible();
      await expect(dialog.getByText("Рассуждения", { exact: true })).toBeVisible();
      await expect(dialog.getByRole("region", { name: "Токены", exact: true })).toBeVisible();
      await expect(dialog.locator(".request-token-total dt")).toHaveText("Всего токенов");
      await expect(dialog.locator(".request-token-total dd")).toHaveText(/19\s340/);
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
      const toolDetails = dialog.locator(".request-technical-details .request-tool-details");
      await expect(toolDetails).toBeVisible();
      await expect(toolDetails).toContainText("Вызовы инструментов2");
      expect(await toolDetails.locator("dd")
        .evaluateAll((elements) => elements.every((element) => element.scrollWidth <= element.clientWidth + 1))).toBe(true);
      await dialog.screenshot({ path: `output/playwright/request-report-technical-${theme}-${width}.png` });
      await dialog.locator(".request-technical-details > summary").click();
      await expect(toolDetails).toBeHidden();
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
  await page.keyboard.press("Tab");
  await expect(dialog.getByRole("button", { name: "Копировать ID запроса", exact: true })).toBeFocused();
  await page.keyboard.press("Tab");
  await expect(dialog.getByRole("button", { name: "Копировать ID запроса провайдера", exact: true })).toBeFocused();
  await page.keyboard.press("Tab");
  await expect(close).toBeFocused();
  await page.keyboard.press("Tab");
  await expect(dialog.getByRole("button", { name: "Копировать сообщение провайдера", exact: true })).toBeFocused();
  await page.keyboard.press("Tab");
  await expect(technicalSummary).toBeFocused();
  await page.keyboard.press("Escape");
  await expect(dialog).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Сведения о запросе: req_synthetic_local" })).toBeFocused();
});
