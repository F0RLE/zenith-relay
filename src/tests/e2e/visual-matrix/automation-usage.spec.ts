import { expect, test } from "../../bun-playwright";
import { installTauriMock } from "../tauri-mock";
import { viewports, expectTopLevelEmptyCentered } from "./fixtures";

test("remote connection choices use clear switches in a centered dialog", async ({ page }) => {
  await installTauriMock(page, { locale: "ru", mode: "remote", theme: "dark", populated: true, remoteConnected: false });
  await page.setViewportSize({ width: 840, height: 560 });
  await page.goto("/");
  await page.getByRole("button", { name: "Подключения", exact: true }).click();
  await page.getByRole("button", { name: "Подключить существующий сервер" }).click();
  await page.getByLabel("Адрес сервера").fill("http://127.0.0.1:14999");

  const dialog = page.getByRole("dialog", { name: "Подключить существующий сервер" });
  await expect(dialog.locator(".setting-toggle")).toHaveCount(2);
  await expect(dialog.getByLabel("Разрешить HTTP без шифрования")).toBeVisible();
  await expect(dialog.getByLabel("Доверять новой идентичности")).toBeVisible();
  await expect(dialog.getByText("Токен и трафик передаются открыто. Используйте только в доверенной локальной сети.", { exact: true })).toBeVisible();
  const [backdropBox, dialogBox] = await Promise.all([page.locator(".relay-modal-backdrop").boundingBox(), dialog.boundingBox()]);
  expect(backdropBox).not.toBeNull();
  expect(dialogBox).not.toBeNull();
  expect(Math.abs(dialogBox!.y + dialogBox!.height / 2 - (backdropBox!.y + backdropBox!.height / 2))).toBeLessThanOrEqual(2);
  expect(await dialog.locator(".setting-toggle").evaluateAll((rows) => rows.every((row) => row.scrollWidth <= row.clientWidth))).toBe(true);
  await page.screenshot({ path: "output/playwright/remote-connect-options-ru-dark-840x560.png" });
});

test("unconfigured API empty state uses the available page center", async ({ page }) => {
  await installTauriMock(page, { locale: "ru", mode: "remote", theme: "dark", populated: true, remoteConnected: false });
  await page.setViewportSize({ width: 1160, height: 760 });
  await page.goto("/");
  await page.getByRole("button", { name: "API", exact: true }).click();
  await expect(page.getByText("API не настроен", { exact: true })).toBeVisible();
  await expectTopLevelEmptyCentered(page);
  await page.screenshot({ path: "output/playwright/gateway-empty-centered-ru-dark-1160x760.png" });
});

test("unsupported usage empty state uses the available page center", async ({ page }) => {
  await installTauriMock(page, { locale: "ru", mode: "remote", theme: "dark", populated: true, remoteFeatures: ["accounts"] });
  await page.setViewportSize({ width: 1160, height: 760 });
  await page.goto("/");
  await page.getByRole("button", { name: "Использование", exact: true }).click();
  await expect(page.getByText("Не поддерживается", { exact: true })).toBeVisible();
  await expectTopLevelEmptyCentered(page);
  await page.screenshot({ path: "output/playwright/usage-unsupported-centered-ru-dark-1160x760.png" });
});

test("automation list fits the standard window without horizontal scrolling", async ({ page }) => {
  await installTauriMock(page, { locale: "ru", mode: "local", theme: "dark", populated: true });
  await page.setViewportSize({ width: 1160, height: 760 });
  await page.goto("/");
  await page.getByRole("button", { name: "Подключения", exact: true }).click();
  await page.getByRole("tab", { name: "Автоматизация" }).click();
  const table = page.locator(".automation-list");
  expect(await table.evaluate((element) => element.scrollWidth <= element.clientWidth)).toBe(true);
});

test("weekly quota reset confirmation stays concise in the compact dark window", async ({ page }) => {
  await installTauriMock(page, { locale: "ru", mode: "local", theme: "dark", populated: true, resetCreditsAvailable: 1 });
  await page.setViewportSize({ width: 840, height: 560 });
  await page.goto("/");
  await page.getByRole("button", { name: "Подключения", exact: true }).click();
  await page.getByRole("button", { name: "Доступен сброс: 1 · Сбросить недельную квоту", exact: true }).click();

  const dialog = page.getByRole("dialog", { name: "Сбросить недельную квоту" });
  await expect(dialog.getByText("Сбросить недельную квоту для этой учётной записи?", { exact: true })).toBeVisible();
  await expect(dialog.getByRole("button", { name: "Нет", exact: true })).toBeVisible();
  await expect(dialog.getByRole("button", { name: "Да, сбросить", exact: true })).toBeVisible();
  expect(await dialog.evaluate((element) => {
    const rect = element.getBoundingClientRect();
    return rect.left >= 0 && rect.right <= innerWidth && rect.top >= 36 && rect.bottom <= innerHeight
      && element.scrollWidth <= element.clientWidth;
  })).toBe(true);
  await page.screenshot({ path: "output/playwright/reset-confirm-after-ru-dark-840x560.png" });
});

