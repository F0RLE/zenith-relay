import { expect, test, type Page } from "../bun-playwright";
import { installTauriMock } from "./tauri-mock";

async function expectReportFits(page: Page) {
  const overflow = await page.locator(".usage-page, .usage-view-toolbar, .usage-scope-controls, .usage-overview, .usage-overview-details, .usage-token-totals, .usage-token-totals dd, .usage-metric, .usage-metric-copy, .usage-filter-panel, .usage-filters, .usage-page .relay-table-wrap, .usage-sortable-table td, .usage-pagination, .usage-pagination-controls, .usage-pagination-form").evaluateAll((elements) => elements
    .filter((element) => element.getBoundingClientRect().width > 0)
    .filter((element) => {
      const rect = element.getBoundingClientRect();
      return element.scrollWidth > element.clientWidth + 1 || rect.left < 0 || rect.right > innerWidth + 1;
    })
    .map((element) => ({
      element: element.className || element.tagName,
      width: element.clientWidth,
      scrollWidth: element.scrollWidth,
      columns: getComputedStyle(element).gridTemplateColumns,
      children: Array.from(element.children).map((child) => ({
        element: child.className || child.tagName,
        width: child.getBoundingClientRect().width,
        column: getComputedStyle(child).gridColumn,
      })),
    })));
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
      const status = page.locator('.usage-request-table td[data-column="status"]');
      await expect(status).toHaveText("Ошибка провайдера");
      await expect(status.getByRole("img", { name: "Ошибка провайдера", exact: true })).toHaveAttribute("data-status", "error");
      await expect(page.locator('.usage-request-table td[data-column="connection"]')).toHaveText("Personal Plus");
      await expect(page.locator(".usage-metrics > .usage-metric")).toHaveCount(4);
      await expect(page.locator(".usage-secondary-metrics .usage-metric")).toHaveCount(2);
      if (width <= 390) {
        const firstRequest = await page.locator(".usage-request-table tbody tr").first().boundingBox();
        expect(firstRequest!.y).toBeLessThan(700);
      }
      await expectReportFits(page);
      expect(await page.locator(".usage-page .relay-workspace-header .relay-tabs").evaluate((element) => element.scrollWidth <= element.clientWidth)).toBe(true);
      await page.mouse.move(0, 0);
      await page.screenshot({ path: `output/playwright/usage-${theme}-${width}.png` });
      if (width === 1160 || width === 390 || width === 360) {
        await page.getByRole("navigation", { name: "Страницы использования" }).scrollIntoViewIfNeeded();
        await page.screenshot({ path: `output/playwright/usage-pagination-${theme}-${width}.png` });
        await page.locator(".usage-pagination-controls").screenshot({ path: `output/playwright/usage-pagination-control-${theme}-${width}.png` });
      }

      await page.getByRole("button", { name: "Другие фильтры" }).click();
      await expectReportFits(page);
      await page.locator(".usage-filters").getByRole("button", { name: /^Протокол:/ }).click();
      await page.getByRole("option", { name: "Responses", exact: true }).click();
      await expect(page.locator(".usage-filter-toggle-wrap small")).toHaveText("1");
      await expectReportFits(page);

      await page.getByRole("button", { name: "Сведения о запросе: req_synthetic_local" }).click();
      const dialog = page.getByRole("dialog", { name: "Сведения о запросе" });
      await expect(dialog.getByRole("tab")).toHaveCount(0);
      await page.screenshot({ path: `output/playwright/usage-details-${theme}-${width}.png` });
      for (const technicalExpanded of [false, true]) {
        if (technicalExpanded) await dialog.locator(".request-technical-details > summary").click();
        expect(await dialog.locator(".relay-dialog-body, .request-details-header, .request-details-metric, .request-details-parameters dd, .request-details-list dd").evaluateAll((elements) => elements.every((element) => element.scrollWidth <= element.clientWidth + 1))).toBe(true);
      }
      await page.locator(".relay-modal-backdrop").click({ position: { x: 2, y: 2 } });
      await expect(dialog).toHaveCount(0);

      for (const [tab, table] of [["Модели", ".usage-models-table"], ["Участники пула", ".usage-connections-table"], ["Ошибки", ".usage-error-table"]] as const) {
        await page.getByRole("tab", { name: tab, exact: true }).click();
        await expect(page.locator(`${table} tbody tr`)).toHaveCount(1);
        await expectReportFits(page);
        if (width === 1160 || width === 390) await page.screenshot({ path: `output/playwright/${table.slice(1)}-${theme}-${width}.png` });
      }

      await page.getByRole("tab", { name: "Запросы", exact: true }).click();
      await page.getByRole("button", { name: /^Участник пула:/ }).click();
      await page.getByRole("option", { name: "Personal Plus", exact: true }).click();
      const account = page.locator(".usage-account-value");
      await expect(account).toBeVisible();
      await expectReportFits(page);
      await account.locator("summary").evaluate((element) => element.scrollIntoView({ block: "end" }));
      await account.locator("summary").click();
      const hint = account.locator("details p");
      await expect(hint).toBeInViewport();
      await expect.poll(() => hint.evaluate((element) => {
        const rect = element.getBoundingClientRect();
        return rect.top >= 36 && rect.bottom <= innerHeight + 1 && rect.left >= 0 && rect.right <= innerWidth
          && element.scrollWidth <= element.clientWidth + 1;
      })).toBe(true);
      if (width === 1160 || width === 390) await page.screenshot({ path: `output/playwright/usage-account-${theme}-${width}.png` });
    });
  }
}

