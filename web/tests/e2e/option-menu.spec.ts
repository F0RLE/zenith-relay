import { expect, test } from "@playwright/test";

test("model menu uses Relay colors, no native popup or mouse focus ring, and fits the viewport", async ({ page }, testInfo) => {
  await page.goto("/preview.html?page=overview");
  await expect(page.locator("select")).toHaveCount(0);
  const trigger = page.getByRole("combobox", { name: "Модель статистики", exact: true });
  await trigger.click();
  await expect(trigger).toHaveAttribute("aria-expanded", "true");
  expect(await trigger.evaluate(element => {
    const style = getComputedStyle(element);
    return { outline: style.outlineStyle, shadow: style.boxShadow };
  })).toEqual({ outline: "none", shadow: "none" });
  const list = page.getByRole("listbox", { name: "Модель статистики", exact: true });
  await expect(list.getByRole("option", { name: "Все модели", exact: true })).toHaveAttribute("aria-selected", "true");
  expect(await list.evaluate(element => {
    const rect = element.getBoundingClientRect();
    return rect.left >= 0 && rect.top >= 0 && rect.right <= innerWidth && rect.bottom <= innerHeight
      && element.scrollWidth <= element.clientWidth
      && getComputedStyle(element).colorScheme === getComputedStyle(document.querySelector(".relay-app")!).colorScheme;
  })).toBe(true);
  await page.screenshot({ path: testInfo.outputPath("model-menu.png") });
  await list.getByRole("option", { name: "GPT · демо", exact: true }).click();
  await expect(list).toHaveCount(0);
  await expect(trigger).toHaveText("GPT · демо");
  await expect(trigger).toBeFocused();
  await trigger.click();
  await expect(list.getByRole("option", { name: "GPT · демо", exact: true })).toHaveAttribute("aria-selected", "true");
  await page.getByRole("heading", { name: "Использование во времени", exact: true }).click();
  await expect(list).toHaveCount(0);
});

test("keyboard keeps a subtle focus indicator; navigation, cancel, typeahead and Tab work", async ({ page }) => {
  await page.goto("/preview.html?page=overview");
  const trigger = page.getByRole("combobox", { name: "Модель статистики", exact: true });
  await trigger.focus();
  await trigger.press("ArrowDown");
  expect(await trigger.evaluate(element => {
    const style = getComputedStyle(element);
    return style.outlineStyle === "none" && style.boxShadow !== "none";
  })).toBe(true);
  const options = page.getByRole("listbox").getByRole("option");
  await trigger.press("End");
  await expect(options.last()).toHaveAttribute("data-active", "true");
  await expect(trigger).toHaveAttribute("aria-activedescendant", await options.last().getAttribute("id") as string);
  await trigger.press("Escape");
  await expect(page.getByRole("listbox")).toHaveCount(0);
  await expect(trigger).toBeFocused();
  await expect(trigger).toHaveText("Все модели");
  await trigger.press("ArrowDown");
  await trigger.press("ArrowDown");
  await trigger.press("Enter");
  await expect(trigger).toHaveText("GPT · демо");
  await trigger.press("c");
  await trigger.press("Enter");
  await expect(trigger).toHaveText("Claude · демо");
  await trigger.press(" ");
  await trigger.press("Home");
  await trigger.press(" ");
  await expect(trigger).toHaveText("Все модели");
  await trigger.press("ArrowUp");
  await trigger.press("Tab");
  await expect(page.getByRole("listbox")).toHaveCount(0);
  await expect(trigger).not.toBeFocused();
});

test("Usage menus keep filters, disabled error status and pagination working", async ({ page }) => {
  await page.goto("/preview.html?page=usage");
  await expect(page.locator("select")).toHaveCount(0);
  await page.getByRole("button", { name: "Следующая страница", exact: true }).click();
  await expect(page.locator(".pagination")).toContainText("9–16 из 28");
  const size = page.getByRole("combobox", { name: "Строк", exact: true });
  await size.click();
  const list = page.getByRole("listbox");
  expect(await list.evaluate(element => {
    const rect = element.getBoundingClientRect();
    return rect.top >= 0 && rect.bottom <= innerHeight;
  })).toBe(true);
  await page.getByRole("option", { name: "16", exact: true }).click();
  await expect(page.locator(".pagination")).toContainText("1–16 из 28");
  await expect(page.locator(".request-table tbody tr")).toHaveCount(16);
  await page.getByRole("button", { name: "Дополнительные фильтры", exact: true }).click();
  for (const [label, option] of [["Протокол", "Responses"], ["Транспорт", "WebSocket"], ["Расчёт", "Все состояния"]]) {
    await page.getByRole("combobox", { name: label, exact: true }).click();
    await page.getByRole("option", { name: option, exact: true }).click();
  }
  await expect(page.locator('.request-table tbody td[data-column="protocol"]')).toHaveText(Array(10).fill("Responses"));
  await page.getByRole("button", { name: "Сбросить фильтры", exact: true }).click();
  await page.getByRole("combobox", { name: "Выполнение", exact: true }).click();
  await page.getByRole("option", { name: "Завершён", exact: true }).click();
  await page.getByRole("group", { name: "Представление использования" }).getByRole("button", { name: "Ошибки", exact: true }).click();
  await expect(page.getByRole("combobox", { name: "Выполнение", exact: true })).toBeDisabled();
  await expect(page.getByRole("combobox", { name: "Выполнение", exact: true })).toHaveText("Ошибка");
  await page.getByRole("group", { name: "Представление использования" }).getByRole("button", { name: "Запросы", exact: true }).click();
  await expect(page.getByRole("combobox", { name: "Выполнение", exact: true })).toBeEnabled();
  await expect(page.getByRole("combobox", { name: "Выполнение", exact: true })).toHaveText("Завершён");
});

test("catalog protocol menu filters models and opening another menu dismisses the first", async ({ page }) => {
  await page.goto("/preview.html?page=models");
  await expect(page.locator("select")).toHaveCount(0);
  await page.getByRole("combobox", { name: "Протокол модели", exact: true }).click();
  await page.getByRole("option", { name: "Messages", exact: true }).click();
  await expect(page.locator(".catalog-table tbody tr")).toHaveCount(1);
  await expect(page.locator(".catalog-table tbody")).toContainText("Claude · демо");
  await page.goto("/preview.html?page=usage");
  await page.getByRole("combobox", { name: "Модель", exact: true }).click();
  await page.getByRole("combobox", { name: "API-ключ", exact: true }).click();
  await expect(page.getByRole("listbox")).toHaveCount(1);
  await expect(page.getByRole("listbox", { name: "API-ключ", exact: true })).toBeVisible();
  await page.getByRole("option", { name: "Бот", exact: true }).click();
  await expect(page.locator('.request-table tbody td[data-column="key"]')).toContainText(Array(8).fill("Бот"));
});
