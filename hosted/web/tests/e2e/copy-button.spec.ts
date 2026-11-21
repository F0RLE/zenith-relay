import { expect, test } from "@playwright/test";

test("one-time key copy retains its honest manual fallback outside request details", async ({ page }) => {
  await page.addInitScript(() => {
    Object.defineProperty(navigator, "clipboard", { value: { writeText: () => Promise.reject(new Error("blocked")) }, configurable: true });
  });
  await page.goto("/app/keys");
  await page.getByRole("button", { name: "Создать ключ", exact: true }).click();
  const form = page.getByRole("form", { name: "Создать ключ", exact: true });
  await form.getByLabel("Название", { exact: true }).fill("Проверка копирования");
  await form.getByRole("button", { name: "Создать ключ", exact: true }).click();
  const result = page.getByRole("region", { name: "Ключ создан", exact: true });
  const secret = await result.locator("code").textContent();
  await result.getByRole("button", { name: "Скопировать ключ", exact: true }).click();
  const manual = result.getByRole("textbox", { name: "Скопировать ключ", exact: true });
  await expect(manual).toHaveValue(secret!);
  await expect(result).toContainText("Копирование недоступно. Выделите текст вручную.");
  await expect(manual).toHaveCSS("border-top-color", await page.locator(".table-shell:visible").evaluate(element => getComputedStyle(element).borderTopColor));
});
