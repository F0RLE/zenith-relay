import { expect, test } from "../bun-playwright";
import { installTauriMock } from "./tauri-mock";
import { setUsageEventOverrides } from "./request-details-fixture";

for (const width of [390, 1160]) {
  test(`context sizes explain a large tool catalog without a false warning at ${width}`, async ({ page }) => {
    await page.setViewportSize({ width, height: 1000 });
    await installTauriMock(page, { mode: "local", locale: "ru", populated: true, diagnosticDebug: true });
    await page.goto("/");
    await setUsageEventOverrides(page, {
      transport: "websocket",
      routing: {
        reason: "quota_headroom", eligibleCandidates: 1, inFlightBefore: 0, dispatchesBefore: 0, endpointKind: "excel_basis_points",
        transport: { upstream: "http", mode: "basis_points", fallbackReason: "basis_points_http", responseFormat: "sse" },
      },
      toolUse: {
        clientToolCount: 291, forwardedToolCount: 291, clientSchemaBytes: 340000, forwardedSchemaBytes: 340000,
        toolChoice: "auto", toolCallCount: 0, textOutput: true, terminalOutput: "text",
        clientContext: { bodyBytes: 400000, instructionBytes: 40000, conversationBytes: 20, toolCatalogBytes: 340000 },
        upstreamContext: { bodyBytes: 390000, instructionBytes: 389000, conversationBytes: 20, toolCatalogBytes: 0 },
        basisPointsCatalog: { mode: "discovery", fullSchemaBytes: 340000, renderedSchemaBytes: 3000, catalogPromptBytes: 18000, loadedToolCount: 3, discoveryRounds: 1, discoveryLatencyMs: 14674, discoveryInputTokens: 4000, discoveryOutputTokens: 20 },
      },
    });
    await page.getByRole("button", { name: "Использование", exact: true }).click();
    await page.getByRole("button", { name: "Сведения о запросе: req_synthetic_local" }).click();
    const dialog = page.getByRole("dialog", { name: "Сведения о запросе" });
    await dialog.locator(".request-technical-details > summary").click();
    await expect(dialog.getByText("WebSocket → HTTP", { exact: true })).toBeVisible();
    await expect(dialog.getByText("Basis Points использует HTTP", { exact: true })).toBeVisible();
    await expect(dialog.getByText("SSE", { exact: true })).toBeVisible();
    await expect(dialog.getByText("Подключение WebSocket", { exact: true })).toHaveCount(0);
    await expect(dialog.getByText("Контекст", { exact: true })).toBeVisible();
    await expect(dialog.locator(".request-context-section p, .request-context-section .form-note")).toHaveCount(0);
    await expect(dialog.getByText("Схемы по запросу", { exact: true })).toBeVisible();
    await expect(dialog.getByText("3 / 291", { exact: true })).toBeVisible();
    await expect(dialog.getByText("4000 / 20", { exact: true })).toBeVisible();
    const lookupTime = dialog.locator(".request-details-list > div").filter({ has: page.getByText("Время загрузки схем", { exact: true }) });
    await expect(lookupTime.locator("dd")).toHaveText("14,7 с");
    await expect(dialog.getByText(/Итоговый ответ буферизуется/)).toHaveCount(0);
    await expect(dialog.locator(".request-tool-details .warning-text")).toHaveCount(0);
    const overflow = await dialog.locator(".request-details-list dd").evaluateAll((values) => values.some((value) => value.scrollWidth > value.clientWidth + 1));
    expect(overflow).toBe(false);
    await dialog.locator(".request-context-section").scrollIntoViewIfNeeded();
    await page.screenshot({ path: `output/playwright/request-context-${width}.png` });
  });
}

for (const measured of [false, true]) {
  test(`transport details distinguish old records from a measured zero handshake: ${measured}`, async ({ page }) => {
    await installTauriMock(page, { mode: "local", locale: "ru", populated: true, diagnosticDebug: true });
    await page.goto("/");
    await setUsageEventOverrides(page, {
      transport: "websocket",
      routing: {
        reason: "only_eligible", eligibleCandidates: 1, inFlightBefore: 0, dispatchesBefore: 0,
        ...(measured ? { transport: { upstream: "websocket", mode: "native", responseFormat: "websocket_frames", handshakeMs: 0 } } : {}),
      },
    });
    await page.getByRole("button", { name: "Использование", exact: true }).click();
    await page.getByRole("button", { name: "Сведения о запросе: req_synthetic_local" }).click();
    const dialog = page.getByRole("dialog", { name: "Сведения о запросе" });
    await dialog.locator(".request-technical-details > summary").click();
    await expect(dialog.getByText("Приложение → провайдер", { exact: true })).toHaveCount(measured ? 1 : 0);
    await expect(dialog.getByText("Подключение WebSocket", { exact: true })).toHaveCount(measured ? 1 : 0);
    if (measured) {
      await expect(dialog.getByText("WebSocket → WebSocket", { exact: true })).toBeVisible();
      const handshake = dialog.locator(".request-route-section .request-details-list > div").filter({ has: page.getByText("Подключение WebSocket", { exact: true }) });
      await expect(handshake.locator("dd")).toHaveText("0 мс");
    }
    await expect(dialog.locator(".request-timings-section")).toHaveCount(0);
    await expect(dialog.getByText("Почему используется HTTP", { exact: true })).toHaveCount(0);
    await expect(dialog.getByText("Время загрузки схем", { exact: true })).toHaveCount(0);
  });
}
