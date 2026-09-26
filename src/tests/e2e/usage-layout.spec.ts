import { expect, test, type Page } from "../bun-playwright";
import { installTauriMock } from "./tauri-mock";

async function expectReportFits(page: Page) {
  const overflow = await page.locator(".usage-page, .usage-view-toolbar, .usage-scope-controls, .usage-metric, .usage-metric-copy, .usage-filter-panel, .usage-filter-secondary, .usage-page .relay-table-wrap, .usage-sortable-table td, .usage-pagination, .usage-pagination-form").evaluateAll((elements) => elements
    .filter((element) => element.getBoundingClientRect().width > 0)
    .filter((element) => {
      const rect = element.getBoundingClientRect();
      return element.scrollWidth > element.clientWidth + 1 || rect.left < 0 || rect.right > innerWidth + 1;
    })
    .map((element) => `${element.className || element.tagName}: ${element.textContent?.slice(0, 100)}`));
  expect(overflow).toEqual([]);
}

for (const theme of ["light", "dark"] as const) {
  for (const width of [1660, 1160, 840, 600, 390, 360]) {
    test(`Usage layout ${theme} ${width}`, async ({ page }) => {
      await page.setViewportSize({ width, height: 900 });
      await installTauriMock(page, {
        mode: "local", locale: "ru", theme, populated: true, accountCount: 4,
        quotaAvailable: true, usageFailure: true, usageToolDiagnostics: "forwarded_text_only",
        usageRequestedModel: "synthetic-model-with-a-long-name-2026",
        usageResolvedModel: "synthetic-model-with-a-long-name-2026",
        usageTotalPages: 2,
      });
      await page.goto("/");
      await page.getByRole("button", { name: "Использование", exact: true }).click();
      await expect(page.locator(".usage-request-table tbody tr")).toHaveCount(1);
      await expectReportFits(page);
      expect(await page.locator(".usage-view-toolbar .relay-tabs").evaluate((element) => element.scrollWidth <= element.clientWidth)).toBe(true);
      await page.mouse.move(0, 0);
      await page.screenshot({ path: `output/playwright/usage-${theme}-${width}.png` });
      if (width === 1160 || width === 390 || width === 360) {
        await page.getByRole("navigation", { name: "Страницы использования" }).scrollIntoViewIfNeeded();
        await page.screenshot({ path: `output/playwright/usage-pagination-${theme}-${width}.png` });
      }

      await page.getByRole("button", { name: "Другие фильтры" }).click();
      await expectReportFits(page);
      await page.locator(".usage-filter-secondary").getByRole("button", { name: /^Протокол:/ }).click();
      await page.getByRole("option", { name: "Responses", exact: true }).click();
      await expect(page.locator(".usage-filter-toggle-wrap small")).toHaveText("1");
      await expectReportFits(page);

      await page.getByRole("button", { name: "Сведения о запросе: req_synthetic_local" }).click();
      const dialog = page.getByRole("dialog", { name: "Сведения о запросе" });
      expect(await dialog.locator(".relay-tabs").evaluate((element) => element.scrollWidth <= element.clientWidth)).toBe(true);
      for (const tab of ["Обзор", "Токены", "Инструменты", "Маршрут"]) {
        await dialog.getByRole("tab", { name: tab, exact: true }).click();
        expect(await dialog.locator(".relay-dialog-body, .request-details-header, .request-details-metric, .request-details-list dd").evaluateAll((elements) => elements.every((element) => element.scrollWidth <= element.clientWidth + 1))).toBe(true);
      }
      await dialog.getByRole("tab", { name: "Обзор", exact: true }).click();
      await page.screenshot({ path: `output/playwright/usage-details-${theme}-${width}.png` });
      await page.locator(".relay-modal-backdrop").click({ position: { x: 2, y: 2 } });
      await expect(dialog).toHaveCount(0);

      for (const [tab, table] of [["Модели", ".usage-models-table"], ["Участники пула", ".usage-connections-table"], ["Ошибки", ".usage-error-table"]] as const) {
        await page.getByRole("tab", { name: tab, exact: true }).click();
        await expect(page.locator(`${table} tbody tr`)).toHaveCount(1);
        await expectReportFits(page);
        if (width === 1160 || width === 390) await page.screenshot({ path: `output/playwright/${table.slice(1)}-${theme}-${width}.png` });
      }

      await page.locator(".usage-account-menu").getByRole("button").click();
      await page.getByRole("option", { name: "Personal Plus", exact: true }).click();
      const account = page.locator(".usage-account-value");
      await expect(account).toBeVisible();
      await expectReportFits(page);
      await account.locator("summary").click();
      const hint = account.locator("details p");
      await expect(hint).toBeInViewport();
      expect(await hint.evaluate((element) => {
        const rect = element.getBoundingClientRect();
        return rect.left >= 0 && rect.right <= innerWidth && element.scrollWidth <= element.clientWidth + 1;
      })).toBe(true);
      if (width === 1160 || width === 390) await page.screenshot({ path: `output/playwright/usage-account-${theme}-${width}.png` });
    });
  }
}
