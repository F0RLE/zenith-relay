import { expect, test } from "@playwright/test";

test.beforeEach(async ({ page }) => {
  await page.goto("/app/overview");
  await expect(page.locator(".overview-chart-stack")).toBeVisible();
});

test("overview uses a headline chart and responsive related chart pairs", async ({ page }, testInfo) => {
  const charts = page.locator(".overview-chart-stack > .overview-chart");
  await expect(charts).toHaveCount(5);
  await expect(page.getByRole("img", { name: /Точные значения доступны/ })).toHaveCount(5);
  await expect(page.locator(".overview-note, .overview-bottom, .preview-note, .topbar")).toHaveCount(0);
  await expect(page.locator(".overview-chart.tokens .overview-chart-summary")).toHaveText("94,3 тыс.");
  await expect(page.locator(".overview-token-trend-summary small")).toHaveText(/Попадания в кэш: 32\s%/);
  await expect(page.locator(".overview-chart.tokens .overview-chart-summary")).toHaveAttribute("data-relay-tooltip", /^94\s280 токенов · 24 из 28 запросов с данными$/);
  await expect(page.locator(".overview-chart.cost .overview-chart-summary")).toHaveText("0,041544 USD");
  expect(await charts.evaluateAll(items => items.every((item, index) => {
    const bounds = item.getBoundingClientRect(), previous = items[index - 1]?.getBoundingClientRect();
    const plot = item.querySelector('.overview-chart-plot, .overview-token-trend-plot')!.getBoundingClientRect();
    const separated = !previous || bounds.top >= previous.bottom - 1
      || (Math.abs(bounds.top - previous.top) <= 1 && bounds.left >= previous.right);
    return item.scrollWidth <= item.clientWidth && separated
      && plot.width > 70 && plot.left >= 0 && plot.right <= innerWidth;
  }))).toBe(true);
  const size = page.viewportSize()!;
  if (size.width >= 640) {
    const sidebar = page.locator(".relay-sidebar");
    await expect(sidebar).toHaveCSS("width", size.width >= 1024 ? "232px" : "56px");
    await page.getByRole("button", { name: size.width >= 1024 ? "Свернуть меню" : "Развернуть меню", exact: true }).click();
    await expect(sidebar).toHaveCSS("width", size.width >= 1024 ? "56px" : "232px");
    await expect(page.getByRole("button", { name: "Обзор", exact: true })).toBeVisible();
  }
  if (size.width >= 1024) {
    await expect(page.locator(".desktop-overview .page-heading")).toHaveCSS("min-height", "88px");
  }
  await expect(page.locator(".overview-metrics")).toHaveCount(0);
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  await page.screenshot({ path: testInfo.outputPath("desktop-parity-overview.png") });
});

test("overview charts follow the command header without a counter strip or utility overlap", async ({ page }, testInfo) => {
  await expect(page.locator(".overview-heading h1")).toHaveText("Обзор");
  await expect(page.locator(".overview-heading").getByRole("combobox", { name: "Модель статистики" })).toBeVisible();
  await expect(page.locator(".overview-heading").getByRole("group", { name: "Период статистики" })).toBeVisible();
  await expect(page.locator(".overview-analytics-header, .overview-metrics, [aria-label='Сводка запросов']")).toHaveCount(0);
  await expect(page.locator(".overview-heading + .overview-analytics")).toHaveCount(1);
  const checkHeader = async () => {
    const layout = await page.locator(".hosted-overview").evaluate(element => {
      const tools = document.querySelector(".hosted-tools")!.getBoundingClientRect();
      const header = element.querySelector(".overview-heading")!.getBoundingClientRect();
      const charts = element.querySelector(".overview-chart-stack")!.getBoundingClientRect();
      const parts = Array.from(element.querySelectorAll(".overview-heading h1, .overview-controls .option-menu, .overview-controls .relay-tabs"));
      const bounds = parts.map(part => part.getBoundingClientRect());
      const apart = (a: DOMRect, b: DOMRect) => a.bottom <= b.top + 1 || a.top >= b.bottom - 1 || a.right <= b.left + 1 || a.left >= b.right - 1;
      return {
        noOverlap: bounds.every((part, i) => apart(part, tools) && bounds.slice(i + 1).every(next => apart(part, next))),
        within: bounds.every(part => part.left >= header.left && part.right <= header.right + 1),
        gap: charts.top - header.bottom,
        chartTop: charts.top,
        overflow: element.scrollWidth > element.clientWidth,
      };
    });
    expect(layout.noOverlap).toBe(true);
    expect(layout.within).toBe(true);
    expect(layout.gap).toBeCloseTo(page.viewportSize()!.width < 640 ? 16 : 20, 0);
    expect(layout.chartTop).toBeLessThan(340);
    expect(layout.overflow).toBe(false);
  };
  await checkHeader();
  if (page.viewportSize()!.width >= 640) {
    await page.locator(".hosted-mode").click();
    await checkHeader();
  }
  await page.screenshot({ path: testInfo.outputPath("overview-compact-header.png") });
});

