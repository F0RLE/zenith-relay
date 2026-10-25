import { expect, test } from "../bun-playwright";
import { installTauriMock } from "./tauri-mock";
import { completedUsage, setUsageEventOverrides } from "./request-details-fixture";

for (const continuation of [false, true]) {
  test(`technical payload details show ${continuation ? "response continuation" : "compression"}`, async ({ page }) => {
    await page.setViewportSize({ width: 390, height: 900 });
    await installTauriMock(page, { mode: "local", locale: "en", populated: true, diagnosticDebug: true });
    await page.goto("/");
    await setUsageEventOverrides(page, {
      ...completedUsage,
      routing: {
        reason: "only_eligible", eligibleCandidates: 1, inFlightBefore: 0, dispatchesBefore: 0,
        transport: {
          upstream: continuation ? "websocket" : "http", mode: "native",
          ...(continuation ? { connectionReused: true } : {}),
          requestPayload: {
            mode: continuation ? "continuation" : "full",
            ...(!continuation ? { fullRequestReason: "no_previous_response_id", compressionMs: 0, compressionUs: 420, httpSendToHeadersMs: 12 } : {}),
            jsonBytes: 100000, bodyBytes: continuation ? 100000 : 2000,
            contentEncoding: continuation ? "identity" : "zstd",
          },
        },
      },
    });
    await page.getByRole("button", { name: "Usage", exact: true }).click();
    await page.getByRole("button", { name: "Request details: req_synthetic_local", exact: true }).click();
    const dialog = page.getByRole("dialog");
    await dialog.locator(".request-technical-details > summary").click();
    const route = dialog.locator(".request-route-section");
    await expect(route.getByText(continuation ? "Response ID continuation" : "Full request", { exact: true })).toBeVisible();
    await expect(route.getByText("Why a full request", { exact: true })).toHaveCount(continuation ? 0 : 1);
    await expect(route.getByText(continuation ? "Reused" : "Zstd", { exact: true })).toBeVisible();
    if (!continuation) await expect(route.getByText("0 ms", { exact: true })).toBeVisible();
    await expect(route.getByText("HTTP send → headers", { exact: true })).toHaveCount(continuation ? 0 : 1);
    if (!continuation) await expect(route.getByText("12 ms", { exact: true })).toBeVisible();
    expect(await route.locator("dd").evaluateAll((fields) => fields.every((field) => field.scrollWidth <= field.clientWidth + 1))).toBe(true);
    await route.scrollIntoViewIfNeeded();
    await page.screenshot({ path: `output/playwright/technical-payload-${continuation ? "continuation" : "compressed"}.png` });
    await page.keyboard.press("Escape");
  });
}

