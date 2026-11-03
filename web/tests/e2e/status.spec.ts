import { expect, test } from "@playwright/test";
import { previewSnapshot } from "../../src/preview/snapshot";

test("status routes show the catalog without fabricating monitoring measurements", async ({ page }) => {
  const errors: string[] = [];
  page.on("pageerror", error => errors.push(error.message));
  for (const path of ["/app/status", "/preview.html?page=status"]) {
    await page.goto(path);
    const status = page.locator(".status-page");
    await expect(status.getByRole("heading", { name: "Статус", level: 1, exact: true })).toBeVisible();
    await expect(status.locator(".model-status-summary dd")).toHaveCount(4);
    await expect(status.locator(".model-status-monitor, .model-status-state")).toHaveCount(0);
    await expect(status).not.toContainText("Ожидаем данные мониторинга");
    for (const metric of await status.locator(".model-status-summary > div").all()) await expect(metric).toHaveCSS("text-align", "center");
    const metricPositions = await status.locator(".model-status-summary > div").evaluateAll(elements => elements.map(element => ({ row: element.getBoundingClientRect().top, value: element.querySelector("dd")!.getBoundingClientRect().top })));
    for (const left of metricPositions) for (const right of metricPositions) if (Math.abs(left.row - right.row) < 1) expect(Math.abs(left.value - right.value)).toBeLessThanOrEqual(1);
    await expect(status.locator(".model-status-summary")).not.toContainText(/\d/);
    await expect(status.locator(".model-status-row")).toHaveCount(previewSnapshot.models.length);
    await expect(status.locator(".model-status-row th")).toHaveText(previewSnapshot.models.map(model => model.name));
    for (const cell of await status.locator(".model-status-row > th, .model-status-row > td").all()) await expect(cell).toHaveCSS("text-align", "center");
    await expect(status.locator(".model-status-row:not([data-state='unknown'])")).toHaveCount(0);
    await expect(status.locator(".model-status-history > span:not([data-state='unknown'])")).toHaveCount(0);
    await expect(status.locator(".model-status-history").first().locator(":scope > span")).toHaveCount(48);
    await expect(status).not.toContainText(/Primary API|Backup API|Настройки маршрутизации|Источники/);
    await expect(page.locator(".sidebar-main-nav button[aria-current='page']")).toHaveAttribute("aria-label", "Статус");
    await expect(page.locator(".hosted-empty:visible")).toHaveCount(0);
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  }
  expect(errors).toEqual([]);
});

