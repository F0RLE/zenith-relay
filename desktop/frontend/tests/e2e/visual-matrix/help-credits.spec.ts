import { expect, test } from "../../bun-playwright";
import { installTauriMock } from "../tauri-mock";
import { themes, locales, viewports } from "./fixtures";

for (const theme of themes) {
  for (const locale of locales) {
    for (const width of [1160, 840, 430]) {
      test(`Help reading layout ${locale} ${theme} ${width}`, async ({ page }) => {
        await page.setViewportSize({ width, height: 760 });
        await installTauriMock(page, { mode: "local", locale, theme, populated: true });
        await page.goto("/");
        await page.getByRole("button", { name: locale === "ru" ? "Помощь" : "Help", exact: true }).click();
        const article = page.locator(".help-document");
        const contents = page.locator(".help-contents");
        await expect(contents).toBeVisible();
        await expect(contents.locator("a[aria-current]")).toHaveCount(1);
        const [articleBox, contentsBox] = await Promise.all([article.boundingBox(), contents.boundingBox()]);
        expect(articleBox).not.toBeNull();
        expect(contentsBox).not.toBeNull();
        if (width > 760) expect(articleBox!.x + articleBox!.width).toBeLessThan(contentsBox!.x);
        else expect(contentsBox!.y + contentsBox!.height).toBeLessThan(articleBox!.y);
        expect(await article.evaluate((element) => element.scrollWidth <= element.clientWidth)).toBe(true);
        await page.screenshot({ path: `output/playwright/help-${locale}-${theme}-${width}.png` });
        const errors = article.getByRole("heading", { name: locale === "ru" ? "8. Ошибки" : "8. Errors", exact: true });
        await errors.evaluate((element) => element.scrollIntoView({ block: "start" }));
        await expect(contents.locator("a[aria-current]")).toHaveText(locale === "ru" ? "Ошибки" : "Errors");
        await expect(contents).toBeInViewport();
        expect(await article.locator(".help-table-wrap, table, td").evaluateAll((elements) => elements.every((element) => element.scrollWidth <= element.clientWidth))).toBe(true);
        if (width <= 760) {
          const [headingBox, menuBox] = await Promise.all([errors.boundingBox(), contents.boundingBox()]);
          expect(headingBox!.y).toBeGreaterThanOrEqual(menuBox!.y + menuBox!.height);
        }
        await page.screenshot({ path: `output/playwright/help-errors-${locale}-${theme}-${width}.png` });
        await article.getByRole("searchbox").fill("no_eligible_source");
        await expect(article.locator(".help-error-group[open]")).toHaveCount(1);
        await article.locator(".help-error-group").scrollIntoViewIfNeeded();
        expect(await article.locator(".help-table-wrap, table, td").evaluateAll((elements) => elements.every((element) => element.scrollWidth <= element.clientWidth))).toBe(true);
        await page.screenshot({ path: `output/playwright/help-error-result-${locale}-${theme}-${width}.png` });
      });
    }
  }
}

for (const viewport of viewports) {
  for (const theme of themes) {
    test(`provider credit rows stay balanced across account cards ${theme} ${viewport.width}`, async ({ page }) => {
      await installTauriMock(page, { mode: "local", locale: "ru", theme, populated: true, accountCount: 6, quotaAvailable: true, providerCredits: 498.2, freeAccountHealthy: true });
      await page.setViewportSize(viewport);
      await page.goto("/");
      const heights: number[] = [];
      for (const [tab, selector] of [["Подключения", ".account-card"], ["Пул", ".pool-member-card"]] as const) {
        await page.getByRole("button", { name: tab, exact: true }).click();
        const card = page.locator(selector).filter({ hasText: "Free reserve" });
        await expect(card.locator(".account-subscription-line")).toHaveCount(0);
        const credits = card.locator(".account-provider-quota-strip");
        await expect(credits.locator("dd")).toHaveText("498,2");
        await card.scrollIntoViewIfNeeded();
        const bounds = await credits.evaluate((row) => {
          const rect = row.getBoundingClientRect();
          const value = row.querySelector("dd")!.getBoundingClientRect();
          return { height: rect.height, top: value.top - rect.top, bottom: rect.bottom - value.bottom, fits: row.scrollWidth <= row.clientWidth };
        });
        expect(Math.abs(bounds.top - bounds.bottom)).toBeLessThanOrEqual(1);
        expect(bounds.fits).toBe(true);
        heights.push(bounds.height);
        await card.screenshot({ path: `output/playwright/credit-card-${selector.slice(1)}-${theme}-${viewport.width}.png` });
      }
      expect(heights[0]).toBe(heights[1]);
    });
  }
}
