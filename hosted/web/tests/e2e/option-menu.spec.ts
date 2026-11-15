import { expect, test } from "@playwright/test";

test("model menu uses launcher pine and cream, no native popup or mouse focus ring, and fits the viewport", async ({ page }, testInfo) => {
  await page.goto("/preview.html?page=overview");
  await expect(page.locator(".relay-page select")).toHaveCount(0);
  const trigger = page.getByRole("combobox", { name: "Модель статистики", exact: true });
  await expect(trigger).toHaveCSS("box-shadow", "none");
  const restingShadow = await trigger.evaluate(element => getComputedStyle(element).boxShadow);
  const restingBorder = await trigger.evaluate(element => getComputedStyle(element).borderTopColor);
  await trigger.hover();
  await expect(trigger).toHaveCSS("border-top-color", restingBorder);
  await trigger.click();
  await expect(trigger).toHaveAttribute("aria-expanded", "true");
  expect(await trigger.evaluate(element => {
    const style = getComputedStyle(element);
    return { outline: style.outlineStyle, shadow: style.boxShadow };
  })).toEqual({ outline: "none", shadow: restingShadow });
  const list = page.getByRole("listbox", { name: "Модель статистики", exact: true });
  const selected = list.getByRole("option", { name: "Все модели", exact: true });
  await expect(selected).toHaveAttribute("aria-selected", "true");
  const pine = await page.locator(".relay-sidebar").evaluate(element => getComputedStyle(element).backgroundColor);
  await expect(list).toHaveCSS("background-color", pine);
  await expect(trigger).toHaveCSS("background-color", pine);
  await expect(list).toHaveCSS("border-top-width", "0px");
  await expect(selected).toHaveCSS("background-color", "rgb(240, 242, 220)");
  await expect(selected).toHaveCSS("color", "rgb(22, 76, 60)");
  await expect(list).toHaveCSS("border-radius", "12px");
  if (page.viewportSize()!.width <= 639) {
    expect(await selected.evaluate(element => element.getBoundingClientRect().height)).toBeGreaterThanOrEqual(44);
  }
  expect(await list.evaluate(element => {
    const rect = element.getBoundingClientRect();
    return rect.left >= 0 && rect.top >= 0 && rect.right <= innerWidth && rect.bottom <= innerHeight
      && element.scrollWidth <= element.clientWidth
      && getComputedStyle(element).colorScheme === getComputedStyle(document.querySelector(".relay-app")!).colorScheme;
  })).toBe(true);
  await page.screenshot({ path: testInfo.outputPath("model-menu.png") });
  await list.getByRole("option", { name: "GPT", exact: true }).click();
  await expect(list).toHaveCount(0);
  await expect(trigger).toHaveText("GPT");
  await expect(trigger).toBeFocused();
  await trigger.click();
  await expect(list.getByRole("option", { name: "GPT", exact: true })).toHaveAttribute("aria-selected", "true");
  await page.getByRole("heading", { name: "Обзор", exact: true }).click();
  await expect(list).toHaveCount(0);
});

test("keyboard navigation, cancel, typeahead and Tab use fill feedback without focus outlines", async ({ page }) => {
  await page.goto("/preview.html?page=overview");
  const trigger = page.getByRole("combobox", { name: "Модель статистики", exact: true });
  await trigger.focus();
  await trigger.press("ArrowDown");
  expect(await trigger.evaluate(element => {
    const style = getComputedStyle(element);
    return element === document.activeElement && style.outlineStyle === "none" && style.boxShadow === "none";
  })).toBe(true);
  const options = page.getByRole("listbox").getByRole("option");
  await trigger.press("End");
  await expect(options.last()).toHaveAttribute("data-active", "true");
  await expect(options.first()).toHaveCSS("background-color", "rgb(240, 242, 220)");
  await expect(options.last()).toHaveCSS("outline-style", "none");
  await expect(options.last()).not.toHaveCSS("background-color", "rgba(0, 0, 0, 0)");
  await expect(trigger).toHaveAttribute("aria-activedescendant", await options.last().getAttribute("id") as string);
  await trigger.press("Escape");
  await expect(page.getByRole("listbox")).toHaveCount(0);
  await expect(trigger).toBeFocused();
  await expect(trigger).toHaveText("Все модели");
  await expect(trigger).toHaveCSS("outline-style", "none");
  const closedFill = await page.locator(".overview-controls .relay-tabs button.active").evaluate(element => getComputedStyle(element).backgroundColor);
  await expect(trigger).toHaveCSS("background-color", closedFill);
  await trigger.press("ArrowDown");
  await trigger.press("ArrowDown");
  await trigger.press("Enter");
  await expect(trigger).toHaveText("GPT");
  await trigger.press("c");
  await trigger.press("Enter");
  await expect(trigger).toHaveText("Claude");
  await trigger.press(" ");
  await trigger.press("Home");
  await trigger.press(" ");
  await expect(trigger).toHaveText("Все модели");
  await trigger.press("ArrowUp");
  await trigger.press("Tab");
  await expect(page.getByRole("listbox")).toHaveCount(0);
  await expect(trigger).not.toBeFocused();
});

