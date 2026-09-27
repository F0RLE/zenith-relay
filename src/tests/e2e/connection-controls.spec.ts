import { expect, test, type Locator } from "../bun-playwright";
import { installTauriMock } from "./tauri-mock";

for (const width of [1440, 1160, 840, 390, 360]) {
  test(`workspace headers keep navigation and primary actions accessible at ${width}px`, async ({ page }) => {
    await page.setViewportSize({ width, height: 844 });
    await installTauriMock(page, { mode: "local", locale: "ru", populated: true });
    await page.goto("/");
    for (const name of ["Подключения", "Пул"]) {
      await page.getByRole("button", { name, exact: true }).click();
      const header = page.locator(".relay-workspace-header");
      await expect(header.getByRole("heading", { name, exact: true })).toBeVisible();
      expect(await header.evaluate((element) => {
        const bounds = element.getBoundingClientRect();
        const targets = Array.from(element.querySelectorAll<HTMLElement>('h1, [role="tab"], .relay-page-actions > .relay-button, .pool-header-actions > .relay-button, summary'));
        return targets.every((item) => {
          const rect = item.getBoundingClientRect();
          return rect.left >= bounds.left && rect.right <= bounds.right + 1
            && rect.top >= bounds.top && rect.bottom <= bounds.bottom + 1
            && item.scrollWidth <= item.clientWidth;
        }) && targets.every((item, index) => {
          const a = item.getBoundingClientRect();
          return targets.slice(index + 1).every((next) => {
            const b = next.getBoundingClientRect();
            return a.right <= b.left + 1 || b.right <= a.left + 1 || a.bottom <= b.top + 1 || b.bottom <= a.top + 1;
          });
        });
      })).toBe(true);
      const firstTab = header.getByRole("tab").first();
      await firstTab.focus();
      await firstTab.press("ArrowRight");
      await expect(header.getByRole("tab").nth(1)).toBeFocused();
      await expect(header.getByRole("tab").nth(1)).toHaveAttribute("aria-selected", "true");
      if (name === "Подключения") {
        const toolbar = page.locator(".connections-toolbar");
        const list = page.locator(".connection-list-wrap");
        await expect(list).toBeVisible();
        const [toolbarBox, listBox] = await Promise.all([toolbar.boundingBox(), list.boundingBox()]);
        expect(Math.abs(toolbarBox!.x - listBox!.x)).toBeLessThanOrEqual(1);
        expect(Math.abs(toolbarBox!.width - listBox!.width)).toBeLessThanOrEqual(1);
      }
      await header.getByRole("tab").nth(1).press("Home");
      await expect(firstTab).toBeFocused();
      await expect(firstTab).toHaveAttribute("aria-selected", "true");
    }
  });
}

async function expectControlsFit(panel: Locator) {
  expect(await panel.evaluate((element) => {
    const bounds = element.getBoundingClientRect();
    const selectors = ".account-command-context, .account-filter-stack, .account-command-actions, .connection-status-summary > div";
    return bounds.left >= 0 && bounds.right <= innerWidth
      && Array.from(element.querySelectorAll<HTMLElement>(selectors)).every((item) => {
        const rect = item.getBoundingClientRect();
        return rect.left >= bounds.left - 1 && rect.right <= bounds.right + 1
          && item.scrollWidth <= item.clientWidth + 1;
      });
  })).toBe(true);
  expect(await panel.locator(".account-command-bar, .account-filter-stack, .connection-status-summary").evaluateAll((rows) => rows.every((row) => {
    const children = Array.from(row.children).map((child) => child.getBoundingClientRect());
    return children.every((a, index) => children.slice(index + 1).every((b) =>
      a.right <= b.left + 1 || b.right <= a.left + 1 || a.bottom <= b.top + 1 || b.bottom <= a.top + 1,
    ));
  }))).toBe(true);
}

