import { expect, test } from "../bun-playwright";
import { installTauriMock } from "./tauri-mock";
import { completedUsage, setUsageEventOverrides } from "./request-details-fixture";

for (const locale of ["ru", "en"] as const) {
  for (const theme of ["light", "dark"] as const) {
    for (const width of [360, 600, 840, 1160]) {
      test(`request summary stays readable: ${locale}, ${theme}, ${width}`, async ({ page }) => {
        await page.setViewportSize({ width, height: 900 });
        await installTauriMock(page, {
          mode: "local", locale, theme, populated: true,
          usageRequestedModel: "synthetic-model-with-a-long-name-2026",
          usageResolvedModel: "synthetic-model-with-a-long-name-2026",
        });
        await page.goto("/");
        await setUsageEventOverrides(page, completedUsage);
        await page.getByRole("button", { name: locale === "ru" ? "Использование" : "Usage", exact: true }).click();
        const opener = page.getByRole("button", {
          name: `${locale === "ru" ? "Сведения о запросе" : "Request details"}: req_synthetic_local`, exact: true,
        });
        await opener.click();
        const dialog = page.getByRole("dialog");
        await expect(dialog.locator(".request-details-metrics > div")).toHaveCount(4);
        await expect(dialog.locator(".request-details-token-group-heading .request-token-count")).toHaveText(
          locale === "ru" ? ["18 420", "920"] : ["18,420", "920"],
        );
        await expect(dialog.locator(".request-details-token-group-heading .request-token-cost")).toHaveText(
          locale === "ru" ? ["≈0,01149 $", "≈0,0138 $"] : ["≈$0.01149", "≈$0.0138"],
        );
        await expect(dialog.locator(".request-details-token-child .request-token-cost")).toHaveText(
          locale === "ru" ? ["≈0,00765 $", "≈0,00384 $", "≈0,00 $", "включено", "включено"]
            : ["≈$0.00765", "≈$0.00384", "≈$0.00", "included", "included"],
        );
        await expect(dialog.locator(".request-technical-details")).not.toHaveAttribute("open");
        await expect(dialog.locator(".request-token-footer, .request-token-price-note, .request-cache-retention")).toHaveCount(0);
        await expect(dialog.locator(".request-details-subtitle dt")).toHaveText(locale === "ru" ? ["Подключение", "Время"] : ["Connection", "Time"]);
        const parameterSection = dialog.getByRole("region", { name: locale === "ru" ? "Параметры" : "Parameters", exact: true });
        await expect(parameterSection).toBeVisible();
        await expect(parameterSection.getByRole("heading")).toHaveCount(0);
        await expect(parameterSection.locator("dt")).toHaveCount(4);
        await expect(parameterSection.locator("dd")).toHaveText(locale === "ru"
          ? ["Responses", "HTTP", "Обычная", "Максимальное"]
          : ["Responses", "HTTP", "Standard", "Max"]);
        expect(await parameterSection.locator(".request-details-parameters > div:not(.request-parameter-wide)").evaluateAll((cells) => cells.every((cell) => {
          const bounds = cell.getBoundingClientRect();
          const center = bounds.x + bounds.width / 2;
          return getComputedStyle(cell).textAlign === "center"
            && getComputedStyle(cell.querySelector("dd")!).justifyContent === "center"
            && Array.from(cell.querySelectorAll(":scope > dt, :scope > dd")).every((element) => {
              const bounds = element.getBoundingClientRect();
              return Math.abs(bounds.x + bounds.width / 2 - center) < 1;
            });
        }))).toBe(true);

        const tokens = await dialog.locator(".request-token-details").boundingBox();
        await expect(dialog.getByRole("region", { name: locale === "ru" ? "Токены" : "Tokens", exact: true })).toBeVisible();
        const tokenTotal = dialog.locator(".request-token-total");
        await expect(tokenTotal.locator("dt")).toHaveText(locale === "ru" ? "Всего токенов" : "Total tokens");
        await expect(tokenTotal.locator("dd")).toHaveText(locale === "ru" ? "19 340" : "19,340");
        const totalLabel = await tokenTotal.locator("dt").boundingBox();
        const totalValue = await tokenTotal.locator("dd").boundingBox();
        const totalCenter = (totalLabel!.x + totalValue!.x + totalValue!.width) / 2;
        expect(totalCenter).toBeCloseTo(tokens!.x + tokens!.width / 2, 0);
        expect(totalValue!.x - totalLabel!.x - totalLabel!.width).toBeLessThanOrEqual(12);
        expect(totalValue!.x).toBeGreaterThan(totalLabel!.x + totalLabel!.width);
        expect(totalValue!.y).toBeLessThan(totalLabel!.y + totalLabel!.height);
        const parameters = await dialog.locator(".request-parameters-section").boundingBox();
        expect(parameters!.y).toBeGreaterThan(tokens!.y + tokens!.height);
        expect(parameters!.width).toBeCloseTo(tokens!.width, 0);
        const input = await dialog.locator(".request-details-token-group").nth(0).boundingBox();
        const output = await dialog.locator(".request-details-token-group").nth(1).boundingBox();
        expect(totalValue!.y + totalValue!.height).toBeLessThan(input!.y);
        expect(totalLabel!.x).toBeGreaterThan(tokens!.x);
        expect(totalValue!.x + totalValue!.width).toBeLessThan(output!.x + output!.width);
        if (width > 620) {
          expect(output!.y).toBeCloseTo(input!.y, 0);
          expect(output!.x).toBeGreaterThan(input!.x + input!.width);
          expect(output!.width).toBeCloseTo(input!.width, 0);
        } else {
          expect(output!.y).toBeGreaterThan(input!.y + input!.height);
        }
        if (width > 820) {
          const values = await dialog.locator(".request-details-parameters > div > dd").evaluateAll((elements) => elements.map((element) => element.getBoundingClientRect().y));
          expect(Math.max(...values) - Math.min(...values)).toBeLessThan(1);
        }
        const status = await dialog.locator(".request-details-status").boundingBox();
        const model = await dialog.locator(".request-details-model > h3").boundingBox();
        expect(status!.x >= model!.x + model!.width || status!.y >= model!.y + model!.height).toBe(true);
        const exportButton = await dialog.getByRole("button", { name: locale === "ru" ? "Сохранить диагностику" : "Save diagnostics", exact: true }).boundingBox();
        const identity = await dialog.locator(".request-details-identity").boundingBox();
        expect(exportButton!.x).toBeGreaterThanOrEqual(status!.x + status!.width);
        expect(exportButton!.x + exportButton!.width).toBeCloseTo(identity!.x + identity!.width, 0);
        expect(status!.x - model!.x - model!.width).toBeLessThanOrEqual(8);
        expect(await dialog.locator(".relay-dialog-body, .request-details-content, section, dl, dt, dd").evaluateAll((elements) =>
          elements.every((element) => element.scrollWidth <= element.clientWidth + 1))).toBe(true);

        if (locale === "ru" && theme === "dark" && width === 360) {
          await dialog.locator(".request-token-details").screenshot({ path: "output/playwright/request-details-token-panel-mobile.png" });
        }
        await parameterSection.screenshot({ path: `output/playwright/request-parameters-centered-${locale}-${theme}-${width}.png` });

        await dialog.locator(".request-technical-details > summary").click();
        await expect(dialog.locator(".request-identifiers-section")).toBeVisible();
        await expect(dialog.locator(".request-route-section, .request-tool-details")).toHaveCount(0);
        await page.keyboard.press("Escape");
        await expect(dialog).toHaveCount(0);
        await expect(opener).toBeFocused();
      });
    }
  }
}

