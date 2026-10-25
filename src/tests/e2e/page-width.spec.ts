import { expect, test, type Page } from "../bun-playwright";
import { installTauriMock } from "./tauri-mock";

const appearances = [
  { locale: "ru", theme: "dark", labels: ["Обзор", "Подключения", "Пул", "Доступ", "Использование", "Восстановление", "Настройки", "Помощь"], collapse: "Свернуть боковую панель" },
  { locale: "en", theme: "light", labels: ["Overview", "Connections", "Pool", "Access", "Usage", "Recovery", "Settings", "Help"], collapse: "Collapse sidebar" },
] as const;
const pageIds = ["overview", "connections", "pool", "integrations", "usage", "profiles", "settings", "help"] as const;

async function expectPageFits(page: Page) {
  await expect.poll(() => page.locator(".relay-page").evaluate((element) => {
    const content = element.closest<HTMLElement>(".relay-content");
    if (!content) return null;
    const bounds = element.getBoundingClientRect();
    const parent = content.getBoundingClientRect();
    const header = element.querySelector(".relay-page-header")?.getBoundingClientRect();
    return {
      fillsWorkspace: Math.abs(bounds.width - content.clientWidth) <= 1,
      leftAligned: Math.abs(bounds.left - parent.left) <= 1,
      headerFits: Boolean(header && header.left >= bounds.left && header.right <= bounds.right),
      contentFits: content.scrollWidth <= content.clientWidth,
    };
  })).toEqual({ fillsWorkspace: true, leftAligned: true, headerFits: true, contentFits: true });
}

