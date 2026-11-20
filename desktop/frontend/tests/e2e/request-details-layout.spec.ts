import { expect, test } from "../bun-playwright";
import { installTauriMock } from "./tauri-mock";
import { completedUsage, receiptCacheShare, setUsageEventOverrides } from "./request-details-fixture";

for (const locale of ["ru", "en"] as const) {
  const ru = locale === "ru";
  for (const theme of ["dark", "light"] as const) {
    for (const width of [360, 600, 840, 1160]) {
      test(`receipt keeps the metrics together above one ledger: ${locale}, ${theme}, ${width}`, async ({ page }) => {
        await page.setViewportSize({ width, height: width === 840 ? 716 : 900 });
        const model = width === 360 ? "synthetic-model-with-a-long-name-2026" : "gpt-5.4";
        await installTauriMock(page, { locale, theme, populated: true, usageRequestedModel: model, usageResolvedModel: model });
        await page.goto("/");
        await setUsageEventOverrides(page, completedUsage);
        await page.getByRole("button", { name: ru ? "Использование" : "Usage", exact: true }).click();
        const opener = page.getByRole("button", { name: `${ru ? "Сведения о запросе" : "Request details"}: req_synthetic_local` });
        await opener.click();
        const dialog = page.getByRole("dialog");
        const ledger = dialog.getByRole("table", { name: ru ? "Токены и стоимость" : "Tokens and cost" });
        await expect(ledger.getByRole("columnheader")).toHaveText(ru ? ["Категория", "Токены", "Эквивалент API"] : ["Category", "Tokens", "API equivalent"]);
        await expect(ledger.locator(".request-details-token-group-heading .request-token-count")).toHaveText(ru ? ["18 420", "920"] : ["18,420", "920"]);
        await expect(ledger.locator(".request-details-token-group-heading .request-token-cost")).toHaveText(ru
          ? ["≈0,01149 $", "≈0,0138 $"] : ["≈$0.01149", "≈$0.0138"]);
        await expect(ledger.locator(".request-details-token-child .request-token-cost")).toHaveText(ru
          ? ["≈0,00765 $", "≈0,00384 $", "≈0,00 $", "включено", "включено"]
          : ["≈$0.00765", "≈$0.00384", "≈$0.00", "included", "included"]);
        await expect(ledger.locator("tfoot td")).toHaveText(ru ? ["19 340", "≈0,02529 $"] : ["19,340", "≈$0.02529"]);
        await expect(receiptCacheShare(dialog).locator("dd")).toHaveText(ru ? /83,4\s*%/ : "83.4%");
        const metrics = dialog.locator(".request-details-metrics");
        await expect(metrics).toHaveAccessibleName(ru ? "Показатели запроса" : "Request metrics");
        await expect(metrics.locator("dt")).toHaveCount(4);
        const top = (await metrics.boundingBox())!;
        const table = (await ledger.boundingBox())!;
        expect(table.y).toBeGreaterThan(top.y + top.height);
        expect(Math.abs(table.x - top.x)).toBeLessThan(1);
        expect(Math.abs(table.width - top.width)).toBeLessThan(1);
        const perRow = await metrics.locator("dt").evaluateAll((els) => {
          const tops = els.map((el) => el.getBoundingClientRect().top);
          return tops.filter((y) => Math.abs(y - tops[0]!) < 1).length;
        });
        expect(perRow).toBe(width === 360 ? 2 : 4);
        expect(await ledger.locator("tr").evaluateAll((rows) => {
          const rights = rows.map((r) => r.children[1]!.getBoundingClientRect().right);
          return Math.max(...rights) - Math.min(...rights);
        })).toBeLessThan(1);
        await expect(dialog.locator(".request-details-parameters, .request-id-list")).toHaveCount(0);
        if (ru && (width === 840 || width === 360)) {
          await page.mouse.move(0, 0);
          await dialog.screenshot({ path: `output/playwright/request-inspector-${theme}-${width}.png` });
        }
        await dialog.locator(".request-technical-details > summary").click();
        await expect(dialog.locator(".request-details-parameters dd")).toHaveText(ru
          ? ["Responses", "HTTP", "Обычная", "Максимальное"] : ["Responses", "HTTP", "Standard", "Max"]);
        await expect(dialog.locator(".request-id-list")).toContainText("req_synthetic_local");
        expect(await dialog.locator(".relay-dialog-body, section, table, th, td, dl, dd").evaluateAll((els) =>
          els.every((el) => el.scrollWidth <= el.clientWidth + 1))).toBe(true);
        await page.keyboard.press("Escape");
        await expect(dialog).toHaveCount(0);
        await expect(opener).toBeFocused();
      });
    }
  }
}

for (const scenario of ["missing", "partial", "micro"] as const) {
  test(`line prices preserve ${scenario} evidence`, async ({ page }) => {
    await installTauriMock(page, { locale: "en", populated: true });
    await page.goto("/");
    const cacheRead = scenario === "partial" ? { microUsd: 0, pricedTokens: 0, unpricedTokens: 15360 }
      : { microUsd: 1, pricedTokens: 15360, unpricedTokens: 0 };
    const input = { microUsd: 7650 + cacheRead.microUsd, pricedTokens: 3060 + cacheRead.pricedTokens, unpricedTokens: cacheRead.unpricedTokens };
    await setUsageEventOverrides(page, { ...completedUsage, apiEquivalentBreakdown:
      scenario === "missing" ? undefined : { ...completedUsage.apiEquivalentBreakdown, input, cacheRead } });
    await page.getByRole("button", { name: "Usage", exact: true }).click();
    await page.getByRole("button", { name: "Request details: req_synthetic_local" }).click();
    const table = page.getByRole("table", { name: "Tokens and cost" });
    if (scenario === "missing") {
      await expect(table.locator(".request-token-cost:not(.request-token-included)")).toHaveText(["—", "—", "—", "—", "—"]);
    } else {
      await expect(table.getByRole("row").filter({ hasText: "Cache reads" }).locator(".request-token-cost")).toHaveText(scenario === "micro" ? "≈$0.000001" : "—");
      const cost = table.locator(".request-details-token-group-heading .request-token-cost").first();
      await expect(cost).toHaveText(scenario === "micro" ? "≈$0.007651" : "≈$0.00765");
      if (scenario === "partial") await expect(cost).toHaveAttribute("data-relay-tooltip", /Unpriced tokens: 15360/);
    }
  });
}

for (const total of [0, null]) {
  test(`receipt does not equate total ${total} with a missing figure`, async ({ page }) => {
    await installTauriMock(page, { locale: "en", populated: true });
    await page.goto("/");
    await setUsageEventOverrides(page, { ...completedUsage, totalTokens: total, inputTokens: total ?? 3060,
      outputTokens: total, cachedInputTokens: null, cacheWriteInputTokens: null, reasoningTokens: null, apiEquivalentBreakdown: undefined });
    await page.getByRole("button", { name: "Usage", exact: true }).click();
    await page.getByRole("button", { name: "Request details: req_synthetic_local" }).click();
    await expect(page.locator(".request-token-total")).toHaveText(total === 0 ? "0" : "—");
    await expect(page.locator(".request-details-token-group-heading .request-token-count")).toHaveText(total === 0 ? ["0", "0"] : ["3,060"]);
  });
}