for (const scenario of ["missing", "partial", "micro"] as const) {
  test(`token costs preserve ${scenario} price evidence`, async ({ page }) => {
    await installTauriMock(page, { mode: "local", locale: "en", populated: true });
    await page.goto("/");
    const cacheRead = scenario === "partial"
      ? { microUsd: 0, pricedTokens: 0, unpricedTokens: 15360 }
      : { microUsd: 1, pricedTokens: 15360, unpricedTokens: 0 };
    const input = {
      microUsd: 7650 + cacheRead.microUsd,
      pricedTokens: 3060 + cacheRead.pricedTokens,
      unpricedTokens: cacheRead.unpricedTokens,
    };
    await setUsageEventOverrides(page, {
      ...completedUsage,
      apiEquivalent: scenario === "missing" ? completedUsage.apiEquivalent : {
        microUsd: input.microUsd + 13800, pricedTokens: input.pricedTokens + 920, unpricedTokens: input.unpricedTokens,
      },
      apiEquivalentBreakdown: scenario === "missing" ? undefined : { ...completedUsage.apiEquivalentBreakdown, input, cacheRead },
    });
    await page.getByRole("button", { name: "Usage", exact: true }).click();
    await page.getByRole("button", { name: "Request details: req_synthetic_local", exact: true }).click();
    const tokens = page.locator(".request-token-details");
    const cache = tokens.locator(".request-details-token-child").filter({ hasText: "Cache reads" });
    if (scenario === "missing") {
      await expect(tokens.locator(".request-token-cost:not(.request-token-included)")).toHaveText(["—", "—", "—", "—", "—"]);
    } else if (scenario === "partial") {
      await expect(cache.locator(".request-token-cost")).toHaveText("—");
      await expect(tokens.locator(".request-details-token-group-heading .request-token-cost").first()).toHaveText("≈$0.00765");
      await expect(tokens.locator(".request-details-token-group-heading .request-token-cost").first()).toHaveAttribute("data-relay-tooltip", /Unpriced tokens: 15360/);
      await expect(tokens.locator(".request-token-price-note")).toHaveCount(0);
    } else {
      await expect(cache.locator(".request-token-cost")).toHaveText("≈$0.000001");
      await expect(tokens.locator(".request-details-token-group-heading .request-token-cost").first()).toHaveText("≈$0.007651");
    }
  });
}

