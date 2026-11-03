import { expect, test } from "@playwright/test";

test("Hosted cards use the Desktop hierarchy with dense responsive columns", async ({ page }, testInfo) => {
  await page.goto("/admin/connections");
  const card = page.getByRole("article", { name: "Code account", exact: true });
  await expect(card.getByRole("meter")).toHaveCount(2);
  await expect(card.getByRole("meter", { name: "Остаток квоты: 5 часов" })).toHaveAttribute("aria-valuenow", "75");
  await expect(card.getByRole("meter", { name: "Остаток квоты: Неделя" })).toHaveAttribute("aria-valuenow", "58");
  await expect(card.locator(".owner-account-plan")).toHaveText("Plus");
  await expect(card.locator(".owner-account-route")).toHaveText("Excel / BPS");
  await expect(card.locator(".owner-account-subscription")).toContainText("26.11.2026");
  await expect(card.locator(".owner-account-values dt")).toHaveText(["API-экв. использ.", "Остаток недели", "Окупаемость"]);
  await expect(card.locator(".owner-resource-models")).toHaveCount(0);
  await expect(card.getByRole("button", { name: "Обновить квоты", exact: true })).toBeDisabled();
  await expect(card.getByRole("button", { name: "Действия: Code account", exact: true })).toHaveCSS("border-color", "rgba(0, 0, 0, 0)");
  const width = page.viewportSize()!.width;
  const grid = await page.locator(".owner-resource-grid:visible").evaluate(el => ({ columns: getComputedStyle(el).gridTemplateColumns.split(" ").length, width: el.clientWidth }));
  expect(grid.columns).toBe(width <= 639 ? 1 : Math.min(12, Math.max(1, Math.floor((grid.width + 10) / 290))));
  expect((await card.boundingBox())!.width).toBeCloseTo(width <= 639 ? grid.width : Math.min(340, (grid.width - (grid.columns - 1) * 10) / grid.columns), 0);
  const bottoms = await card.locator(".owner-account-values dd").evaluateAll(values => values.map(el => el.getBoundingClientRect().bottom));
  expect(Math.max(...bottoms) - Math.min(...bottoms)).toBeLessThan(1);
  expect(await card.evaluate(el => el.scrollWidth <= el.clientWidth)).toBe(true);
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  await page.screenshot({ path: testInfo.outputPath("desktop-account-anatomy.png"), fullPage: true });
});

test("wide viewports add card columns instead of stretching account and provider cards", async ({ page }, testInfo) => {
  test.skip(testInfo.project.name !== "desktop-light", "One dedicated wide-screen geometry matrix");
  for (const width of [1440, 1920, 2560, 3440]) {
    await page.setViewportSize({ width, height: 1000 });
    for (const path of ["connections", "pools"]) {
      await page.goto(`/admin/${path}`);
      const grid = page.locator(".owner-resource-grid:visible");
      await expect(grid.locator(".owner-resource-card").first()).toBeVisible();
      const gridSize = await grid.evaluate(el => ({ width: el.clientWidth, columns: getComputedStyle(el).gridTemplateColumns.split(" ").length }));
      const sizes = await grid.locator(".owner-resource-card").evaluateAll(cards => cards.map(card => ({ width: card.getBoundingClientRect().width, overflowing: card.scrollWidth > card.clientWidth })));
      expect(sizes.length).toBeGreaterThan(0);
      for (const size of sizes) {
        expect(size.width).toBeCloseTo(Math.min(340, (gridSize.width - (gridSize.columns - 1) * 10) / gridSize.columns), 0);
        expect(size.width).toBeGreaterThanOrEqual(280);
        expect(size.overflowing).toBe(false);
      }
      expect(gridSize.columns).toBe(Math.floor((gridSize.width + 10) / 290));
      expect(await page.locator(".relay-content").evaluate(el => el.scrollWidth <= el.clientWidth)).toBe(true);
      await page.screenshot({ path: testInfo.outputPath(`${path}-${width}-compact-cards.png`) });
    }
  }
});

test("card action menu supports keyboard, dismissal and viewport containment", async ({ page }) => {
  await page.goto("/admin/connections");
  const trigger = page.getByRole("button", { name: "Действия: Code account", exact: true });
  const menu = page.getByRole("menu", { name: "Действия: Code account", exact: true });
  await trigger.focus();
  await page.keyboard.press("ArrowDown");
  await expect(page.getByRole("menuitem", { name: "Приостановить", exact: true })).toBeFocused();
  await page.keyboard.press("End");
  await expect(page.getByRole("menuitem", { name: "В корзину", exact: true })).toBeFocused();
  await page.keyboard.press("Home");
  await expect(page.getByRole("menuitem", { name: "Приостановить", exact: true })).toBeFocused();
  await page.keyboard.press("Escape");
  await expect(menu).toHaveCount(0);
  await expect(trigger).toBeFocused();
  await trigger.click();
  const bounds = (await menu.boundingBox())!;
  expect(bounds.x).toBeGreaterThanOrEqual(8);
  expect(bounds.y).toBeGreaterThanOrEqual(8);
  expect(bounds.x + bounds.width).toBeLessThanOrEqual(page.viewportSize()!.width - 7);
  expect(bounds.y + bounds.height).toBeLessThanOrEqual(page.viewportSize()!.height - 7);
  await page.getByRole("heading", { name: "Подключения", exact: true }).click();
  await expect(menu).toHaveCount(0);
  await trigger.click();
  await page.keyboard.press("Tab");
  await expect(menu).toHaveCount(0);
});

test("long synthetic names stay on one line and missing telemetry never becomes fake quota", async ({ page }, testInfo) => {
  await page.goto("/admin/connections");
  await page.getByRole("button", { name: "Добавить подключение", exact: true }).click();
  const name = "СинтетическийАккаунтБезТелеметрии".repeat(2);
  await page.getByRole("dialog").getByLabel("Имя", { exact: true }).fill(name);
  await page.getByRole("dialog").getByRole("button", { name: "Добавить", exact: true }).click();
  const card = page.getByRole("article", { name, exact: true });
  await expect(card.getByRole("heading", { name, exact: true })).toHaveCSS("text-overflow", "ellipsis");
  await expect(card.getByRole("heading", { name, exact: true })).toHaveAttribute("data-relay-tooltip", name);
  const header = (await card.locator("header").boundingBox())!;
  const firstHeader = (await page.getByRole("article", { name: "Code account", exact: true }).locator("header").boundingBox())!;
  expect(header.height).toBe(firstHeader.height);
  await expect(card.getByRole("status")).toHaveText("Квоты ещё не получены");
  await expect(card.getByRole("meter")).toHaveCount(0);
  await expect(card.locator(".owner-account-subscription, .owner-account-token")).toHaveCount(0);
  await expect(card.locator(".owner-account-values dd")).toHaveText(["—", "—", "—"]);
  await expect(card.getByRole("button", { name: "Обновить квоты", exact: true })).toBeDisabled();
  expect(await card.evaluate(el => el.scrollWidth <= el.clientWidth)).toBe(true);
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  await card.scrollIntoViewIfNeeded();
  await page.screenshot({ path: testInfo.outputPath("account-unknown-long-name.png") });
});