for (const locale of ["en", "ru", "zh-CN"] as const) {
  test(`status periods and legend stay compact in ${locale}`, async ({ page }, testInfo) => {
    await page.addInitScript(value => localStorage.setItem("relay-hosted-locale", value), locale);
    await page.goto("/app/status");
    const headingBar = page.locator(".status-heading");
    const period = headingBar.locator(".model-status-period");
    await expect(period).toHaveClass("relay-tabs model-status-period");
    await expect(period.getByRole("button")).toHaveCount(2);
    await expect(period.getByRole("button").first()).toHaveClass("active");
    await expect(period.getByRole("button").first()).toHaveAttribute("aria-pressed", "true");
    await expect(page.locator(".model-status-controls")).toHaveCount(0);
    await page.evaluate(() => document.fonts.ready);
    const titleBox = (await headingBar.getByRole("heading", { level: 1 }).boundingBox())!;
    const periodBox = (await period.boundingBox())!;
    const toolsBox = (await page.locator(".hosted-tools:visible").boundingBox())!;
    const headingSpace = await headingBar.evaluate(element => {
      const style = getComputedStyle(element);
      return { width: element.clientWidth - parseFloat(style.paddingLeft) - parseFloat(style.paddingRight), gap: parseFloat(style.columnGap) };
    });
    if (headingSpace.width >= titleBox.width + periodBox.width + headingSpace.gap) {
      expect(Math.abs(periodBox.x - titleBox.x - titleBox.width - headingSpace.gap)).toBeLessThan(1);
      expect(Math.abs(titleBox.y + titleBox.height / 2 - periodBox.y - periodBox.height / 2)).toBeLessThan(1);
    } else {
      expect(periodBox.y).toBeGreaterThanOrEqual(titleBox.y + titleBox.height);
    }
    for (const box of [titleBox, periodBox]) {
      expect(box.x + box.width <= toolsBox.x || box.x >= toolsBox.x + toolsBox.width
        || box.y + box.height <= toolsBox.y || box.y >= toolsBox.y + toolsBox.height).toBe(true);
    }
    const toolbar = page.locator(".model-status-toolbar");
    const legend = toolbar.locator(".model-status-legend");
    await expect(legend).toBeVisible();
    await expect(legend.locator("li")).toHaveCount(4);
    await expect(page.locator(".model-status-models > .model-status-legend")).toHaveCount(0);
    await page.evaluate(() => document.fonts.ready);
    await toolbar.scrollIntoViewIfNeeded();
    const layout = await toolbar.evaluate(element => ({
      width: element.getBoundingClientRect().width,
      fits: element.scrollWidth <= element.clientWidth,
      items: Array.from(element.children).map(child => {
        const bounds = child.getBoundingClientRect();
        return { x: bounds.x, y: bounds.y, width: bounds.width, height: bounds.height, fits: child.scrollWidth <= child.clientWidth + 1 };
      }),
    }));
    expect(layout.fits).toBe(true);
    expect(layout.items).toHaveLength(3);
    for (const item of layout.items) expect(item.fits).toBe(true);
    const [heading, states, search] = layout.items;
    if (layout.width > 820) {
      expect(heading!.x + heading!.width).toBeLessThanOrEqual(states!.x);
      expect(states!.x + states!.width).toBeLessThanOrEqual(search!.x);
      expect(Math.abs(heading!.y + heading!.height / 2 - search!.y - search!.height / 2)).toBeLessThan(1);
      expect(Math.abs(states!.y + states!.height / 2 - search!.y - search!.height / 2)).toBeLessThan(1);
    } else if (layout.width > 560) {
      expect(heading!.x + heading!.width).toBeLessThanOrEqual(search!.x);
      expect(states!.y).toBeGreaterThanOrEqual(Math.max(heading!.y + heading!.height, search!.y + search!.height));
    } else {
      expect(heading!.y + heading!.height).toBeLessThanOrEqual(states!.y);
      expect(states!.y + states!.height).toBeLessThanOrEqual(search!.y);
      expect(await legend.evaluate(element => getComputedStyle(element).display)).toBe("grid");
    }
    const frame = await page.locator(".model-status-frame").boundingBox();
    expect(frame!.y).toBeGreaterThanOrEqual(Math.max(...layout.items.map(item => item.y + item.height)));
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    await page.screenshot({ path: testInfo.outputPath(`status-controls-${locale}.png`) });
    await toolbar.getByRole("searchbox").fill("missing-model");
    await expect(page.locator(".model-status-row")).toHaveCount(0);
    await expect(legend).toBeVisible();
  });
}