test("desktop chart presentation preserves missing observations and exact UTC windows", async ({ page }) => {
  const tokens = page.locator(".overview-chart.tokens");
  const points = tokens.locator(".hosted-chart-dot");
  await expect(points.first()).toHaveAttribute("data-interval", /20.10.2026, 07:32 — 20.10.2026, 08:32 UTC/);
  expect(await points.evaluateAll(items => items.some(item => /^20\.10\.2026, 0[0-6]:/.test(item.getAttribute("data-interval") ?? "")))).toBe(false);
  await expect(page.locator("#overview-statistics")).toHaveAttribute("data-relay-tooltip", /19.10.2026, 14:32 — 20.10.2026, 14:32 UTC/);
  const speed = page.locator(".overview-chart.speed");
  await expect(speed.locator(".overview-chart-summary")).toHaveAttribute("data-relay-tooltip", /Выходные токены.*Измерено 24 из 28 запросов/);
  await expect(speed.locator(".hosted-chart-dot").last()).toHaveAttribute("data-interval", /14:32 UTC/);
  await speed.locator(".hosted-chart-canvas").focus();
  await expect(speed.getByRole("tooltip")).toContainText("13:32 — 20.10.2026, 14:32 UTC");
  await expect(speed.getByRole("tooltip")).not.toContainText("Неполный интервал");
  await page.getByRole("button", { name: "7 дней", exact: true }).click();
  await expect(page.locator("#overview-statistics")).toHaveAttribute("data-relay-tooltip", /13.10.2026, 14:32 — 20.10.2026, 14:32 UTC/);
  await page.getByRole("combobox", { name: "Модель статистики", exact: true }).click();
  await page.getByRole("option", { name: "GPT", exact: true }).click();
  await expect(tokens.locator(".overview-chart-summary")).toHaveText("28,8 тыс.");
  const requestTotal = page.locator(".overview-chart.requests .overview-chart-summary");
  await expect(requestTotal).not.toHaveText("28");
  const data = page.getByRole("table", { name: "Данные графиков", exact: true });
  await expect(data.locator("tbody tr")).toHaveCount(7);
  const bucketTotal = await data.locator("tbody td:nth-child(2)").evaluateAll(cells => cells.reduce((total, cell) => total + Number(cell.textContent!.replace(/\s/g, "")), 0));
  await expect(requestTotal).toHaveText(String(bucketTotal));
  await page.getByRole("button", { name: "30 дней", exact: true }).click();
  await expect(data.locator("tbody tr")).toHaveCount(30);
  await expect(page.locator("#overview-statistics")).toHaveAttribute("data-relay-tooltip", /20.09.2026, 14:32 — 20.10.2026, 14:32 UTC/);
});

test("charts use available desktop width without the old dashboard cap", async ({ page }, testInfo) => {
  test.skip(!testInfo.project.name.startsWith("desktop"), "Desktop layout width");
  await page.setViewportSize({ width: 2560, height: 1200 });
  const content = (await page.locator(".desktop-overview").boundingBox())!;
  expect(content.width).toBeGreaterThan(2000);
  expect(content.x).toBe(264);
  const charts = await page.locator(".overview-chart-stack > article").evaluateAll(items => items.map(item => item.getBoundingClientRect().width));
  expect(new Set(charts.slice(1)).size).toBe(1);
  expect(Math.abs(charts[0]! - charts[1]! * 2 - 20)).toBeLessThanOrEqual(1);
  await page.getByRole("button", { name: "7 дней", exact: true }).click();
  await expect(page.getByRole("table", { name: "Данные графиков", exact: true }).locator("tbody tr")).toHaveCount(7);
  await page.screenshot({ path: testInfo.outputPath("desktop-parity-wide.png") });
});