for (const locale of ["ru", "en"] as const) {
  for (const theme of ["light", "dark"] as const) {
    for (const width of [390, 1160]) {
      test(`technical details group diagnostics without clipping: ${locale}, ${theme}, ${width}`, async ({ page }) => {
        await page.setViewportSize({ width, height: 900 });
        await installTauriMock(page, { mode: "local", locale, theme, populated: true, diagnosticDebug: true });
        await page.goto("/");
        await setUsageEventOverrides(page, {
          ...completedUsage,
          transport: "websocket",
          routing: {
            reason: "quota_headroom", eligibleCandidates: 4, quotaRemainingBasisPoints: 7240,
            inFlightBefore: 1, dispatchesBefore: 12, endpointKind: "excel_basis_points",
            upstreamResponseModel: "synthetic-provider-model",
            transport: {
              upstream: "http", mode: "basis_points", fallbackReason: "basis_points_http", responseFormat: "sse",
              responseHeadersMs: 240, requestPreparationMs: 125,
              requestPayload: { mode: "full", fullRequestReason: "basis_points_transport",
                jsonBytes: 59000, bodyBytes: 59000, contentEncoding: "identity", httpSendToHeadersMs: 200 },
              requestPreparation: { bodyMs: 65, schemaValidationMs: 0, catalogMs: 25, attachmentsMs: 30 },
              upstreamResponseMs: 7040, clientFirstOutputMs: 640, clientResponseReadyMs: 7200,
            },
          },
          toolUse: {
            ...completedUsage.toolUse,
            clientToolCount: 291, forwardedToolCount: 291, requestedParallelToolCalls: true,
            upstreamToolCallCount: 1, toolCallsReadyMs: 7200,
            clientContext: { bodyBytes: 400000, instructionBytes: 40000, conversationBytes: 20, toolCatalogBytes: 340000 },
            upstreamContext: { bodyBytes: 59000, instructionBytes: 58000, conversationBytes: 20, toolCatalogBytes: 0 },
            basisPointsCatalog: {
              mode: "discovery", fullSchemaBytes: 340000, renderedSchemaBytes: 3000, catalogPromptBytes: 18000,
              loadedToolCount: 3, discoveryRounds: 1, discoveryLatencyMs: 620, discoveryInputTokens: 4000, discoveryOutputTokens: 20,
            },
          },
        });
        await page.getByRole("button", { name: locale === "ru" ? "Использование" : "Usage", exact: true }).click();
        await page.getByRole("button", {
          name: `${locale === "ru" ? "Сведения о запросе" : "Request details"}: req_synthetic_local`, exact: true,
        }).click();
        const dialog = page.getByRole("dialog");
        const technical = dialog.locator(".request-technical-details");
        await technical.locator(":scope > summary").click();
        await expect(technical.locator("p, .form-note")).toHaveCount(0);

        const timings = technical.getByRole("region", { name: locale === "ru" ? "Время выполнения" : "Timing", exact: true });
        await expect(timings).toBeVisible();
        await expect(timings.locator(".request-preparation-total dd")).toHaveText(locale === "ru" ? "125 мс" : "125 ms");
        await expect(timings.locator(".request-timing-metrics dt")).toHaveText(locale === "ru"
          ? ["Заголовки ответа", "Первый вывод Relay", "Ответ источника собран", "Ответ Relay готов"]
          : ["Response headers", "First Relay output", "Upstream response collected", "Relay response ready"]);
        await expect(timings.locator(".request-timing-metrics dd")).toHaveText(locale === "ru"
          ? ["+240 мс", "+640 мс", "+7 с", "+7,2 с"] : ["+240 ms", "+640 ms", "+7 s", "+7.2 s"]);
        await expect(timings.locator(".request-preparation-details .request-technical-fields dd")).toHaveText(
          locale === "ru" ? ["65 мс", "0 мс", "25 мс", "30 мс"] : ["65 ms", "0 ms", "25 ms", "30 ms"],
        );
        await expect(technical.locator(".request-route-section .request-timing-metrics")).toHaveCount(0);
        await expect(technical.getByText(locale === "ru" ? "Полный запрос" : "Full request", { exact: true })).toBeVisible();
        await expect(technical.getByText(locale === "ru" ? "Транспорт BPS требует полный контекст" : "BPS transport requires full context", { exact: true })).toBeVisible();
        await expect(technical.getByText(locale === "ru" ? "Отправка HTTP → заголовки" : "HTTP send → headers", { exact: true })).toBeVisible();
        const preparation = await timings.locator(".request-preparation-details").boundingBox();
        const response = await timings.locator(".request-response-timings").boundingBox();
        expect(preparation!.y + preparation!.height).toBeLessThan(response!.y);
        const metrics = await timings.locator(".request-timing-metrics > div").evaluateAll((elements) =>
          elements.map((element) => {
            const { left, right, top } = element.getBoundingClientRect();
            return { left, right, top };
          }));
        const rows = [...new Set(metrics.map((metric) => metric.top))];
        expect(rows).toHaveLength(width < 620 ? 2 : 1);
        for (const top of rows) {
          const row = metrics.filter((metric) => metric.top === top);
          expect(row[0].left).toBeCloseTo(metrics[0].left, 0);
          expect(row.at(-1)!.right).toBeCloseTo(response!.x + response!.width, 0);
        }
        await expect(technical.locator(".request-technical-grid > section:last-child")).toHaveClass(/request-identifiers-section/);
        expect(await technical.locator("section, dl, dd").evaluateAll((elements) =>
          elements.every((element) => element.scrollWidth <= element.clientWidth + 1))).toBe(true);
        const cards = await technical.locator(".request-technical-grid > section").evaluateAll((elements) =>
          elements.map((element) => ({ left: element.getBoundingClientRect().left, width: element.getBoundingClientRect().width })));
        expect(cards.every((card) => Math.abs(card.left - cards[0].left) < 1 && Math.abs(card.width - cards[0].width) < 1)).toBe(true);

        await timings.scrollIntoViewIfNeeded();
        await page.screenshot({ path: `output/playwright/technical-details-${locale}-${theme}-${width}.png` });
        await technical.getByRole("button", { name: locale === "ru" ? "Копировать ID запроса" : "Copy request ID", exact: true }).click();
        await expect.poll(() => page.evaluate(() => navigator.clipboard.readText())).toBe("req_synthetic_local");
        await page.keyboard.press("Escape");
        await expect(dialog).toHaveCount(0);
      });
    }
  }
}

for (const scenario of [
  {
    name: "partial response",
    transport: { upstream: "http", mode: "native", responseHeadersMs: 0, clientFirstOutputMs: 300, clientResponseReadyMs: 900 },
    preparation: [], response: ["+0 ms", "+300 ms", "+900 ms"],
  },
  {
    name: "preparation without a measured total",
    transport: { upstream: "http", mode: "native", requestPreparation: { bodyMs: 0, schemaValidationMs: 0 } },
    preparation: ["—"], response: [],
  },
] as const) {
  test(`technical timing preserves ${scenario.name} without empty sections or gaps`, async ({ page }) => {
    await page.setViewportSize({ width: 390, height: 900 });
    await installTauriMock(page, { mode: "local", locale: "en", populated: true, diagnosticDebug: true });
    await page.goto("/");
    await setUsageEventOverrides(page, {
      ...completedUsage,
      routing: { reason: "only_eligible", eligibleCandidates: 1, inFlightBefore: 0, dispatchesBefore: 0, transport: scenario.transport },
    });
    await page.getByRole("button", { name: "Usage", exact: true }).click();
    await page.getByRole("button", { name: "Request details: req_synthetic_local", exact: true }).click();
    const dialog = page.getByRole("dialog");
    await dialog.locator(".request-technical-details > summary").click();
    const timings = dialog.locator(".request-timings-section");
    await expect(timings.locator(".request-preparation-total dd")).toHaveText([...scenario.preparation]);
    await expect(timings.locator(".request-timing-metrics dd")).toHaveText([...scenario.response]);
    await expect(timings.locator(".request-response-timings")).toHaveCount(scenario.response.length ? 1 : 0);
    if (scenario.response.length) {
      const metrics = await timings.locator(".request-timing-metrics").boundingBox();
      const last = await timings.locator(".request-timing-metrics > div").last().boundingBox();
      expect(last!.width).toBeCloseTo(metrics!.width, 0);
    }
  });
}