for (const appearance of appearances) {
  test(`pages use the desktop workspace with bounded controls ${appearance.locale} ${appearance.theme}`, async ({ page }) => {
    test.setTimeout(90_000);
    await installTauriMock(page, { locale: appearance.locale, theme: appearance.theme, mode: "local", populated: true, accountCount: 5 });
    await page.setViewportSize({ width: 1444, height: 953 });
    await page.goto("/");

    for (const [index, id] of pageIds.entries()) {
      await page.setViewportSize({ width: 1444, height: 953 });
      await page.getByRole("button", { name: appearance.labels[index], exact: true }).click();
      await expect(page.locator(".relay-shell")).toHaveAttribute("data-page", id);
      await expect(page.locator(".relay-page-header h1")).toBeVisible();
      await expectPageFits(page);
      const normalWidth = await page.locator(".relay-page").evaluate((element) => element.getBoundingClientRect().width);
      const cards = page.locator(id === "pool" ? ".pool-member-card" : ".account-card");
      const normalCardWidth = id === "pool" || id === "connections"
        ? await cards.first().evaluate((element) => element.getBoundingClientRect().width)
        : null;
      if (id === "pool") {
        await page.screenshot({ path: `output/playwright/page-width-${appearance.locale}-${id}-1444x953.png` });
      }

      await page.setViewportSize({ width: 2548, height: 1432 });
      await expectPageFits(page);
      const wideWidth = await page.locator(".relay-page").evaluate((element) => element.getBoundingClientRect().width);
      expect(wideWidth - normalWidth).toBeGreaterThan(1000);
      if (normalCardWidth !== null) {
        await expect(cards).toHaveCount(id === "pool" ? 6 : 5);
        const positions = await cards.evaluateAll((items) => items.map((item) => {
          const rect = item.getBoundingClientRect();
          return { top: rect.top, width: rect.width, fits: item.scrollWidth <= item.clientWidth };
        }));
        expect(positions.every((item) => item.fits)).toBe(true);
        expect(positions.every((item) => item.top === positions[0].top)).toBe(true);
        expect(positions.every((item) => item.width >= 280 && item.width <= 340)).toBe(true);
        expect(normalCardWidth).toBeGreaterThanOrEqual(280);
        expect(normalCardWidth).toBeLessThanOrEqual(340);
      }
      if (id === "settings" || id === "integrations") {
        const form = page.locator(id === "settings" ? ".settings-groups" : ".integrations-category");
        if (await form.count()) {
          const box = (await form.boundingBox())!;
          const header = (await page.locator(".relay-page-header").boundingBox())!;
          expect(box.width).toBeLessThanOrEqual(id === "settings" ? 1040 : 1120);
          expect(Math.abs(box.x - header.x)).toBeLessThanOrEqual(1);
        }
      }
      await page.mouse.move(1, 1);
      await page.screenshot({ path: `output/playwright/page-width-${appearance.locale}-${id}-2548x1432.png` });
    }
  });

  test(`workspace follows the sidebar and retains narrow layouts and viewport dialogs ${appearance.locale} ${appearance.theme}`, async ({ page }) => {
    await installTauriMock(page, { locale: appearance.locale, theme: appearance.theme, mode: "local", populated: true, accountCount: 5 });
    await page.setViewportSize({ width: 2548, height: 1432 });
    await page.goto("/");
    await page.getByRole("button", { name: appearance.labels[2], exact: true }).click();
    await expectPageFits(page);
    const expandedWidth = await page.locator(".relay-page").evaluate((element) => element.getBoundingClientRect().width);
    const columns = () => page.locator(".pool-member-list").evaluate((element) => getComputedStyle(element).gridTemplateColumns.split(" ").length);
    const expandedColumns = await columns();
    await page.getByRole("button", { name: appearance.collapse, exact: true }).click();
    await expectPageFits(page);
    const collapsedWidth = await page.locator(".relay-page").evaluate((element) => element.getBoundingClientRect().width);
    expect(Math.abs(collapsedWidth - expandedWidth - 176)).toBeLessThanOrEqual(1);
    expect(await columns()).toBeGreaterThan(expandedColumns);

    await page.locator(".pool-member-card .pool-member-actions button").last().click();
    const dialog = page.getByRole("dialog");
    await expect(dialog).toBeVisible();
    expect(await dialog.evaluate((element) => {
      const rect = element.getBoundingClientRect();
      return Math.abs(rect.left + rect.width / 2 - innerWidth / 2) <= 1
        && rect.top >= 36 && rect.bottom <= innerHeight;
    })).toBe(true);
    await page.keyboard.press("Escape");
    await expect(dialog).toHaveCount(0);

    for (const width of [840, 600, 390, 320]) {
      await page.setViewportSize({ width, height: 760 });
      for (const index of [1, 2]) {
        await page.getByRole("button", { name: appearance.labels[index], exact: true }).click();
        await expectPageFits(page);
        const list = page.locator(index === 1 ? ".account-list" : ".pool-member-list");
        expect(await list.evaluate((element) => getComputedStyle(element).gridTemplateColumns.split(" ").length)).toBe(width === 840 ? 2 : 1);
        const cards = list.locator(index === 1 ? ".account-card" : ".pool-member-card");
        await expect(cards).toHaveCount(index === 1 ? 5 : 6);
        expect(await cards.evaluateAll((items) => items.every((item) => item.scrollWidth <= item.clientWidth))).toBe(true);
        expect(await cards.evaluateAll((items) => items.every((item) => item.getBoundingClientRect().width <= 340))).toBe(true);
      }
      await page.screenshot({ path: `output/playwright/page-width-${appearance.locale}-pool-${width}x760.png` });
    }
  });

  test(`card grids use available tracks without stretching lone cards ${appearance.locale} ${appearance.theme}`, async ({ page }) => {
    await installTauriMock(page, { locale: appearance.locale, theme: appearance.theme, mode: "local", populated: true, accountCount: 5 });
    await page.setViewportSize({ width: 2394, height: 1243 });
    await page.goto("/");
    await page.getByRole("button", { name: appearance.labels[2], exact: true }).click();
    const members = page.locator(".pool-member-list");
    for (const width of [2394, 1920, 1440, 1160, 840, 600, 390, 320]) {
      await page.setViewportSize({ width, height: 900 });
      await expectPageFits(page);
      const layout = await members.evaluate((element) => {
        const columns = getComputedStyle(element).gridTemplateColumns.split(" ").map(Number.parseFloat);
        const grid = element.getBoundingClientRect();
        const cards = [...element.querySelectorAll(".pool-member-card")].map((card) => {
          const rect = card.getBoundingClientRect();
          return { top: rect.top, left: rect.left, right: rect.right, width: rect.width, fits: card.scrollWidth <= card.clientWidth };
        });
        return { columns, width: grid.width, left: grid.left, right: grid.right, cards };
      });
      expect(layout.columns.length).toBe(Math.max(1, Math.min(12, Math.floor((layout.width + 10) / 290))));
      expect(layout.cards.every((card) => card.fits && card.width <= 340
        && card.left >= layout.left && card.right <= layout.right + 1)).toBe(true);
      expect(Math.max(...layout.cards.map((card) => card.width)) - Math.min(...layout.cards.map((card) => card.width))).toBeLessThanOrEqual(1);
      if (width >= 1920) expect(layout.cards.slice(0, 5).every((card) => card.top === layout.cards[0].top)).toBe(true);
      await page.mouse.move(1, 1);
      await page.screenshot({ path: `output/playwright/workspace-pool-${appearance.locale}-${width}.png` });
    }
  });
}
