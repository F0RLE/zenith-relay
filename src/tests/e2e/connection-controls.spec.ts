import { expect, test, type Locator } from "../bun-playwright";
import { installTauriMock } from "./tauri-mock";

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
      if (width === 1160) expect((await panel.boundingBox())!.height).toBeLessThanOrEqual(100);
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
      await page.mouse.move(0, 0);
      await panel.screenshot({ path: testInfo.outputPath("controls.png"), animations: "disabled" });
      await page.screenshot({ path: testInfo.outputPath("connections.png"), animations: "disabled" });
    });
  }
}

for (const width of [1160, 390]) {
  for (const credits of ["missing", "zero", "finite", "unlimited"] as const) {
    test(`connection and pool summaries keep dividers with ${credits} credits at ${width}px`, async ({ page }) => {
      await page.setViewportSize({ width, height: 844 });
      await installTauriMock(page, {
        mode: "local", locale: "en", populated: true, accountCount: 2,
        ...(credits === "finite" ? { providerCredits: 2.5 } : credits === "zero" ? { providerCredits: 0 } : {}),
        providerCreditsUnlimited: credits === "unlimited",
      });
      await page.goto("/");
      for (const tab of ["Connections", "Pool"]) {
        await page.getByRole("button", { name: tab, exact: true }).click();
        const summary = page.locator(".connection-status-summary");
        const hasCredits = credits === "finite" || credits === "unlimited";
        await expect(summary).toHaveAttribute("data-has-provider-credits", String(hasCredits));
        await expect(summary.locator(":scope > div")).toHaveCount(hasCredits ? 5 : 4);
        const total = summary.locator('[data-summary="provider-credits"]');
        if (hasCredits) {
          await expect(total).toContainText("Total credits");
          await expect(total.locator("strong")).toHaveText(credits === "finite" ? "5" : "\u221e");
          if (width < 640) await expect(total).toHaveCSS("border-top-width", "1px");
        } else {
          await expect(total).toHaveCount(0);
        }
        const borders = await summary.locator(":scope > div").evaluateAll((items) => items.map((item) => getComputedStyle(item).borderLeftWidth));
        expect(borders).toEqual(width < 640
          ? hasCredits ? ["0px", "1px", "0px", "1px", "0px"] : ["0px", "1px", "0px", "1px"]
          : hasCredits ? ["0px", "1px", "1px", "1px", "1px"] : ["0px", "1px", "1px", "1px"]);
      }
    });
  }
}