test("automation editor fits the compact window without hidden controls", async ({ page }) => {
  await installTauriMock(page, { locale: "ru", mode: "local", theme: "dark", populated: true, accountCount: 3 });
  await page.setViewportSize({ width: 840, height: 560 });
  await page.goto("/");
  await page.getByRole("button", { name: "Подключения", exact: true }).click();
  await page.getByRole("tab", { name: "Автоматизация" }).click();
  await page.getByRole("button", { name: "Добавить автоматизацию" }).click();

  const dialog = page.getByRole("dialog", { name: "Добавить автоматизацию" });
  await expect(dialog.getByText("После восстановления основной квоты", { exact: true })).toBeVisible();
  await expect(dialog.getByText("Цель", { exact: true })).toHaveCount(0);
  await expect(dialog.getByText("Выполнение", { exact: true })).toHaveCount(0);
  await expect(dialog.getByText("Самая лёгкая поддерживаемая", { exact: true })).toHaveCount(0);
  await dialog.getByRole("button", { name: /^Модель:/ }).click();
  await expect(page.locator('[role="option"][data-value="gpt-5.4"]')).toBeVisible();
  await expect(page.locator('[role="option"][data-value="gpt-5.4-mini"]')).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(dialog.getByRole("button", { name: "Автоматически" })).toHaveCount(0);
  await expect(dialog.getByRole("button", { name: "Вручную" })).toHaveCount(0);
  expect(await dialog.evaluate((element) => {
    const rect = element.getBoundingClientRect();
    const body = element.querySelector(".relay-dialog-body");
    const execution = element.querySelector("footer");
    const bodyRect = body?.getBoundingClientRect();
    const executionRect = execution?.getBoundingClientRect();
    return rect.left >= 0 && rect.right <= innerWidth && rect.top >= 36 && rect.bottom <= innerHeight
      && element.scrollWidth <= element.clientWidth
      && Boolean(body && body.scrollHeight <= body.clientHeight)
      && Boolean(bodyRect && executionRect && executionRect.top >= bodyRect.bottom && executionRect.bottom <= innerHeight);
  })).toBe(true);
  await page.screenshot({ path: "output/playwright/automation-dialog-ru-dark-840x560.png" });

  await dialog.getByRole("button", { name: /^Аккаунты:/ }).click();
  await page.locator('[role="option"][data-value="account_ids"]').click();
  await dialog.getByLabel("Personal Plus").check();
  await dialog.getByLabel("Backup account").check();
  await expect(dialog.getByRole("button", { name: "Модель: gpt-5.4-mini" })).toBeVisible();
  expect(await dialog.evaluate((element) => {
    const rect = element.getBoundingClientRect();
    const body = element.querySelector(".relay-dialog-body");
    const execution = element.querySelector("footer");
    const bodyRect = body?.getBoundingClientRect();
    const executionRect = execution?.getBoundingClientRect();
    return rect.left >= 0 && rect.right <= innerWidth && rect.top >= 36 && rect.bottom <= innerHeight
      && element.scrollWidth <= element.clientWidth
      && Boolean(body && body.scrollHeight <= body.clientHeight)
      && Boolean(bodyRect && executionRect && executionRect.top >= bodyRect.bottom && executionRect.bottom <= innerHeight);
  })).toBe(true);
  await page.screenshot({ path: "output/playwright/automation-dialog-selected-ru-dark-840x560.png" });
});

