import { expect, test } from "@playwright/test";

test("model ID copy retains its honest manual fallback outside request details", async ({ page }) => {
  await page.addInitScript(() => {
    Object.defineProperty(navigator, "clipboard", { value: { writeText: () => Promise.reject(new Error("blocked")) }, configurable: true });
  });
  await page.goto("/preview.html?page=models");
  await page.getByRole("button", { name: "Подробнее о GPT · демо", exact: true }).click();
  const dialog = page.getByRole("dialog");
  await dialog.getByRole("button", { name: "Скопировать ID модели", exact: true }).click();
  const manual = dialog.getByRole("textbox", { name: "Скопировать ID модели", exact: true });
  await expect(manual).toHaveValue("demo-gpt");
  await expect(dialog).toContainText("Копирование недоступно. Выделите текст вручную.");
  await expect(manual).toHaveCSS("border-top-color", await page.locator(".topbar .notification-button").evaluate(element => getComputedStyle(element).borderTopColor));
});