test("header theme remains interactive and persists without a settings page", async ({ page }) => {
  const root = page.locator(".hosted-app");
  const original = await root.getAttribute("data-theme");
  const next = original === "light" ? "dark" : "light";
  await page.getByRole("button", { name: /^(Тёмная|Светлая) тема$/ }).click();
  await expect(root).toHaveAttribute("data-theme", next);
  await page.reload();
  await expect(root).toHaveAttribute("data-theme", next);
  await expect(page.getByRole("button", { name: "Настройки", exact: true, includeHidden: true })).toHaveCount(0);
  await expect(page.locator(".language-switcher").getByRole("combobox")).toBeVisible();
});

test("responsive sidebar and focused chart tooltips stay within the browser", async ({ page }) => {
  await page.setViewportSize({ width: 1440, height: 1000 });
  await expect(page.locator(".relay-sidebar")).toHaveCSS("width", "232px");
  await page.setViewportSize({ width: 768, height: 1000 });
  await expect(page.locator(".relay-sidebar")).toHaveCSS("width", "56px");
  await page.getByRole("button", { name: "Развернуть меню", exact: true }).click();
  await page.setViewportSize({ width: 1440, height: 1000 });
  await page.setViewportSize({ width: 768, height: 1000 });
  await expect(page.locator(".relay-sidebar")).toHaveCSS("width", "232px");
  await page.setViewportSize({ width: 320, height: 740 });
  const plot = page.locator(".tokens .hosted-chart-canvas");
  await page.keyboard.press("Tab");
  await plot.focus();
  const tooltip = page.locator(".tokens").getByRole("tooltip");
  await expect(tooltip).toBeVisible();
  const bounds = (await tooltip.boundingBox())!;
  expect(bounds.x).toBeGreaterThanOrEqual(0);
  expect(bounds.x + bounds.width).toBeLessThanOrEqual(320);
  expect(await page.locator(".relay-content").evaluate(element => element.scrollWidth <= element.clientWidth)).toBe(true);
  await expect(page.locator(".overview-token-trend .overview-chart-x-axis, .hosted-chart-x-axis")).toHaveCount(0);
});

test("usage tabs and filter actions stay clear of shell tools", async ({ page }, testInfo) => {
  await page.goto("/app/usage");
  await expect(page.locator(".usage-overview")).toBeVisible();
  await page.getByRole("group", { name: "Представление использования" }).getByRole("button", { name: "Модели", exact: true }).click();
  const header = page.locator(".usage-page .relay-workspace-header");
  const checkHeader = async () => {
    const layout = await header.evaluate(element => {
      const bounds = (selector: string) => document.querySelector(selector)!.getBoundingClientRect().toJSON();
      return { header: element.getBoundingClientRect().toJSON(), tabs: bounds(".usage-page .relay-page-navigation"), filters: bounds(".usage-filters"), actions: bounds(".usage-key-controls .option-trigger"), refresh: bounds(".usage-refresh"), tools: bounds(".hosted-tools") };
    });
    expect(layout.tabs.bottom).toBeLessThanOrEqual(layout.header.bottom + 1);
    expect(layout.filters.top).toBeGreaterThanOrEqual(layout.header.bottom - 1);
    expect(layout.actions.left).toBeGreaterThanOrEqual(layout.filters.left);
    expect(layout.actions.right).toBeLessThanOrEqual(layout.filters.right + 1);
    expect(layout.refresh.left).toBeGreaterThanOrEqual(layout.actions.left);
    expect(layout.refresh.right).toBeLessThanOrEqual(layout.filters.right + 1);
    expect(layout.actions.top >= layout.tools.bottom || layout.actions.right <= layout.tools.left).toBe(true);
    expect(await header.evaluate(element => element.scrollWidth <= element.clientWidth)).toBe(true);
    expect(await page.locator(".relay-content").evaluate(element => element.scrollWidth <= element.clientWidth)).toBe(true);
  };
  await checkHeader();
  if (page.viewportSize()!.width >= 640) {
    await page.locator(".hosted-mode").click();
    await checkHeader();
  }
  await page.getByRole("button", { name: "Обновить использование", exact: true }).click();
  await expect(page.locator(".usage-page .form-note[role='status']")).toHaveCount(0);
  await page.screenshot({ path: testInfo.outputPath("usage-header.png") });
});
