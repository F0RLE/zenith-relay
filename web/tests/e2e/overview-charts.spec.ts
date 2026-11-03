import { expect, test } from "@playwright/test";

test.beforeEach(async ({ page }) => {
  await page.goto("/app/overview");
  await expect(page.locator(".tokens .hosted-chart-canvas")).toBeVisible();
});

for (const [locale, label] of [["en", "All time"], ["zh-CN", "全部时间"]] as const) {
  test(`admin all-time controls fit in ${locale}`, async ({ page }) => {
    await page.addInitScript(value => localStorage.setItem("relay-hosted-locale", value), locale);
    await page.goto("/admin/overview");
    const period = page.locator(".overview-controls .relay-tabs");
    await expect(period.getByRole("button")).toHaveCount(4);
    await period.getByRole("button", { name: label, exact: true }).click();
    await expect(period.getByRole("button", { name: label, exact: true })).toHaveAttribute("aria-pressed", "true");
    expect(await page.locator(".relay-content").evaluate(element => element.scrollWidth <= element.clientWidth)).toBe(true);
    const periodBounds = await period.boundingBox();
    expect(await period.evaluate(element => element.scrollHeight <= element.clientHeight)).toBe(true);
    for (const button of await period.getByRole("button").all()) {
      const bounds = await button.boundingBox();
      expect(bounds!.x).toBeGreaterThanOrEqual(0);
      expect(bounds!.x + bounds!.width).toBeLessThanOrEqual(page.viewportSize()!.width + 1);
      expect(bounds!.y).toBeGreaterThanOrEqual(periodBounds!.y - 1);
      expect(bounds!.y + bounds!.height).toBeLessThanOrEqual(periodBounds!.y + periodBounds!.height + 1);
      expect(await button.evaluate(element => element.scrollWidth <= element.clientWidth)).toBe(true);
    }
  });
}

test("charts have readable scales and one combined exact interval tooltip", async ({ page }) => {
  const tokens = page.locator(".tokens.hosted-chart");
  await expect(tokens.locator(".hosted-chart-axis").first().locator("span")).toHaveText(["20 тыс.", "10 тыс.", "0"]);
  await expect(page.locator(".hosted-chart-rate-axis")).toHaveCount(1);
  await expect(page.locator(".overview-chart-y-axis-right")).toHaveCount(0);
  for (const tone of ["cost", "requests", "speed", "e2e-speed"]) await expect(page.locator(`.${tone}.hosted-chart .hosted-chart-axis`)).toHaveCount(1);
  const plot = tokens.locator(".hosted-chart-canvas");
  await plot.focus();
  const tooltip = tokens.getByRole("tooltip");
  await expect(tooltip).toBeVisible();
  await expect(tooltip).toContainText("14:32 UTC");
  await expect(tooltip).toContainText("13:32 — 20.10.2026, 14:32 UTC");
  await expect(tooltip).not.toContainText("Неполный интервал");
  await expect(tooltip.locator("dl > div")).toHaveCount(5);
  const input = Number(await tokens.locator(".hosted-chart-dot.is-input").last().getAttribute("data-value"));
  await expect(tooltip.locator(".is-input dd")).toHaveText(input.toLocaleString("ru-RU", { maximumFractionDigits: 6 }));
  await plot.press("ArrowLeft");
  await expect(tooltip).toContainText("12:32 — 20.10.2026, 13:32 UTC");
  await expect(tooltip).not.toContainText("Неполный интервал");
  await plot.press("Home");
  await expect(tooltip.locator("dd")).toHaveText(["—", "—", "—", "—", "—"]);
  await plot.press("Escape");
  await expect(tooltip).toHaveCount(0);
});

test("legend toggles only rendered series, preserves totals and restores empty selection", async ({ page }) => {
  const chart = page.locator(".tokens.hosted-chart");
  const total = chart.locator(".overview-chart-summary");
  const legend = chart.getByRole("group", { name: "Показатели токенов" });
  const input = legend.getByRole("button", { name: "Вход", exact: true });
  await input.click();
  await expect(input).toHaveAttribute("aria-pressed", "false");
  await expect(chart.locator("path.is-input")).toHaveCount(0);
  await expect(total).toHaveText("94,3 тыс.");
  for (const label of ["Выход", "Создание кэша", "Чтение кэша", "Попадания в кэш"]) await legend.getByRole("button", { name: label, exact: true }).click();
  await expect(chart.locator(".hosted-chart-empty")).toHaveText("Выберите показатель");
  await expect(chart.locator(".hosted-chart-rate-axis")).toHaveCount(0);
  await input.click();
  await expect(chart.locator(".hosted-chart-empty")).toHaveCount(0);
  await expect(chart.locator("path.is-input")).toHaveCount(1);
  await expect(total).toHaveText("94,3 тыс.");
});