for (const locale of ["ru", "en"] as const) {
  for (const width of [1160, 360]) {
    test(`usage pagination compact long history ${locale} ${width}`, async ({ page }) => {
      await page.setViewportSize({ width, height: 900 });
      await installTauriMock(page, { mode: "local", locale, theme: width === 1160 ? "light" : "dark", populated: true, usageTotalPages: 10_000 });
      await page.goto("/");
      await page.getByRole("button", { name: locale === "ru" ? "Использование" : "Usage", exact: true }).click();
      const pagination = page.locator(".usage-pagination");
      await pagination.scrollIntoViewIfNeeded();
      await expect(pagination.locator(".usage-pagination-page")).toHaveText(["1", "2", "3", "10000"]);
      await expectReportFits(page);

      await pagination.getByRole("button", { name: locale === "ru" ? "Перейти к странице" : "Jump to page", exact: true }).click();
      const input = pagination.getByRole("textbox");
      await expect(input).toBeFocused();
      await input.fill("5678");
      await expectReportFits(page);
      await input.press("Enter");
      await expect(pagination.locator('[aria-current="page"]')).toHaveText("5678");
      await expect(pagination.locator(".usage-pagination-page")).toHaveText(["1", "5678", "10000"]);
      await expectReportFits(page);
      await page.mouse.move(0, 0);
      await page.screenshot({ path: `output/playwright/usage-pagination-long-${locale}-${width}.png` });

      await pagination.locator(".usage-pagination-page").last().click();
      await expect(pagination.locator('[aria-current="page"]')).toHaveText("10000");
      await expect(pagination.getByRole("button", { name: locale === "ru" ? "Продолжить" : "Continue", exact: true })).toBeDisabled();
      await expectReportFits(page);
    });
  }
}

