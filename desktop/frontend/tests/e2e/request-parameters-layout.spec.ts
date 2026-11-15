import { expect, test } from "../bun-playwright";
import { installTauriMock } from "./tauri-mock";
import { completedUsage, setUsageEventOverrides } from "./request-details-fixture";

for (const locale of ["ru", "en"] as const) {
  for (const theme of ["light", "dark"] as const) {
    for (const width of [360, 840, 1160]) {
      test(`background request parameters share the grid: ${locale}, ${theme}, ${width}`, async ({ page }) => {
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
        const parameters = page.getByRole("dialog").locator(".request-details-parameters");
        const cells = parameters.locator(":scope > div");
        await expect(cells).toHaveCount(5);
        await expect(cells.last().locator("dt")).toHaveText(locale === "ru" ? "Тип запроса" : "Request type");
        await expect(cells.last().locator("dd")).toHaveText(locale === "ru" ? "Сводки памяти" : "Memory summaries");
        const layout = await cells.evaluateAll((elements) => elements.map((element) => {
          const bounds = element.getBoundingClientRect();
          const center = bounds.x + bounds.width / 2;
          return {
            top: bounds.y,
            leftBorder: getComputedStyle(element).borderInlineStartWidth,
            centered: getComputedStyle(element).textAlign === "center"
              && Array.from(element.querySelectorAll(":scope > dt, :scope > dd")).every((child) => {
                const childBounds = child.getBoundingClientRect();
                return Math.abs(childBounds.x + childBounds.width / 2 - center) < 1;
              }),
          };
        }));
        expect(layout.every((cell) => cell.centered)).toBe(true);
        if (width > 820) {
          expect(Math.max(...layout.map((cell) => cell.top)) - Math.min(...layout.map((cell) => cell.top))).toBeLessThan(1);
        } else {
          const grid = await parameters.boundingBox();
          const origin = await cells.last().boundingBox();
          expect(origin!.width).toBeCloseTo(grid!.width - 2, 0);
          expect(layout.at(-1)!.leftBorder).toBe("0px");
        }
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
    const parameters = page.getByRole("dialog").locator(".request-details-parameters");
    await expect(parameters.locator(":scope > .request-parameter-wide")).toHaveCount(2);
    const cells = parameters.locator(":scope > div:not(.request-parameter-wide)");
    await expect(cells).toHaveCount(2);
    const grid = await parameters.boundingBox();
    const widths = await cells.evaluateAll((elements) => elements.map((element) => element.getBoundingClientRect().width));
    expect(widths[0]).toBeCloseTo((grid!.width - 2) / 2, 0);
    expect(widths[1]).toBeCloseTo(widths[0], 0);
  });
}
