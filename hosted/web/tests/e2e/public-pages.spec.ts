import { expect, test } from "@playwright/test";

test("public guides work on direct entry without a session, including narrow screens", async ({ page }, info) => {
  await page.goto("/docs");
  await expect(page.getByRole("heading", { level: 1, name: "Подключение к API" })).toBeVisible();
  await expect(page.locator("main ol > li")).toHaveCount(4);
  await expect(page.locator("#main-content a[aria-current='page']")).toHaveAttribute("href", "/docs");
  await expect(page.locator("main a[href^='/app'], main a[href^='/admin']")).toHaveCount(0);
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  await page.screenshot({ path: info.outputPath("api-guide.png"), animations: "disabled" });
  await page.getByRole("link", { name: "Документы и поддержка" }).click();
  await expect(page).toHaveURL(/\/legal$/);
  await expect(page.getByText("Владелец сервиса пока не опубликовал документы или контакты поддержки.")).toBeVisible();
  await expect(page.locator("main a[href^='https:']")).toHaveCount(0);
  await expect(page.locator(".landing-brand")).toHaveAttribute("href", "/");
});
