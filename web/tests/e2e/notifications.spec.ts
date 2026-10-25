import { expect, test } from "@playwright/test";

test.beforeEach(async ({ page }) => { await page.goto("/preview.html?page=usage"); });

test("notifications are a compact anchored panel, not a blocking request dialog", async ({ page }, testInfo) => {
  const bell = page.getByRole("button", { name: /^Уведомления:/ });
  await bell.click();
  const panel = page.getByRole("dialog", { name: "Уведомления", exact: true });
  await expect(panel).toHaveClass("notification-panel");
  await expect(page.locator("dialog.request-dialog")).toHaveCount(0);
  expect(await page.evaluate(() => document.body.style.overflow)).not.toBe("hidden");
  await expect(bell).toHaveAttribute("aria-expanded", "true");
  await expect(panel.getByRole("button", { name: "Закрыть уведомления", exact: true })).toBeFocused();
  await expect(panel.locator(".notification-panel-header")).toContainText("1 новое");
  await expect(panel.getByRole("article")).toHaveCount(1);
  await expect(panel).toContainText("Дождитесь завершения расчёта");
  expect(await panel.evaluate(element => {
    const bounds = element.getBoundingClientRect();
    return bounds.width <= 400 && bounds.height < 360 && bounds.left >= 12 && bounds.top >= 12
      && bounds.right <= innerWidth - 12 && bounds.bottom <= innerHeight - 12
      && element.scrollWidth <= element.clientWidth;
  })).toBe(true);
  await page.screenshot({ path: testInfo.outputPath("notification-panel-unread.png") });
  await panel.getByRole("button", { name: "Закрыть уведомления", exact: true }).click();
  await expect(panel).toHaveCount(0);
  await expect(bell).toBeFocused();
  await bell.click();
  await bell.click();
  await expect(panel).toHaveCount(0);
  await bell.click();
  await page.setViewportSize({ width: page.viewportSize()!.width, height: 180 });
  await expect.poll(() => panel.evaluate(element => {
    const bounds = element.getBoundingClientRect();
    return bounds.top >= 12 && bounds.bottom <= innerHeight - 12;
  })).toBe(true);
  await expect(panel.getByRole("button", { name: "Закрыть уведомления", exact: true })).toBeInViewport();
  await panel.getByRole("button", { name: "Закрыть уведомления", exact: true }).click();
  await expect(panel).toHaveCount(0);
});

test("read state persists; outside clicks and Escape dismiss; request details keep focus ownership", async ({ page }, testInfo) => {
  const bell = page.getByRole("button", { name: /^Уведомления:/ });
  const panel = page.getByRole("dialog", { name: "Уведомления", exact: true });
  await bell.click();
  await panel.getByRole("button", { name: "Отметить прочитанным", exact: true }).click();
  await expect(bell).toHaveAttribute("data-unread", "false");
  await expect(panel.locator(".notification-panel-header")).toContainText("Нет новых");
  await expect(panel.locator(".notification-unread-dot")).toHaveCount(0);
  await expect(panel.getByRole("button", { name: "Отметить прочитанным", exact: true })).toBeDisabled();
  await page.screenshot({ path: testInfo.outputPath("notification-panel-read.png") });
  const keyFilter = page.getByRole("combobox", { name: "API-ключ", exact: true });
  await keyFilter.click();
  await expect(panel).toHaveCount(0);
  await expect(keyFilter).toHaveAttribute("aria-expanded", "true");
  await expect(keyFilter).toBeFocused();
  await page.keyboard.press("Escape");
  await bell.click();
  await expect(panel.locator(".notification-panel-header")).toContainText("Нет новых");
  await page.keyboard.press("Escape");
  await expect(panel).toHaveCount(0);
  await expect(bell).toBeFocused();
  await bell.click();
  await panel.getByRole("button", { name: "Открыть запрос", exact: true }).click();
  await expect(panel).toHaveCount(0);
  const details = page.getByRole("dialog", { name: "Детали запроса", exact: true });
  await expect(details).toContainText("Ответ завершён, расчёт ожидается");
  await page.keyboard.press("Escape");
  await expect(details).toHaveCount(0);
  await expect(bell).toBeFocused();
});