for (const locale of ["ru", "en"] as const) {
  for (const width of [1440, 390]) {
    test(`usage high-volume summary ${locale} ${width}`, async ({ page }) => {
      await page.setViewportSize({ width, height: 1000 });
      await installTauriMock(page, {
        mode: "local", locale, theme: "dark", populated: true,
        usageSummaryTotals: {
          requests: 56_498, successfulRequests: 52_692,
          totalTokens: 8_338_700_000, inputTokens: 8_300_000_000, outputTokens: 38_700_000,
          cachedInputTokens: 7_500_000_000, cachedInputSamples: 56_498,
          cacheWriteInputTokens: 190_100_000, cacheWriteInputSamples: 56_498,
          generationOutputTokens: 38_700_000, generationMs: 834_051_724,
          speedOutputTokens: 38_700_000, speedDurationMs: 1_176_291_793,
          apiEquivalent: { microUsd: 3_112_864_200, pricedTokens: 8_178_000_000, unpricedTokens: 160_700_000 },
        },
      });
      await page.goto("/");
      await page.getByRole("button", { name: locale === "ru" ? "Использование" : "Usage", exact: true }).click();
      const summary = page.locator(".usage-overview");
      await expect(summary.locator(".usage-success-metric strong")).toHaveText("93%");
      await expect(summary.locator(".usage-metrics strong").last()).toHaveText(locale === "ru" ? /3\s112,86\s\$/ : "≈$3,112.86");
      await expect(summary.locator(".usage-token-totals dd")).toHaveCount(4);
      await expect(summary.locator(".usage-metrics svg")).toHaveCount(0);
      await expectReportFits(page);
      await page.screenshot({ path: `output/playwright/usage-redesign-${locale}-${width}.png` });
      await summary.screenshot({ path: `output/playwright/usage-summary-clean-${locale}-${width}.png` });
      const equivalent = summary.locator(".usage-metrics > .usage-metric").last();
      await expect(equivalent.locator("small")).toContainText(locale === "ru" ? "не оценено" : "unpriced");
      await summary.locator(".usage-token-metric").focus();
      await page.keyboard.press("Tab");
      await expect(equivalent).toBeFocused();
      await expect(page.getByRole("tooltip")).toContainText(locale === "ru" ? /3\s112,8642/ : "$3,112.8642");
    });
  }

  for (const unpriced of [0, 28]) {
    test(`usage price coverage is visible only for unpriced tokens: ${locale}, unpriced=${unpriced}`, async ({ page }) => {
      await installTauriMock(page, {
        mode: "local", locale, populated: true,
        usageSummaryTotals: { apiEquivalent: { microUsd: unpriced ? 0 : 148, pricedTokens: 28 - unpriced, unpricedTokens: unpriced } },
      });
      await page.goto("/");
      await page.getByRole("button", { name: locale === "ru" ? "Использование" : "Usage", exact: true }).click();
      const equivalent = page.locator(".usage-metrics > .usage-metric").last();
      await expect(equivalent.locator("small")).toHaveCount(unpriced ? 1 : 0);
      if (unpriced) {
        await expect(equivalent.locator("small")).toContainText(locale === "ru" ? "не оценено: 28" : "28 unpriced");
        await expect(equivalent.locator("strong")).toHaveText("—");
      }
      await page.locator(".usage-range-menu button").click();
      await page.getByRole("option", { name: locale === "ru" ? "Всё время" : "All time", exact: true }).click();
      await expect(page.locator(".usage-range-menu button")).toContainText(locale === "ru" ? "Всё время" : "All time");
    });
  }

  for (const measured of [false, true]) {
    test(`usage cache summary distinguishes unknown from zero: ${locale}, measured=${measured}`, async ({ page }) => {
      await installTauriMock(page, {
        mode: "local", locale, populated: true,
        usageCache: { cachedInputTokens: measured ? 0 : null, cacheWriteInputTokens: measured ? 0 : null },
      });
      await page.goto("/");
      await page.getByRole("button", { name: locale === "ru" ? "Использование" : "Usage", exact: true }).click();
      const summary = page.locator(".usage-token-totals");
      const readLabel = locale === "ru" ? "Кэш: прочитано" : "Cache read";
      const writeLabel = locale === "ru" ? "Кэш: записано" : "Cache write";
      await expect(summary.locator("div").filter({ has: page.getByText(readLabel, { exact: true }) }).locator("dd")).toHaveText(measured ? "0" : "—");
      if (measured) await expect(summary.locator("div").filter({ has: page.getByText(writeLabel, { exact: true }) }).locator("dd")).toHaveText("0");
      else await expect(summary).not.toContainText(writeLabel);
    });
  }
}