test("interval details work with hover and keyboard, stay on-screen and follow filters", async ({ page }, testInfo) => {
  await page.goto("/app/status");
  await expect(page.getByRole("heading", { name: "Статус", level: 1, exact: true })).toBeVisible();
  const history = page.locator(".model-status-history").first();
  const tooltip = page.locator(".model-status-tooltip");
  await history.scrollIntoViewIfNeeded();
  await history.hover({ position: { x: 4, y: 20 } });
  await expect(tooltip).toBeVisible();
  await expect(tooltip.locator("dt")).toHaveText(["Доступность", "До первого токена", "Запросы", "Доля попаданий в кэш"]);
  await expect(tooltip.locator("dd")).toHaveCount(4);
  await expect(tooltip.locator("strong")).toContainText("UTC");
  const first = await tooltip.locator("strong").innerText();
  const bounds = await history.boundingBox();
  await history.hover({ position: { x: bounds!.width - 4, y: 20 } });
  await expect(tooltip.locator("strong")).not.toHaveText(first);
  const rect = await tooltip.boundingBox();
  const viewport = page.viewportSize()!;
  expect(rect!.x).toBeGreaterThanOrEqual(7);
  expect(rect!.y).toBeGreaterThanOrEqual(7);
  expect(rect!.x + rect!.width).toBeLessThanOrEqual(viewport.width - 7);
  expect(rect!.y + rect!.height).toBeLessThanOrEqual(viewport.height - 7);
  await page.screenshot({ path: testInfo.outputPath("status-tooltip.png") });
  await page.keyboard.press("Escape");
  await expect(tooltip).toHaveCount(0);
  await history.focus();
  await history.press("Home");
  await expect(history.locator(":scope > span").first()).toHaveAttribute("data-active", "true");
  await history.press("ArrowRight");
  await expect(history.locator(":scope > span").nth(1)).toHaveAttribute("data-active", "true");
  await history.press("End");
  await expect(history.locator(":scope > span").last()).toHaveAttribute("data-active", "true");
  await history.press("Escape");
  await expect(tooltip).toHaveCount(0);
  await page.getByRole("button", { name: "7 дней", exact: true }).click();
  await expect(page.locator(".status-heading").getByRole("button", { name: "7 дней", exact: true })).toHaveClass("active");
  await expect(page.locator(".status-heading").getByRole("button", { name: "24 часа", exact: true })).toHaveAttribute("aria-pressed", "false");
  await expect(history.locator(":scope > span")).toHaveCount(28);
  await page.getByRole("searchbox", { name: "Поиск моделей" }).fill("cLaUdE");
  await expect(page.locator(".model-status-row")).toHaveCount(1);
  await expect(page.locator(".model-status-name")).toHaveText("Claude");
  await page.getByRole("searchbox", { name: "Поиск моделей" }).fill("missing-model");
  await expect(page.locator(".model-status-row")).toHaveCount(0);
  await expect(page.getByRole("status")).toHaveText("Модели не найдены");
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
});

test.describe("touch history", () => {
  test.use({ hasTouch: true });
  test("tap opens interval details and tapping outside dismisses them", async ({ page }) => {
    await page.goto("/app/status");
    await expect(page.getByRole("heading", { name: "Статус", level: 1, exact: true })).toBeVisible();
    const history = page.locator(".model-status-history").first();
    await history.tap({ position: { x: 4, y: 20 } });
    await expect(page.locator(".model-status-tooltip")).toBeVisible();
    await page.getByRole("heading", { name: "Статус", level: 1, exact: true }).tap();
    await expect(page.locator(".model-status-tooltip")).toHaveCount(0);
  });
});

test("status navigation supports desktop, mobile, reload and browser history", async ({ page }, testInfo) => {
  await page.goto("/app/models");
  await expect(page.getByRole("heading", { name: "Модели и цены", level: 1, exact: true })).toBeVisible();
  const status = page.locator(".sidebar-main-nav").getByRole("button", { name: "Статус", exact: true, includeHidden: true });
  if (!await status.isVisible()) {
    await page.getByRole("button", { name: "Меню", exact: true }).click();
    await expect(status).toBeVisible();
    await page.screenshot({ path: testInfo.outputPath("status-sidebar.png") });
  }
  await status.click();
  await expect(page).toHaveURL(/\/app\/status$/);
  await expect(page.getByRole("heading", { name: "Статус", level: 1, exact: true })).toBeVisible();
  await expect(status).toHaveAttribute("aria-current", "page");
  await expect(page.locator(".relay-sidebar")).toHaveAttribute("data-open", "false");
  await expect(page.getByRole("dialog")).toHaveCount(0);
  await page.screenshot({ path: testInfo.outputPath("status-page.png") });
  await page.reload();
  await expect(page.getByRole("heading", { name: "Статус", level: 1, exact: true })).toBeVisible();
  await page.goBack();
  await expect(page.getByRole("heading", { name: "Модели и цены", level: 1, exact: true })).toBeVisible();
  await page.goForward();
  await expect(page.getByRole("heading", { name: "Статус", level: 1, exact: true })).toBeVisible();
});