for (const total of [0, null] as const) {
  test(`token panel preserves ${total === 0 ? "reported zero" : "unavailable"} total`, async ({ page }) => {
    await page.setViewportSize({ width: 360, height: 900 });
    await installTauriMock(page, { mode: "local", locale: "en", populated: true });
    await page.goto("/");
    await setUsageEventOverrides(page, {
      inputTokens: total === 0 ? 0 : 3060, outputTokens: total === 0 ? 0 : null,
      cachedInputTokens: null, cacheWriteInputTokens: null, reasoningTokens: null,
      totalTokens: total, apiEquivalentBreakdown: undefined,
    });
    await page.getByRole("button", { name: "Usage", exact: true }).click();
    await page.getByRole("button", { name: "Request details: req_synthetic_local", exact: true }).click();
    const panel = page.getByRole("region", { name: "Tokens", exact: true });
    await expect(panel.locator(".request-details-token-group")).toHaveCount(2);
    if (total === 0) {
      await expect(panel.locator(".request-token-total dd")).toHaveText("0");
      await expect(panel.locator(".request-details-token-group-heading .request-token-count")).toHaveText(["0", "0"]);
    } else {
      await expect(panel.locator(".request-token-total")).toHaveCount(0);
      await expect(panel.locator(".request-details-token-group-heading .request-token-count")).toHaveText(["3,060", "—"]);
    }
  });
}

test("separate token cards and parameters fit a short desktop window", async ({ page }) => {
  await page.setViewportSize({ width: 968, height: 716 });
  await installTauriMock(page, { mode: "local", locale: "ru", theme: "dark", populated: true });
  await page.goto("/");
  await setUsageEventOverrides(page, {
    ...completedUsage,
    inputTokens: 264820, cachedInputTokens: 257440, cacheWriteInputTokens: 560,
    outputTokens: 1268, reasoningTokens: 492, totalTokens: 266088,
    effectiveReasoningEffort: "xhigh",
    apiEquivalent: { microUsd: 389040, pricedTokens: 265528, unpricedTokens: 560 },
    apiEquivalentBreakdown: {
      input: { microUsd: 325640, pricedTokens: 264260, unpricedTokens: 560 },
      uncachedInput: { microUsd: 68200, pricedTokens: 6820, unpricedTokens: 0 },
      cacheRead: { microUsd: 257440, pricedTokens: 257440, unpricedTokens: 0 },
      cacheWrite: { microUsd: 0, pricedTokens: 0, unpricedTokens: 560 },
      output: { microUsd: 63400, pricedTokens: 1268, unpricedTokens: 0 },
    },
  });
  await page.getByRole("button", { name: "Использование", exact: true }).click();
  await page.getByRole("button", { name: "Сведения о запросе: req_synthetic_local", exact: true }).click();
  const dialog = page.getByRole("dialog");
  await expect(dialog.locator(".request-details-token-group-heading .request-token-count")).toHaveText(["264 820", "1 268"]);
  await expect(dialog.locator(".request-details-token-group-heading .request-token-cost")).toHaveText(["≈0,32564 $", "≈0,0634 $"]);
  await expect(dialog.locator(".request-token-footer, .request-token-price-note, .request-cache-retention")).toHaveCount(0);
  await expect(dialog.locator(".request-details-token-group-heading .request-token-cost").first()).toHaveAttribute("data-relay-tooltip", /Неоценённых токенов: 560/);
  await expect(dialog.locator(".request-parameters-section")).toContainText("Максимальное→Очень высокое");
  const bodySize = await dialog.locator(".relay-dialog-body").evaluate((element) => ({ content: element.scrollHeight, viewport: element.clientHeight }));
  expect(bodySize.content).toBeLessThanOrEqual(bodySize.viewport + 1);
  expect(await dialog.locator(".request-details-token-group, .request-details-parameters, dt, dd").evaluateAll((elements) =>
    elements.every((element) => element.scrollWidth <= element.clientWidth + 1))).toBe(true);
  await expect(dialog.locator(".request-token-total dd")).toHaveText("266 088");
  await dialog.locator(".request-token-details").screenshot({ path: "output/playwright/request-details-token-panel.png" });
  await dialog.screenshot({ path: "output/playwright/request-details-token-panel-dialog.png" });
  await dialog.locator(".request-details-parameters").screenshot({ path: "output/playwright/request-parameters-centered-reasoning-change.png" });
  await dialog.locator(".request-technical-details > summary").click();
  await expect(dialog.locator(".request-identifiers-section")).toBeVisible();
});
