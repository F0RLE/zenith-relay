import { expect, test } from "@playwright/test";

test.beforeEach(async ({ page }) => {
  await page.goto("/preview.html?page=overview");
  await expect(page.getByRole("heading", { name: "Использование во времени", exact: true })).toBeVisible();
});

test("Relay overview has aligned chart headings and totals without clipping", async ({ page }, testInfo) => {
  const panels = page.locator(".overview-chart-block");
  await expect(panels).toHaveCount(5);
  await expect(page.getByRole("img", { name: /Точные значения доступны/ })).toHaveCount(5);
  await expect(page.locator(".overview-note, .overview-bottom, .preview-note")).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Открыть запросы за этот период", exact: true })).toHaveCount(0);
  expect(await page.locator(".overview-analytics").evaluate(element => element === element.parentElement?.lastElementChild)).toBe(true);
  const tokens = page.getByRole("article", { name: "Использование токенов", exact: true });
  await expect(tokens.locator(".overview-chart-total strong")).toHaveText("94,3 тыс.");
  await expect(tokens.locator(".cache-hit-note")).toHaveText("31,9% попадания в кэш");
  await expect(tokens.locator(".chart-coverage")).toHaveText("Счётчики получены: 24 из 28 запросов");
  await expect(page.getByRole("article", { name: "Расходы", exact: true }).locator("strong")).toHaveText("0,041544 USD");
  expect(await panels.evaluateAll(elements => elements.every(panel => {
    const title = panel.querySelector("h3")!.getBoundingClientRect();
    const total = panel.querySelector(".overview-chart-total")!.getBoundingClientRect();
    const svg = panel.querySelector("svg[role='img']")!;
    const chart = svg.getBoundingClientRect();
    return panel.scrollWidth <= panel.clientWidth && title.right <= total.left
      && chart.left >= 0 && chart.right <= innerWidth && chart.width > 150;
  }))).toBe(true);
  expect(await page.locator(".overview-controls .option-trigger").evaluate(element => getComputedStyle(element).borderTopColor)).toBe("rgba(0, 0, 0, 0)");
  expect(await tokens.locator(".chart-legend span").evaluateAll(elements => new Set(elements.map(element => getComputedStyle(element).color)).size)).toBe(5);
  expect(await tokens.locator(".chart-legend span").evaluateAll(elements => elements.map(element => getComputedStyle(element).color))).toEqual([
    "rgb(57, 123, 214)", "rgb(19, 166, 138)", "rgb(196, 139, 27)", "rgb(23, 153, 182)", "rgb(118, 80, 177)",
  ]);
  await expect(page.getByRole("article", { name: "Расходы", exact: true }).locator("strong")).toHaveCSS("color", "rgb(173, 79, 130)");
  await expect(page.getByRole("article", { name: "Скорость с учётом ожидания", exact: true }).locator("strong")).toHaveCSS("color", "rgb(118, 80, 177)");
  const expectedWarning = await page.locator(".relay-app").evaluate(element => {
    const probe = document.createElement("span"); probe.style.color = "var(--warning)"; element.append(probe);
    const color = getComputedStyle(probe).color; probe.remove(); return color;
  });
  await expect(page.getByRole("article", { name: "Скорость генерации", exact: true }).locator("strong")).toHaveCSS("color", expectedWarning);
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  await page.screenshot({ path: testInfo.outputPath("overview.png"), fullPage: true });
});

test("chart scales stay readable and gaps do not invent token observations", async ({ page }) => {
  const tokens = page.getByRole("article", { name: "Использование токенов", exact: true });
  await expect(tokens.locator("svg text").filter({ hasText: /^20 тыс\.$/ })).toHaveCount(1);
  await expect(tokens.locator("circle title").first()).toContainText("20.10.2026, 08:00 — 20.10.2026, 09:00 UTC");
  await expect(tokens.locator("circle title").filter({ hasText: /^20\.10\.2026, 0[0-7]:00/ })).toHaveCount(0);
  await expect(page.locator(".overview-period")).toHaveText("20.10.2026, 00:00 — 20.10.2026, 14:32 UTC");
  await expect(page.locator(".overview-step")).toContainText("Точка на графике — 1 час");
  const speed = page.getByRole("article", { name: "Скорость генерации", exact: true });
  await expect(speed.locator(".chart-definition")).toContainText("Выходные токены / суммарное время генерации");
  await expect(speed.locator(".overview-chart-total small")).toHaveText("Измерено 24 из 28 запросов");
  await expect(speed.locator("circle title").last()).toContainText("14:32 UTC · неполный интервал");
  const costs = page.getByRole("article", { name: "Расходы", exact: true });
  await expect(costs.locator("svg text").filter({ hasText: /^0,01$/ })).toHaveCount(1);
  await page.getByRole("group", { name: "Период статистики" }).getByRole("button", { name: "7 дней", exact: true }).click();
  await expect(page.locator(".overview-step")).toContainText("Точка на графике — 1 день");
  await expect(page.locator(".overview-period")).toHaveText("14.10.2026, 00:00 — 20.10.2026, 14:32 UTC");
  await page.getByRole("combobox", { name: "Модель статистики", exact: true }).click();
  await page.getByRole("option", { name: "GPT · демо", exact: true }).click();
  await expect(tokens.locator(".overview-chart-total strong")).toHaveText("28,8 тыс.");
  const accessibleData = page.getByRole("table", { name: "Данные графиков", exact: true });
  await expect(accessibleData.locator("tbody tr")).toHaveCount(7);
  await expect(accessibleData.locator("tbody")).toContainText("20.10");
  await page.getByRole("button", { name: "30 дней", exact: true }).click();
  await expect(accessibleData.locator("tbody tr")).toHaveCount(30);
  await expect(page.locator(".overview-period")).toHaveText("21.09.2026, 00:00 — 20.10.2026, 14:32 UTC");
});

test("wide screens cap content and point spacing without changing the period", async ({ page }, testInfo) => {
  test.skip(!testInfo.project.name.startsWith("desktop"), "Wide viewport is covered in both desktop themes");
  await page.setViewportSize({ width: 2560, height: 1200 });
  await expect(page.locator(".overview-page")).toHaveCSS("max-width", "1280px");
  expect((await page.locator(".overview-page").boundingBox())!.width).toBeLessThanOrEqual(1280);
  await page.getByRole("button", { name: "7 дней", exact: true }).click();
  await expect.poll(() => page.locator(".overview-chart svg[role='img']").evaluateAll(elements => elements.every(element => {
    const chart = element as SVGSVGElement;
    return (chart.viewBox.baseVal.width - 104) / 7 <= 96.1;
  }))).toBe(true);
  await expect(page.getByRole("table", { name: "Данные графиков", exact: true }).locator("tbody tr")).toHaveCount(7);
  await expect(page.locator(".overview-period")).toContainText("14.10.2026, 00:00 — 20.10.2026, 14:32 UTC");
  await page.screenshot({ path: testInfo.outputPath("overview-wide-week.png"), fullPage: true });
});