for (const theme of ["light", "dark"] as const) {
  for (const width of [1160, 840, 390, 360]) {
    test(`connection controls fit filters and selection in ${theme} at ${width}px`, async ({ page }, testInfo) => {
      await page.setViewportSize({ width, height: 844 });
      await installTauriMock(page, { mode: "local", locale: "ru", theme, populated: true, accountCount: 3, providerCredits: 125.5 });
      await page.goto("/");
      await page.getByRole("button", { name: "Подключения", exact: true }).click();
      const panel = page.locator(".connections-account-controls");
      await expect(panel.locator('[data-summary="provider-credits"] strong')).toHaveText("376,5");
      await expectControlsFit(panel);
      if (width === 1160) expect((await panel.boundingBox())!.height).toBeLessThanOrEqual(120);
      const summaryBox = (await panel.locator(".connection-status-summary").boundingBox())!;
      const toolbarBox = (await panel.locator(".account-command-bar").boundingBox())!;
      expect(summaryBox.y + summaryBox.height).toBeLessThan(toolbarBox.y);
      if (width <= 390) {
        const search = (await panel.getByRole("textbox", { name: "Поиск", exact: true }).boundingBox())!;
        expect(search.width).toBeGreaterThanOrEqual(200);
      }
      await panel.getByRole("button", { name: "По подписке", exact: true }).hover();
      await expect(page.getByRole("tooltip", { name: "По подписке", exact: true })).toBeVisible();
      await panel.getByRole("button", { name: "По подписке", exact: true }).click();
      await expect(panel.getByRole("button", { name: "По подписке", exact: true })).toHaveAttribute("aria-pressed", "true");
      await panel.getByRole("button", { name: /^Фильтр по подписке:/ }).click();
      await page.getByRole("option").filter({ hasText: "Plus" }).click();
      await expect(page.locator(".account-card")).toHaveCount(1);
      await panel.getByRole("checkbox").check();
      await expect(panel.locator(".account-command-context > span")).toHaveText("Выбрано: 1");
      await expect(panel.getByRole("button", { name: "Экспортировать выбранные (1)" })).toBeVisible();
      await expectControlsFit(panel);
      await panel.screenshot({ path: testInfo.outputPath("selection.png") });
      await panel.getByRole("button", { name: "Снять выделение", exact: true }).click();
      await panel.getByRole("button", { name: /^Фильтр по подписке:/ }).click();
      await page.locator('[role="option"][data-value="all"]').click();
      await panel.getByRole("textbox", { name: "Поиск", exact: true }).fill("Personal Plus");
      await expect(page.locator(".account-card")).toHaveCount(1);
      await panel.getByRole("textbox", { name: "Поиск", exact: true }).fill("");
      await expect(page.locator(".account-card")).toHaveCount(3);
      await panel.getByRole("button", { name: "По подписке", exact: true }).click();
      await expect(panel.getByRole("button", { name: "По подписке", exact: true })).toHaveAttribute("aria-pressed", "false");
      await page.mouse.click(1, 1);
      await panel.screenshot({ path: testInfo.outputPath("controls.png"), animations: "disabled" });
      await page.screenshot({ path: testInfo.outputPath("connections.png"), animations: "disabled" });
    });
  }
}

for (const width of [1160, 390]) {
  for (const credits of ["missing", "zero", "finite", "unlimited"] as const) {
    test(`connection and pool summaries share their layout with ${credits} credits at ${width}px`, async ({ page }) => {
      await page.setViewportSize({ width, height: 844 });
      await installTauriMock(page, {
        mode: "local", locale: "en", populated: true, accountCount: 2,
        ...(credits === "finite" ? { providerCredits: 2.5 } : credits === "zero" ? { providerCredits: 0 } : {}),
        providerCreditsUnlimited: credits === "unlimited",
      });
      await page.goto("/");
      for (const tab of ["Connections", "Pool"]) {
        await page.getByRole("button", { name: tab, exact: true }).click();
        const summary = page.locator(".relay-status-summary");
        const hasCredits = credits === "finite" || credits === "unlimited";
        await expect(summary).toHaveAttribute("data-has-provider-credits", String(hasCredits));
        await expect(summary.locator(":scope > div")).toHaveCount(hasCredits ? 5 : 4);
        const total = summary.locator('[data-summary="provider-credits"]');
        if (hasCredits) {
          await expect(total).toContainText("Total credits");
          await expect(total.locator("strong")).toHaveText(credits === "finite" ? "5" : "\u221e");
        } else {
          await expect(total).toHaveCount(0);
        }
        expect(await summary.locator(':scope > div:not([data-summary="provider-credits"])').evaluateAll((items) => items.every((item) => {
          const value = item.querySelector("strong")!.getBoundingClientRect();
          const label = item.querySelector("span")!.getBoundingClientRect();
          return label.left >= value.right && getComputedStyle(item).borderLeftWidth === "0px";
        }))).toBe(true);
        await expect(summary).toHaveCSS("border-bottom-width", "1px");
        if (hasCredits && width < 640) {
          const [summaryBox, creditsBox] = await Promise.all([summary.boundingBox(), total.boundingBox()]);
          expect(creditsBox!.width).toEqual(summaryBox!.width);
        }
      }
    });
  }
}