test("ru compact disclosure labels stay readable", async ({ page }) => {
  await installTauriMock(page, { locale: "ru", mode: "local", theme: "dark", populated: true });
  await page.setViewportSize({ width: 840, height: 560 });
  await page.goto("/");

  await page.locator(".mode-picker > button").click();
  const modeMenu = page.getByRole("menu");
  await expect(modeMenu.getByRole("menuitemradio")).toHaveCount(3);
  expect(await modeMenu.locator("span").evaluateAll((items) => items.every((item) => item.scrollWidth <= item.clientWidth))).toBe(true);
  await page.screenshot({ path: "output/playwright/shell-mode-menu-ru-840x560.png" });
  await page.keyboard.press("Escape");

  await page.locator(".relay-sidebar nav button").nth(1).click();
  await page.getByRole("tab", { name: "Источники API" }).click();
  await page.locator(".relay-table .row-actions summary").click();
  let menu = page.getByRole("menu");
  expect(await menu.locator("span").evaluateAll((items) => items.every((item) => item.scrollWidth <= item.clientWidth))).toBe(true);
  await page.screenshot({ path: "output/playwright/source-actions-ru-840x560.png" });
  await page.keyboard.press("Escape");

  await page.locator(".relay-sidebar nav button").nth(2).click();
  await page.getByRole("button", { name: "Правила участника пула: Personal Plus", exact: true }).click();
  let dialog = page.getByRole("dialog", { name: /Правила участника пула/ });
  await expect(dialog).toBeVisible();
  await expect(dialog).not.toContainText("Приоритет при равенстве");
  await expect(dialog).not.toContainText("Доля трафика");
  await expect(dialog.getByRole("tab", { name: "Модели", exact: true })).toHaveAttribute("aria-selected", "true");
  await expect(dialog.locator("[data-member-model-id]")).toHaveCount(2);
  expect(await dialog.locator("[data-member-model-id]").evaluateAll((rows) => rows.every((row) => row.scrollWidth <= row.clientWidth))).toBe(true);
  await page.screenshot({ path: "output/playwright/pool-member-dialog-ru-840x560.png" });
  await dialog.getByRole("button", { name: "Закрыть" }).first().click();

  await page.locator(".relay-sidebar nav button").nth(4).click();
  await page.getByRole("button", { name: "Сведения о запросе: req_synthetic_local" }).click();
  dialog = page.getByRole("dialog", { name: "Сведения о запросе" });
  await expect(dialog).toContainText("req_synthetic_local");
  await expect(dialog).toContainText("РазмышлениеЗапрошено: Максимальное, передано: Низкое");
  await dialog.getByRole("tab", { name: "Маршрут", exact: true }).click();
  await expect(dialog).toContainText("Причина выбораНаибольший остаток квоты");
  await expect(dialog).toContainText("Доступных участников4");
  await expect(dialog).toContainText("Квота при выборе63.00%");
  expect(await dialog.locator(".request-details-list > div").evaluateAll((items) => items.every((item) => item.scrollWidth <= item.clientWidth))).toBe(true);
  expect(await dialog.evaluate((element) => {
    const rect = element.getBoundingClientRect();
    return rect.left >= 0 && rect.right <= innerWidth && rect.top >= 36 && rect.bottom <= innerHeight;
  })).toBe(true);
  await page.screenshot({ path: "output/playwright/request-details-dialog-ru-840x560.png" });
  await dialog.getByRole("button", { name: "Закрыть" }).first().click();
});