test("usage period never gains a ring when switching from keyboard to pointer", async ({ page }) => {
  await page.goto("/preview.html?page=usage");
  const trigger = page.getByRole("combobox", { name: "Период, UTC", exact: true });
  const border = await trigger.evaluate(element => getComputedStyle(element).borderTopColor);
  await trigger.hover();
  await expect(trigger).toHaveCSS("border-top-color", border);
  await trigger.focus();
  await trigger.press("ArrowDown");
  await expect(trigger).toHaveCSS("outline-style", "none");
  await trigger.press("Escape");
  await expect(trigger).toHaveCSS("border-top-color", border);
  await trigger.click();
  await expect(trigger).toHaveCSS("outline-style", "none");
  await page.getByRole("option", { name: "Последние 7 дней", exact: true }).hover();
  await expect(page.getByRole("option", { name: "Последние 7 дней", exact: true })).toHaveCSS("outline-style", "none");
  await page.getByRole("option", { name: "Последние 7 дней", exact: true }).click();
  await expect(trigger).toHaveText("7 дней");
  await expect(trigger).toHaveCSS("outline-style", "none");
  await expect(trigger).toHaveCSS("box-shadow", "none");
  await expect(trigger).toHaveCSS("border-top-color", border);
});

test("Usage menus keep filters, disabled error status and pagination working", async ({ page }) => {
  await page.goto("/preview.html?page=usage");
  await expect(page.locator("select")).toHaveCount(0);
  await page.getByRole("button", { name: "Следующая страница", exact: true }).click();
  await expect(page.getByRole("navigation", { name: "Страницы использования", exact: true })).toContainText("9–16 из 28");
  const size = page.getByRole("combobox", { name: "Строк", exact: true });
  await size.click();
  const list = page.getByRole("listbox");
  expect(await list.evaluate(element => {
    const rect = element.getBoundingClientRect();
    return rect.top >= 0 && rect.bottom <= innerHeight;
  })).toBe(true);
  await page.getByRole("option", { name: "16", exact: true }).click();
  await expect(page.getByRole("navigation", { name: "Страницы использования", exact: true })).toContainText("1–16 из 28");
  await expect(page.locator(".request-table tbody tr")).toHaveCount(16);
  const more = page.getByRole("button", { name: "Дополнительные фильтры", exact: true });
  if (await more.isVisible()) await more.click();
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

test("usage menu switching works with desktop pointer and mobile keyboard", async ({ page }) => {
  await page.goto("/preview.html?page=usage");
  const model = page.getByRole("combobox", { name: "Модель", exact: true });
  const key = page.getByRole("combobox", { name: "API-ключ", exact: true });
  await expect(model).toHaveJSProperty("tagName", "BUTTON");
  await expect(key).toHaveJSProperty("tagName", "BUTTON");
  await model.click();
  await expect(page.getByRole("listbox", { name: "Модель", exact: true })).toBeVisible();
  if (page.viewportSize()!.width < 640) {
    // The mobile fields stack: the open model list covers the key trigger.
    // Tab switches controls through the public keyboard path, without forced clicks.
    await page.keyboard.press("Tab");
    await expect(key).toBeFocused();
    await page.keyboard.press("Enter");
  } else await key.click();
  await expect(model).toHaveAttribute("aria-expanded", "false");
  await expect(key).toHaveAttribute("aria-expanded", "true");
  await expect(page.getByRole("listbox")).toHaveCount(1);
  await expect(page.getByRole("listbox", { name: "API-ключ", exact: true })).toBeVisible();
  await page.getByRole("option", { name: "Bot", exact: true }).click();
  await expect(page.locator('.request-table tbody td[data-column="key"]')).toContainText(Array(8).fill("Bot"));
});

test("workspace filters share launcher styling and honor reduced motion and high contrast", async ({ page }) => {
  await page.emulateMedia({ reducedMotion: "reduce" });
  await page.goto("/preview.html?page=overview");
  const trigger = page.getByRole("combobox", { name: "Модель статистики", exact: true });
  await trigger.click();
  const list = page.getByRole("listbox");
  const selected = list.locator('[aria-selected="true"]');
  await expect(selected).toHaveCSS("background-color", "rgb(240, 242, 220)");
  await expect(list).toHaveCSS("animation-name", "none");
  await expect(trigger).toHaveCSS("transition-duration", "0s");
  await trigger.press("Escape");
  await page.emulateMedia({ forcedColors: "active" });
  await trigger.focus();
  await trigger.press("ArrowDown");
  await expect(list).toHaveCSS("border-top-width", "1px");
  await expect(list).toHaveCSS("box-shadow", "none");
  await expect(selected).toHaveCSS("forced-color-adjust", "none");
  await trigger.press("Escape");
  await expect(trigger).toBeFocused();
});
