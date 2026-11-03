import { expect, test, type Locator } from "../bun-playwright";
import type { CacheContextDiagnostics, RoutingDiagnostics, ToolUseDiagnostics } from "../../src/features/relay/api/types";
import { installTauriMock } from "./tauri-mock";
import { setUsageEventOverrides } from "./request-details-fixture";

const cacheContext: CacheContextDiagnostics = {
  baseline: "completed_request", scope: "client_session", candidateChanged: false, previousCompletedAgeMs: 12000,
  clientChanges: ["tools"], upstreamChanges: ["tools"], relayChanges: [], provider: { type: "unavailable" },
  clientHistory: { comparison: "appended", inputItems: 12, inputBytes: 57000, sharedPrefixItems: 10, firstChangedItemKind: null },
  upstreamHistory: { comparison: "rewritten", inputItems: 13, inputBytes: 57000, sharedPrefixItems: 4, firstChangedItemKind: "additional_tools" },
  relayHistory: { comparison: "unchanged", inputItems: 12, inputBytes: 57000, sharedPrefixItems: 12, firstChangedItemKind: null },
};

const diagnostics = {
  requestedModel: `claude-fable-5-relay-${"synthetic-".repeat(6)}model`,
  resolvedModel: "synthetic-reasoning-model",
  routing: {
    reason: "quota_headroom", eligibleCandidates: 4, quotaRemainingBasisPoints: 6300, inFlightBefore: 0, dispatchesBefore: 3,
    upstreamResponseModel: "synthetic-provider-model-2026", cacheContext,
    transport: {
      upstream: "http", mode: "native", responseFormat: "sse", connectionReused: true, handshakeMs: 0,
      requestPreparationMs: 12, requestPreparation: { bodyMs: 4, schemaValidationMs: 0, catalogMs: 6, attachmentsMs: 2 },
      responseHeadersMs: 20, clientFirstOutputMs: 128, upstreamResponseMs: 420, clientResponseReadyMs: 428,
      requestPayload: {
        mode: "full", fullRequestReason: "no_previous_response_id", jsonBytes: 385120, bodyBytes: 48200,
        contentEncoding: "zstd", compressionMs: 2, httpSendToHeadersMs: 18,
      },
    },
  },
  toolUse: {
    clientToolCount: 311, forwardedToolCount: 311, toolCallCount: 3, toolChoice: "auto", textOutput: true,
    terminalOutput: "mixed", requestedParallelToolCalls: false, upstreamToolCallCount: 4, toolCallsReadyMs: 236,
    clientContext: { bodyBytes: 385120, instructionBytes: 28100, conversationBytes: 57000, toolCatalogBytes: 300000 },
    upstreamContext: { bodyBytes: 98200, instructionBytes: 28100, conversationBytes: 57000, toolCatalogBytes: 13100 },
    basisPointsCatalog: {
      mode: "discovery", fullSchemaBytes: 300000, renderedSchemaBytes: 13100, catalogPromptBytes: 1200,
      loadedToolCount: 8, discoveryRounds: 1, discoveryInputTokens: 2510, discoveryOutputTokens: 52, discoveryLatencyMs: 82,
    },
  },
} satisfies { requestedModel: string; resolvedModel: string; routing: RoutingDiagnostics; toolUse: ToolUseDiagnostics };

async function expectNoClipping(dialog: Locator) {
  const overflow = await dialog.locator(".relay-dialog-body, .request-technical-details, .request-technical-grid, .request-detail-section, dt, dd, code").evaluateAll((elements) => elements
    .filter((element) => element.getBoundingClientRect().width > 0 && element.clientWidth > 0)
    .filter((element) => element.scrollWidth > element.clientWidth + 1)
    .map((element) => ({ tag: element.tagName, class: element.className, width: element.clientWidth, scroll: element.scrollWidth,
      children: Array.from(element.children).map((child) => ({ tag: child.tagName, width: child.clientWidth, scroll: child.scrollWidth, left: child.getBoundingClientRect().left, right: child.getBoundingClientRect().right })) })));
  expect(overflow).toEqual([]);
}