for (const scenario of [
  { locale: "en" as const, theme: "light" as const, width: 1160, label: "First / total", file: "usage-timing-en-light-1160x760.png" },
  { locale: "ru" as const, theme: "dark" as const, width: 840, label: "Первый / всего", file: "usage-timing-ru-dark-840x560.png" },
]) {
  test(`usage timing ${scenario.locale} ${scenario.theme} ${scenario.width}`, async ({ page }) => {
    await installTauriMock(page, { locale: scenario.locale, mode: "local", theme: scenario.theme, populated: true });
    await page.setViewportSize({ width: scenario.width, height: scenario.width === 840 ? 560 : 760 });
    await page.goto("/");
    await page.locator(".relay-sidebar nav button").nth(4).click();

    if (scenario.width === 840) {
      await page.locator(".usage-metrics > div").nth(2).evaluate((card) => {
        card.querySelector("strong")!.textContent = "49,3 млн";
        card.querySelector("small")!.textContent = "Вх. 49,2 млн · Кэш ↓ 43,6 млн · Вых. 137,3 тыс.";
      });
      await page.locator(".usage-metrics > div").nth(3).evaluate((card) => {
        card.querySelector("strong")!.textContent = "≈54,0431 $";
        card.querySelector("small")!.textContent = "Оценено токенов: 49,3 млн";
      });
    }
    await expect(page.getByRole("columnheader", { name: scenario.label })).toBeVisible();
    expect(await page.locator(".usage-metrics").evaluate((grid) => getComputedStyle(grid).gridTemplateColumns.split(" ").length)).toBe(scenario.width === 840 ? 2 : 3);
    if (scenario.width === 1160) {
      expect(await page.locator(".usage-metrics > div").evaluateAll((cards) => new Set(cards.map((card) => Math.round(card.getBoundingClientRect().top))).size)).toBe(2);
    }
    expect(await page.locator(".usage-metrics > div").evaluateAll((cards) => cards.every((card) => card.scrollWidth <= card.clientWidth))).toBe(true);
    expect(await page.locator(".usage-overview strong").evaluateAll((values) => new Set(values.map((value) => getComputedStyle(value).fontSize)).size)).toBe(1);
    expect(await page.locator(".usage-metrics > div").evaluateAll((items) => items.every((item) => getComputedStyle(item).textAlign === "left"))).toBe(true);
    expect(await page.locator(".usage-request-table th, .usage-request-table td").evaluateAll((items) => items.every((item) => getComputedStyle(item).textAlign === "center"))).toBe(true);
    const timing = page.getByRole("row").filter({ hasText: "req_synthetic_local" }).locator('td[data-column="timing"]');
    await expect(timing).toHaveText(scenario.locale === "ru" ? "128 мс / 428 мс" : "128 ms / 428 ms");
    expect(await timing.evaluate((element) => element.scrollWidth <= element.clientWidth)).toBe(true);
    const clippedHeaders = await page.locator(".usage-request-table th").evaluateAll((items) => items.filter((item) => item.scrollWidth > item.clientWidth || item.scrollHeight > item.clientHeight).map((item) => item.textContent));
    expect(clippedHeaders).toEqual([]);
    const clippedHeaderLabels = await page.locator(".usage-request-table .usage-column-heading > span").evaluateAll((items) => items.filter((item) => item.scrollWidth > item.clientWidth).map((item) => item.textContent));
    expect(clippedHeaderLabels).toEqual([]);
    await page.screenshot({ path: `output/playwright/${scenario.file}` });
  });
}

for (const viewport of viewports) {
  test(`usage filter hierarchy ${viewport.width}x${viewport.height}`, async ({ page }) => {
    await installTauriMock(page, { locale: "ru", mode: "local", theme: "dark", populated: true });
    await page.setViewportSize(viewport);
    await page.goto("/");
    await page.getByRole("button", { name: "Использование", exact: true }).click();

    const filters = page.locator(".usage-filter-panel");
    await expect(page.locator(".usage-range-menu").getByRole("button", { name: /^Период:/ })).toBeVisible();
    await expect(filters.getByRole("button", { name: /^Модель:/ })).toBeVisible();
    await expect(filters.getByRole("button", { name: /^Участник пула:/ })).toBeVisible();
    await expect(filters.getByLabel("Локальный ключ")).toHaveCount(0);
    await filters.getByRole("button", { name: "Другие фильтры" }).click();
    await expect(filters.getByLabel("Локальный ключ")).toHaveCount(0);
    await expect(filters.getByRole("button", { name: /^Категория ошибки:/ })).toBeVisible();
    const protocol = filters.getByRole("button", { name: /^Протокол:/ });
    await protocol.click();
    await expect(protocol).toHaveAttribute("aria-expanded", "true");
    await page.getByRole("option", { name: "Responses", exact: true }).click();
    await expect(filters.locator(".usage-filter-toggle-wrap small")).toHaveText("1");
    await expect(filters.getByRole("button", { name: "Сбросить фильтры" })).toBeVisible();
    await filters.getByRole("button", { name: "Сбросить фильтры" }).click();
    await expect(filters.getByRole("button", { name: "Протокол: Любой протокол" })).toBeVisible();
    await expect(filters.getByRole("button", { name: "Сбросить фильтры" })).toHaveCount(0);
    await page.screenshot({ path: `output/playwright/usage-filters-open-ru-dark-${viewport.width}x${viewport.height}.png` });

    await page.getByRole("tab", { name: "Модели" }).click();
    const aggregate = page.locator(".usage-aggregate-table");
    await expect(aggregate.getByRole("columnheader")).toHaveText(["Модель", "Запросы", "Входные токены", "Выходные токены", "Прочитано из кэша", "API-экв."]);
    expect(await aggregate.locator("th, td").evaluateAll((cells) => cells.every((cell) => cell.scrollWidth <= cell.clientWidth && cell.scrollHeight <= cell.clientHeight + 1))).toBe(true);
    expect(await aggregate.locator("xpath=..").evaluate((element) => element.scrollWidth <= element.clientWidth)).toBe(true);
    await page.screenshot({ path: `output/playwright/usage-models-ru-dark-${viewport.width}x${viewport.height}.png` });
  });
}
