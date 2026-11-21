import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";

async function navigate(page: Page, name: string) {
  const sidebar = page.locator(".relay-sidebar");
  await expect(page.locator(".hosted-app")).toBeVisible();
  if (!await sidebar.isVisible()) await page.getByRole("button", { name: "Меню", exact: true }).click();
  await sidebar.getByRole("button", { name, exact: true }).click();
}

async function fits(page: Page) {
  await page.evaluate(() => document.fonts.ready);
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  expect(await page.locator(".relay-content").evaluate(element => element.scrollWidth <= element.clientWidth)).toBe(true);
  for (const element of await page.locator(".owner-notifications-page input, .owner-notifications-page textarea, .owner-notifications-page button").all()) {
    const bounds = await element.boundingBox();
    expect(bounds!.x).toBeGreaterThanOrEqual(0);
    expect(bounds!.x + bounds!.width).toBeLessThanOrEqual(page.viewportSize()!.width + 1);
  }
}

test("notification management is an admin-only destination with history and reload support", async ({ page }, testInfo) => {
  await page.goto("/admin/overview");
  await navigate(page, "Уведомления");
  await expect(page).toHaveURL(/\/admin\/notifications$/);
  await expect(page.getByRole("heading", { name: "Уведомления", exact: true })).toBeVisible();
  await expect(page.getByRole("heading", { name: "Черновиков уведомлений пока нет" })).toBeVisible();
  await fits(page);
  await page.screenshot({ path: testInfo.outputPath("notifications-empty.png"), fullPage: true });
  await page.reload();
  await expect(page.getByRole("heading", { name: "Уведомления", exact: true })).toBeVisible();
  await navigate(page, "Использование");
  await page.goBack();
  await expect(page.getByRole("heading", { name: "Уведомления", exact: true })).toBeVisible();
  await page.goto("/app/overview");
  await expect(page.locator(".sidebar-main-nav").getByRole("button", { name: "Уведомления", exact: true })).toHaveCount(0);
  await page.goto("/app/notifications");
  await expect(page.getByRole("heading", { name: "Страница не найдена", exact: true })).toBeVisible();
});

test("preview drafts can be created, edited and deleted without sending notifications", async ({ page }, testInfo) => {
  const mutations: string[] = [];
  page.on("request", request => { if (["POST", "PUT", "PATCH", "DELETE"].includes(request.method())) mutations.push(request.url()); });
  await page.goto("/admin/notifications");
  const surface = page.locator(".owner-notifications-page:visible");
  await surface.getByRole("button", { name: "Новое уведомление", exact: true }).click();
  await expect(surface.getByLabel("Заголовок", { exact: true })).toBeFocused();
  const save = surface.getByRole("button", { name: "Сохранить черновик", exact: true });
  await expect(save).toBeDisabled();
  await surface.getByLabel("Заголовок", { exact: true }).fill("Preview service notice");
  await surface.getByLabel("Сообщение", { exact: true }).fill("   ");
  await expect(save).toBeDisabled();
  await surface.getByLabel("Сообщение", { exact: true }).fill("A synthetic announcement for all users.\nNo delivery is performed.");
  await expect(surface.getByRole("button", { name: "Отправить", exact: true })).toBeDisabled();
  await expect(surface.getByRole("note")).toContainText("не отправляются");
  await fits(page);
  await page.screenshot({ path: testInfo.outputPath("notifications-editor.png"), fullPage: true });
  await save.click();
  const row = surface.getByRole("article", { name: "Preview service notice", exact: true });
  await expect(row).toContainText("Черновик");
  await expect(surface.getByRole("button", { name: "Новое уведомление", exact: true })).toBeFocused();
  await navigate(page, "Использование");
  await navigate(page, "Уведомления");
  await expect(row).toBeVisible();
  await row.getByRole("button", { name: "Изменить", exact: true }).click();
  await surface.getByLabel("Сообщение", { exact: true }).fill("Unsaved change");
  await surface.getByRole("button", { name: "Отмена", exact: true }).click();
  await expect(row).not.toContainText("Unsaved change");
  await expect(row.getByRole("button", { name: "Изменить", exact: true })).toBeFocused();
  await row.getByRole("button", { name: "Изменить", exact: true }).click();
  await surface.getByLabel("Сообщение", { exact: true }).fill("Updated announcement");
  await save.click();
  await expect(row).toContainText("Updated announcement");
  await fits(page);
  await page.screenshot({ path: testInfo.outputPath("notifications-draft.png"), fullPage: true });
  await row.getByRole("button", { name: "Удалить", exact: true }).click();
  await expect(row).toContainText("Удалить черновик?");
  await row.getByRole("button", { name: "Отмена", exact: true }).click();
  await expect(row).toBeVisible();
  await row.getByRole("button", { name: "Удалить", exact: true }).click();
  await row.getByRole("button", { name: "Удалить", exact: true }).click();
  await expect(row).toHaveCount(0);
  expect(mutations).toEqual([]);
});

for (const [locale, heading, create, title, message, save] of [
  ["en", "Notifications", "New notification", "Title", "Message", "Save draft"],
  ["zh-CN", "通知", "新建通知", "标题", "消息", "保存草稿"],
] as const) {
  test(`notification controls and long content fit in ${locale}`, async ({ page }, testInfo) => {
    await page.addInitScript(value => localStorage.setItem("relay-hosted-locale", value), locale);
    await page.goto("/admin/notifications");
    const surface = page.locator(".owner-notifications-page:visible");
    await expect(surface.getByRole("heading", { name: heading, exact: true })).toBeVisible();
    await surface.getByRole("button", { name: create, exact: true }).click();
    await surface.getByLabel(title, { exact: true }).fill("A".repeat(120));
    await surface.getByLabel(message, { exact: true }).fill("Message".repeat(100));
    await fits(page);
    await surface.getByRole("button", { name: save, exact: true }).click();
    await expect(surface.getByRole("article")).toHaveCount(1);
    await fits(page);
    await page.screenshot({ path: testInfo.outputPath(`notifications-${locale}.png`), fullPage: true });
    await page.reload();
    await expect(surface.getByRole("article")).toHaveCount(0);
  });
}
