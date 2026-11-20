import { expect, test } from "../bun-playwright";
import { installTauriMock } from "./tauri-mock";
import { completedUsage, setUsageEventOverrides } from "./request-details-fixture";

for (const locale of ["ru", "en"] as const) {
  for (const theme of ["light", "dark"] as const) {
    for (const width of [360, 840, 1160]) {
      test(`background request parameters stay under the disclosure: ${locale}, ${theme}, ${width}`, async ({ page }) => {
        await page.setViewportSize({ width, height: 900 });
        await installTauriMock(page, {
          mode: "local", locale, theme, populated: true,
          usageRequestedModel: "synthetic-model", usageResolvedModel: "synthetic-model",
        });
        await page.goto("/");
        await setUsageEventOverrides(page, {
          ...completedUsage,
          errorCategory: "codex_memory_consolidation",
          requestedReasoningEffort: "medium", effectiveReasoningEffort: "medium",
        });
        await page.getByRole("button", { name: locale === "ru" ? "Использование" : "Usage", exact: true }).click();
        await page.getByRole("button", {
          name: `${locale === "ru" ? "Сведения о запросе" : "Request details"}: req_synthetic_local`, exact: true,
        }).click();
        const dialog = page.getByRole("dialog");
        const parameters = dialog.locator(".request-details-parameters");
        await expect(parameters).toHaveCount(0);
        await dialog.locator(".request-technical-details > summary").click();
        const cells = parameters.locator(":scope > div");
        await expect(cells).toHaveCount(5);
        await expect(cells.last().locator("dt")).toHaveText(locale === "ru" ? "Тип запроса" : "Request type");
        await expect(cells.last().locator("dd")).toHaveText(locale === "ru" ? "Сводки памяти" : "Memory summaries");
        const layout = await cells.evaluateAll((elements) => elements.map((element) => element.getBoundingClientRect().top));
        if (width > 690) expect(layout[1]).toBe(layout[0]);
        else expect(layout[1]).toBeGreaterThan(layout[0]!);
        expect(await parameters.locator("div, dt, dd").evaluateAll((elements) =>
          elements.every((element) => element.scrollWidth <= element.clientWidth + 1))).toBe(true);
        await parameters.screenshot({ path: `output/playwright/request-origin-${locale}-${theme}-${width}.png` });
      });
    }
  }
}

for (const width of [360, 1160]) {
  test(`partial parameters keep model identifiers separate: ${width}`, async ({ page }) => {
    await page.setViewportSize({ width, height: 900 });
    await installTauriMock(page, {
      mode: "local", locale: "en", theme: "dark", populated: true,
      usageRequestedModel: "synthetic-requested-model", usageResolvedModel: "synthetic-routed-model",
    });
    await page.goto("/");
    await setUsageEventOverrides(page, {
      ...completedUsage,
      wireApi: null, serviceTier: null,
      requestedReasoningEffort: null, effectiveReasoningEffort: null,
      errorCategory: "codex_memory_consolidation",
    });
    await page.getByRole("button", { name: "Usage", exact: true }).click();
    await page.getByRole("button", { name: "Request details: req_synthetic_local", exact: true }).click();
    const dialog = page.getByRole("dialog");
    await dialog.locator(".request-technical-details > summary").click();
    await expect(dialog.locator(".request-details-parameters dt")).toHaveText(["Client transport", "Request type"]);
    await expect(dialog.locator(".request-id-list")).toContainText("synthetic-requested-model");
    await expect(dialog.locator(".request-id-list")).toContainText("synthetic-routed-model");
  });
}