for (const scenario of [
  { width: 1160, locale: "ru", theme: "dark" },
  { width: 1160, locale: "en", theme: "light" },
  { width: 840, locale: "ru", theme: "dark" },
  { width: 390, locale: "ru", theme: "dark" },
] as const) {
  test(`technical request cards ${scenario.width} ${scenario.locale} ${scenario.theme}`, async ({ page }) => {
    await page.setViewportSize({ width: scenario.width, height: 900 });
    await installTauriMock(page, { mode: "local", populated: true, diagnosticDebug: true, ...scenario });
    await page.goto("/");
    await setUsageEventOverrides(page, diagnostics);
    await page.getByRole("button", { name: scenario.locale === "ru" ? "Использование" : "Usage", exact: true }).click();
    await page.getByRole("button", { name: /req_synthetic_local/ }).click();
    const dialog = page.getByRole("dialog");
    const details = dialog.locator(".request-technical-details");
    await expect(details).not.toHaveAttribute("open");
    await expect(details).toHaveCSS("border-top-width", "1px");
    await expect(details).toHaveCSS("border-radius", "12px");
    await expect(details.locator(":scope > summary")).toHaveCSS("min-height", scenario.width > 620 ? "54px" : "50px");
    await expect(details.locator(".request-disclosure-icon")).toHaveCSS("width", "28px");
    const overview = dialog.locator(".request-overview");
    const before = await overview.evaluate((element) => ({ width: element.clientWidth, height: element.clientHeight }));
    await details.locator(":scope > summary").click();
    await expect(details.locator(".request-payload-section")).toBeVisible();
    expect(await overview.evaluate((element) => ({ width: element.clientWidth, height: element.clientHeight }))).toEqual(before);
    await expect(details.locator(".request-technical-grid > section")).toHaveCount(6);
    await expect(details.locator(".request-payload-section")).toContainText("Zstd");
    await expect(details.locator(".request-preparation-details")).toContainText(scenario.locale === "ru" ? "0 мс" : "0 ms");
    await expect(details.locator(".request-timing-metrics dd")).toHaveText(
      [20, 128, 420, 428].map((value) => `+${value} ${scenario.locale === "ru" ? "мс" : "ms"}`),
    );
    await expect(details.locator(".request-context-section")).toContainText("8 / 311");
    await expect(details.locator(".request-tool-details")).toContainText("311");
    const route = await details.locator(".request-route-section").boundingBox();
    const payload = await details.locator(".request-payload-section").boundingBox();
    if (scenario.width > 620) {
      expect(Math.abs(route!.y - payload!.y)).toBeLessThan(1);
      expect(payload!.x).toBeGreaterThan(route!.x + route!.width);
    } else {
      expect(payload!.y).toBeGreaterThanOrEqual(route!.y + route!.height);
    }
    await expectNoClipping(dialog);
    await details.locator(":scope > summary").evaluate((element) => element.scrollIntoView({ block: "start" }));
    await page.mouse.move(0, 0);
    await dialog.screenshot({ path: `output/playwright/request-technical-${scenario.locale}-${scenario.width}.png` });
    await expectNoClipping(dialog);
    const ids = details.locator(".request-identifiers-section");
    await ids.scrollIntoViewIfNeeded();
    await ids.getByRole("button", { name: scenario.locale === "ru" ? "Копировать ID запроса" : "Copy request ID", exact: true }).click();
    await expect.poll(() => page.evaluate(() => navigator.clipboard.readText())).toBe("req_synthetic_local");
    await expectNoClipping(dialog);
    await dialog.screenshot({ path: `output/playwright/request-technical-identifiers-${scenario.locale}-${scenario.width}.png` });
    const comparison = details.locator(".request-context-details");
    await expect(details.locator("details")).toHaveCount(0);
    await expect(comparison.locator("summary")).toHaveCount(0);
    await expect(comparison.getByRole("heading", { name: scenario.locale === "ru" ? "Сравнение контекста" : "Context comparison", exact: true })).toBeVisible();
    await expect(comparison.locator(".request-cache-context")).toBeVisible();
    await expect(comparison).toContainText(scenario.locale === "ru" ? "Провайдер не предоставил сравнение" : "Provider comparison unavailable");
    await expectNoClipping(dialog);
    await comparison.scrollIntoViewIfNeeded();
    await dialog.screenshot({ path: `output/playwright/request-technical-cache-${scenario.locale}-${scenario.width}.png` });
    await details.locator(":scope > summary").focus();
    await page.keyboard.press("Enter");
    await expect(details).not.toHaveAttribute("open");
    await expect(details.locator(".request-technical-grid")).toHaveCount(0);
  });
}

for (const failure of [false, true]) {
  test(`technical cards preserve ${failure ? "errors" : "required-tool warnings"} without debug mode`, async ({ page }) => {
    await installTauriMock(page, {
      mode: "local", populated: true, locale: "en", diagnosticDebug: false, usageFailure: failure,
    });
    await page.goto("/");
    await setUsageEventOverrides(page, {
      ...diagnostics,
      ...(failure ? { upstreamError: { httpStatus: 503, code: "synthetic_provider_unavailable", errorType: "upstream_error", requestId: "synthetic_upstream_request", message: "Synthetic provider failure", redacted: false, truncated: false } } : {}),
      toolUse: { clientToolCount: 1, forwardedToolCount: 1, toolCallCount: 0, toolChoice: "required", textOutput: true, terminalOutput: "text" },
    });
    await page.getByRole("button", { name: "Usage", exact: true }).click();
    await page.getByRole("button", { name: /req_synthetic_local/ }).click();
    const details = page.getByRole("dialog").locator(".request-technical-details");
    await details.locator(":scope > summary").click();
    await expect(details.locator(".request-route-section, .request-payload-section, .request-timings-section, .request-context-section")).toHaveCount(0);
    await expect(details.locator(".request-identifiers-section")).toContainText("req_synthetic_local");
    if (failure) {
      await expect(details.locator(".request-response-section")).toContainText("503");
      await expect(details.locator(".request-response-section")).toContainText("synthetic_provider_unavailable");
    } else {
      await expect(details.locator(".request-tool-details .warning-text")).toBeVisible();
    }
  });
}
