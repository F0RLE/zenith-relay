import { expect, test } from "../bun-playwright";
import { installTauriMock } from "./tauri-mock";
import { completedUsage, receiptCacheShare, setUsageEventOverrides } from "./request-details-fixture";

for (const locale of ["en", "ru"] as const) {
  for (const debug of [false, true]) {
    test(`the same compact receipt and explicit diagnostic export in ${locale}, debug=${debug}`, async ({ page }) => {
      await installTauriMock(page, { mode: "local", locale, theme: debug ? "dark" : "light", populated: true, diagnosticDebug: debug });
      await page.setViewportSize({ width: 840, height: 760 });
      await page.goto("/");
      await setUsageEventOverrides(page, {
        ...completedUsage, success: true, httpStatus: 200,
        routing: {
          reason: "response_affinity", eligibleCandidates: 2, quotaRemainingBasisPoints: 4000,
          inFlightBefore: 0, dispatchesBefore: 16, endpointKind: "chatgpt_siwc",
          cacheAffinity: { selection: "warm_alternative", reason: "response_owner" },
          routeLatency: { state: "ready", p50Ms: 15, p90Ms: 22, p95Ms: 30 },
          transport: { upstream: "websocket", mode: "native", requestPreparationMs: 20, requestPreparation: {
            bodyMs: 10, attachmentsMs: 10,
            attachments: { credentialMs: 0, stagingMs: 2, resolveMs: 8, imageCount: 4, fileReferences: 1, uploadedImages: 1, uploadedBytes: 1024, reusedImages: 2 },
          } },
        },
        toolUse: {
          ...completedUsage.toolUse,
          clientContext: { bodyBytes: 680700, instructionBytes: 51800, conversationBytes: 248500, toolCatalogBytes: 378700 },
          upstreamContext: { bodyBytes: 324300, instructionBytes: 71400, conversationBytes: 252500, toolCatalogBytes: 0 },
        },
      });
      await page.getByRole("button", { name: locale === "en" ? "Usage" : "Использование", exact: true }).click();
      const settingsReads = () => page.evaluate(() => (window as unknown as {
        __TAURI_TEST_INVOKES__: { command: string }[];
      }).__TAURI_TEST_INVOKES__.filter(({ command }) => command === "get_diagnostic_settings").length);
      const before = await settingsReads();
      await page.getByRole("button", { name: `${locale === "en" ? "Request details" : "Сведения о запросе"}: req_synthetic_local`, exact: true }).click();
      const dialog = page.getByRole("dialog");
      await expect(dialog.locator(".request-details-metrics dt")).toHaveCount(4);
      await expect(dialog.locator(".request-token-details")).toBeVisible();
      await dialog.locator(".request-technical-details > summary").click();
      await expect(dialog.getByText("req_synthetic_local", { exact: true })).toBeVisible();
      await expect(dialog.locator(".request-id-list dt")).toHaveText(locale === "en"
        ? ["Connection choice", "Provider connection", "Relay request ID"]
        : ["Выбор подключения", "Связь с провайдером", "ID запроса Relay"]);
      await expect(dialog.locator(".request-id-list dd").first()).toHaveText(locale === "en"
        ? "Continued on the connection that returned the previous response"
        : "Продолжение на подключении, которое вернуло предыдущий ответ");
      await expect(dialog).not.toContainText(/chatgpt_siwc|P50|P90|P95|680700|eligibleCandidates/);
      await expect(receiptCacheShare(dialog).locator("dd")).toHaveText(locale === "en" ? "83.4%" : /83,4\s*%/);
      expect(await settingsReads()).toBe(before);
      await dialog.getByRole("button", { name: locale === "en" ? "Save diagnostics" : "Сохранить диагностику", exact: true }).click();
      await expect.poll(() => page.evaluate(() => (window as unknown as {
        __TAURI_TEST_INVOKES__: { command: string; args: {requestId?: string} }[];
      }).__TAURI_TEST_INVOKES__.filter(({ command }) => command === "export_request_diagnostics").map(({ args }) => args.requestId)))
        .toEqual(["req_synthetic_local"]);
      expect(await dialog.locator(".relay-dialog-body, .request-details-list dd").evaluateAll((items) =>
        items.every((item) => item.scrollWidth <= item.clientWidth + 1))).toBe(true);
      if (locale === "ru") await dialog.screenshot({ path: `output/playwright/request-receipt-${debug ? "dark" : "light"}.png` });
    });
  }
}

test("required-tool warning remains visible outside debug mode", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, diagnosticDebug: false });
  await page.goto("/");
  await setUsageEventOverrides(page, {
    ...completedUsage, success: true,
    toolUse: { ...completedUsage.toolUse, toolChoice: "required", toolCallCount: 0, terminalOutput: "text" },
  });
  await page.getByRole("button", { name: "Usage", exact: true }).click();
  await page.getByRole("button", { name: "Request details: req_synthetic_local", exact: true }).click();
  const dialog = page.getByRole("dialog");
  await expect(dialog.getByText("The required tool call is missing.", { exact: true })).toBeVisible();
  await expect(dialog.locator(".request-technical-details")).not.toHaveAttribute("open");
  await expect(dialog.locator(".request-technical-details p, .request-technical-details .form-note")).toHaveCount(0);
  await expect(dialog.locator(".request-tools-list")).toHaveCount(0);
});

for (const [scenario, input, cached, rate, baseline, changed, selection, label] of [
  ["missing input", null, 10, null, "overlapping_requests", true, "other", null],
  ["missing cache", 100, null, null, "completed_request", null, "other", null],
  ["inconsistent counters", 100, 101, null, "completed_request", true, "other", "Different connection from the previous completed request"],
  ["measured zero", 100, 0, "0%", "completed_request", false, "warm_alternative", "Backup connection already used in this chat"],
  ["cache without rotation diagnostics", 100, 100, "100%", "unavailable", null, "other", null],
  ["blocked owner", 100, 2, "2%", "completed_request", true, "other", "Switched because the preferred connection was rate-limited"],
] as const) {
  test(`receipt separates measured cache from connection choice: ${scenario}`, async ({ page }) => {
    await installTauriMock(page, { mode: "local", locale: "en", populated: true });
    await page.goto("/");
    await setUsageEventOverrides(page, {
      ...completedUsage, inputTokens: input, cachedInputTokens: cached, routing: {
        reason: "pool_policy", cacheContext: { baseline, candidateChanged: changed },
        cacheAffinity: scenario === "blocked owner" ? { selection, reason: "owner_unavailable", ownerBlockReason: "rate_limited" } : { selection },
      },
    });
    await page.getByRole("button", { name: "Usage", exact: true }).click();
    await page.getByRole("button", { name: "Request details: req_synthetic_local", exact: true }).click();
    const dialog = page.getByRole("dialog");
    const share = receiptCacheShare(dialog);
    if (rate) await expect(share.locator("dd")).toHaveText(rate);
    else await expect(share).toHaveCount(0);
    await dialog.locator(".request-technical-details > summary").click();
    const choice = dialog.locator(".request-id-list > div").filter({ hasText: "Connection choice" });
    if (label) await expect(choice.locator("dd")).toHaveText(label);
    else await expect(choice).toHaveCount(0);
  });
}
