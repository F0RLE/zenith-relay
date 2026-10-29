import { expect, test } from "../bun-playwright";
import { installTauriMock } from "./tauri-mock";
import { completedUsage, setUsageEventOverrides } from "./request-details-fixture";

for (const locale of ["en", "ru"] as const) {
  for (const debug of [false, true]) {
    test(`request diagnostics are opt-in in ${locale}, debug=${debug}`, async ({ page }) => {
      await installTauriMock(page, { mode: "local", locale, populated: true, diagnosticDebug: debug });
      await page.setViewportSize({ width: 840, height: 760 });
      await page.goto("/");
      await setUsageEventOverrides(page, {
        ...completedUsage, success: true, httpStatus: 200,
        routing: {
          reason: "only_eligible", eligibleCandidates: 1, quotaRemainingBasisPoints: 4000,
          inFlightBefore: 0, dispatchesBefore: 16, endpointKind: "excel_basis_points",
          transport: { upstream: "http", mode: "basis_points", requestPreparationMs: 20, requestPreparation: { bodyMs: 10 } },
        },
        toolUse: {
          ...completedUsage.toolUse,
          clientContext: { bodyBytes: 680700, instructionBytes: 51800, conversationBytes: 248500, toolCatalogBytes: 378700 },
          upstreamContext: { bodyBytes: 324300, instructionBytes: 71400, conversationBytes: 252500, toolCatalogBytes: 0 },
        },
      });
      await page.getByRole("button", { name: locale === "en" ? "Usage" : "Использование", exact: true }).click();
      await page.getByRole("button", { name: `${locale === "en" ? "Request details" : "Сведения о запросе"}: req_synthetic_local`, exact: true }).click();
      const dialog = page.getByRole("dialog");
      await expect(dialog.locator(".request-details-metrics dt")).toHaveCount(4);
      await expect(dialog.locator(".request-token-details")).toBeVisible();
      await dialog.locator(".request-technical-details > summary").click();
      await expect(dialog.getByText("req_synthetic_local", { exact: true })).toBeVisible();
      for (const selector of [".request-route-section", ".request-context-section", ".request-tool-details"]) {
        await expect(dialog.locator(selector)).toHaveCount(debug ? 1 : 0);
      }
      expect(await dialog.locator(".relay-dialog-body, .request-details-list dd").evaluateAll((items) =>
        items.every((item) => item.scrollWidth <= item.clientWidth + 1))).toBe(true);
      if (locale === "ru") await dialog.screenshot({ path: `output/playwright/usage-debug-${debug}.png` });
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
  await dialog.locator(".request-technical-details > summary").click();
  await expect(dialog.locator(".request-tool-details dd.warning-text")).toBeVisible();
  await expect(dialog.locator(".request-tool-details dd.warning-text")).toHaveText("The required tool call is missing.");
  await expect(dialog.locator(".request-tool-details dt")).toHaveText(["Terminal output"]);
  await expect(dialog.locator(".request-technical-details p, .request-technical-details .form-note")).toHaveCount(0);
  await expect(dialog.locator(".request-tools-list")).toHaveCount(0);
});