for (const area of ["app", "admin"]) {
  test(`${area} legend keeps markers and labels inset in every interaction state`, async ({ page }, testInfo) => {
    if (area === "admin") await page.goto("/admin/overview");
    const chart = page.locator(".tokens.hosted-chart");
    const legend = chart.getByRole("group", { name: "Показатели токенов" });
    const output = legend.getByRole("button", { name: "Выход", exact: true });
    await expect(output).toBeVisible();
    await page.evaluate(() => document.fonts.ready);

    async function expectInsetContent() {
      const items = await legend.getByRole("button").evaluateAll(buttons => buttons.map(button => {
        const bounds = button.getBoundingClientRect();
        const marker = button.querySelector("i")!.getBoundingClientRect();
        const label = [...button.childNodes].find(node => node.nodeType === Node.TEXT_NODE && node.textContent?.trim())!;
        const range = document.createRange();
        range.selectNode(label);
        const text = range.getBoundingClientRect();
        const group = button.parentElement!.getBoundingClientRect();
        return {
          label: button.textContent!.trim(),
          startInset: marker.left - bounds.left,
          endInset: bounds.right - text.right,
          lineCount: range.getClientRects().length,
          row: Math.round(bounds.top),
          fits: button.scrollWidth <= button.clientWidth && bounds.left >= group.left && bounds.right <= group.right + 1,
          height: bounds.height,
        };
      }));
      expect(items).toHaveLength(5);
      if (page.viewportSize()!.width <= 639) expect(new Set(items.map(item => item.row)).size).toBeLessThanOrEqual(3);
      for (const item of items) {
        expect(item.startInset, item.label).toBeCloseTo(8, 1);
        expect(item.endInset, item.label).toBeCloseTo(8, 1);
        expect(item.lineCount, item.label).toBe(1);
        expect(item.fits, item.label).toBe(true);
        expect(item.height, item.label).toBeGreaterThanOrEqual(page.viewportSize()!.width <= 639 ? 44 : 32);
      }
    }

    await expectInsetContent();
    await output.hover();
    await expect(output).not.toHaveCSS("background-color", "rgba(0, 0, 0, 0)");
    await expectInsetContent();
    await page.screenshot({ path: testInfo.outputPath(`${area}-legend-hover.png`) });
    await page.mouse.move(1, 1);
    await legend.getByRole("button", { name: "Вход", exact: true }).focus();
    await page.keyboard.press("Tab");
    await expect(output).toBeFocused();
    await expectInsetContent();
    await output.press("Space");
    await expect(output).toHaveAttribute("aria-pressed", "false");
    await expect(chart.locator("path.is-output")).toHaveCount(0);
    await expectInsetContent();
    await output.press("Space");
    await expect(output).toHaveAttribute("aria-pressed", "true");
    await expect(chart.locator("path.is-output")).toHaveCount(1);
  });
}

test("rolling windows retain exact buckets without visible time labels or overflow", async ({ page }) => {
  for (const [period, count] of [["24 часа", 24], ["7 дней", 7], ["30 дней", 30]] as const) {
    await page.getByRole("button", { name: period, exact: true }).click();
    for (const chart of await page.locator(".hosted-chart").all()) {
      await expect(chart.locator(".hosted-chart-x-axis")).toHaveCount(0);
      await expect(chart.locator(".hosted-chart-points > li")).toHaveCount(count);
      expect(await chart.evaluate(el => el.scrollWidth <= el.clientWidth)).toBe(true);
    }
    await expect(page.getByRole("table", { name: "Данные графиков", exact: true }).locator("tbody tr")).toHaveCount(count);
  }
});

test("month tooltip works across the plot, respects the viewport and has one keyboard stop", async ({ page }, testInfo) => {
  await page.getByRole("button", { name: "30 дней", exact: true }).click();
  const chart = page.locator(".tokens.hosted-chart");
  const plot = chart.locator(".hosted-chart-canvas");
  await plot.hover({ position: { x: 2, y: 40 } });
  const tooltip = chart.getByRole("tooltip");
  await expect(tooltip).toContainText("20.09.2026");
  await plot.focus();
  await plot.press("End");
  await expect(tooltip).toContainText("20.10.2026");
  const bounds = (await tooltip.boundingBox())!;
  expect(bounds.x).toBeGreaterThanOrEqual(0);
  expect(bounds.x + bounds.width).toBeLessThanOrEqual(page.viewportSize()!.width);
  expect(await tooltip.evaluate(el => el.scrollWidth <= el.clientWidth)).toBe(true);
  await expect(chart.locator(".hosted-chart-points [tabindex]")).toHaveCount(0);
  expect(await page.locator(".relay-content").evaluate(el => el.scrollWidth <= el.clientWidth)).toBe(true);
  await page.screenshot({ path: testInfo.outputPath("overview-month-tooltip.png") });
});
