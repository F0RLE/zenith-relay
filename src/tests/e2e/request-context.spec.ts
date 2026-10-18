import { expect, test } from "../bun-playwright";
import { installTauriMock } from "./tauri-mock";
import { setUsageEventOverrides } from "./request-details-fixture";

for (const width of [390, 1160]) {
  test(`context sizes explain a large tool catalog without a false warning at ${width}`, async ({ page }) => {
    await page.setViewportSize({ width, height: 1000 });
    await installTauriMock(page, { mode: "local", locale: "ru", populated: true });
    await page.goto("/");
    await setUsageEventOverrides(page, {
      routing: { reason: "quota_headroom", eligibleCandidates: 1, inFlightBefore: 0, dispatchesBefore: 0, endpointKind: "excel_basis_points" },
      toolUse: {
        clientToolCount: 291, forwardedToolCount: 291, clientSchemaBytes: 340000, forwardedSchemaBytes: 340000,
        toolChoice: "auto", toolCallCount: 0, textOutput: true, terminalOutput: "text",
        clientContext: { bodyBytes: 400000, instructionBytes: 40000, conversationBytes: 20, toolCatalogBytes: 340000 },
        upstreamContext: { bodyBytes: 390000, instructionBytes: 389000, conversationBytes: 20, toolCatalogBytes: 0 },
        basisPointsCatalog: { mode: "discovery", fullSchemaBytes: 340000, renderedSchemaBytes: 3000, catalogPromptBytes: 18000, loadedToolCount: 3, discoveryRounds: 1, discoveryInputTokens: 4000, discoveryOutputTokens: 20 },
      },
    });
    await page.getByRole("button", { name: "Использование", exact: true }).click();
    await page.getByRole("button", { name: "Сведения о запросе: req_synthetic_local" }).click();
    const dialog = page.getByRole("dialog", { name: "Сведения о запросе" });
    await dialog.locator(".request-technical-details > summary").click();
    await expect(dialog.getByText("Контекст", { exact: true })).toBeVisible();
    await expect(dialog.getByText("Инструменты BPS → инструкции.", { exact: true })).toBeVisible();
    await expect(dialog.getByText("Схемы по запросу", { exact: true })).toBeVisible();
    await expect(dialog.getByText("3 / 291", { exact: true })).toBeVisible();
    await expect(dialog.getByText("4000 / 20", { exact: true })).toBeVisible();
    await expect(dialog.locator(".request-tool-details .warning-text")).toHaveCount(0);
    const overflow = await dialog.locator(".request-details-list dd").evaluateAll((values) => values.some((value) => value.scrollWidth > value.clientWidth + 1));
    expect(overflow).toBe(false);
    await dialog.getByText("Инструменты BPS → инструкции.", { exact: true }).scrollIntoViewIfNeeded();
    await page.screenshot({ path: `output/playwright/request-context-${width}.png` });
  });
}
