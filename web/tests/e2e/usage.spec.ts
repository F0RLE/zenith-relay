import { expect, test } from "@playwright/test";

test.beforeEach(async ({ page }) => { await page.goto("/preview.html?page=usage"); });

test("request ID stays visible while duplicate totals, technical and copy UI are removed", async ({ page }, testInfo) => {
  const errors: string[] = [];
  page.on("pageerror", error => errors.push(error.message));
  await page.getByRole("button", { name: "Детали запроса demo_request_0003", exact: true }).click();
  const dialog = page.getByRole("dialog");
  await expect(dialog.getByRole("heading", { name: "Детали запроса" })).toBeVisible();
  const tokens = dialog.getByRole("region", { name: "Токены" });
  await expect(tokens).toBeVisible();
  await expect(tokens.locator(".token-charge")).toHaveCount(5);
  await expect(dialog.getByTestId("request-charge-total")).toContainText("USD");
  await expect(dialog.getByText("Итоговое списание", { exact: true })).toHaveCount(1);
  await expect(dialog.locator(".tokens-total, .technical-details, .request-footer")).toHaveCount(0);
  await expect(dialog.getByRole("button", { name: /Скопировать/ })).toHaveCount(0);
  await expect(dialog.locator(".request-id code")).toHaveText("demo_request_0003");
  await expect(dialog.locator(".request-id code")).toBeVisible();
  await expect(dialog.locator(".request-id code")).toHaveCSS("user-select", "all");
  const modelBox = (await dialog.locator(".request-identity h3").boundingBox())!;
  const idBox = (await dialog.locator(".request-id").boundingBox())!;
  expect(idBox.y).toBeGreaterThanOrEqual(modelBox.y + modelBox.height);
  await expect(dialog.locator("summary").filter({ hasText: "Расчёт стоимости" })).toHaveCount(0);
  await expect(dialog).not.toContainText("Технические сведения");
  await expect(dialog).not.toContainText("Без текста запроса, ответа и секретов");
  await expect(dialog).not.toContainText("Тариф на момент запроса");
  await expect(dialog.locator(".dialog-body > :last-child")).toHaveAttribute("aria-labelledby", "request-tokens-title");
  await expect(dialog).not.toContainText("Участник пула");
  await expect(page.locator(".relay-app")).toHaveJSProperty("scrollWidth", await page.locator(".relay-app").evaluate(element => element.clientWidth));
  await expect(dialog).toHaveJSProperty("scrollWidth", await dialog.evaluate(element => element.clientWidth));
  await dialog.screenshot({ path: testInfo.outputPath("request-details-lean.png") });
  await page.keyboard.press("Escape");
  await expect(dialog).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Детали запроса demo_request_0003", exact: true })).toBeFocused();
  expect(errors).toEqual([]);
});

test("running and completed-but-pending requests never look free", async ({ page }) => {
  await page.getByRole("button", { name: "Детали запроса demo_request_0001", exact: true }).click();
  const dialog = page.getByRole("dialog");
  await expect(dialog.getByRole("heading", { name: "Запрос выполняется" })).toBeVisible();
  await expect(dialog.getByTestId("request-charge-total")).toHaveText("—");
  await expect(dialog).not.toContainText("Удержано");
  await dialog.getByRole("button", { name: "Закрыть", exact: true }).click();
  await page.getByRole("button", { name: "Детали запроса demo_request_0002", exact: true }).click();
  await expect(dialog).toContainText("Ответ завершён, расчёт ожидается");
  await expect(dialog).toContainText("не бесплатный запрос");
  await expect(dialog.getByTestId("request-charge-total")).toHaveText("—");
});

test("filters survive detail close; key groups drill into requests", async ({ page }) => {
  await page.getByRole("combobox", { name: "API-ключ", exact: true }).click();
  await page.getByRole("option", { name: "Редактор", exact: true }).click();
  await page.getByRole("button", { name: "Детали запроса demo_request_0001", exact: true }).click();
  await page.getByRole("button", { name: "Закрыть", exact: true }).click();
  await expect(page.getByRole("combobox", { name: "API-ключ", exact: true })).toHaveText("Редактор");
  await page.getByRole("button", { name: "Сбросить фильтры", exact: true }).click();
  await page.getByRole("group", { name: "Представление использования" }).getByRole("button", { name: "API-ключи", exact: true }).click();
  await page.getByRole("button", { name: "Мой проект", exact: true }).click();
  await expect(page.getByRole("combobox", { name: "API-ключ", exact: true })).toHaveText("Мой проект");
  await page.getByRole("button", { name: "Дополнительные фильтры", exact: true }).click();
  await page.getByLabel("ID запроса или клиент").fill("not-a-real-request");
  await expect(page.getByText("Запросов не найдено", { exact: true })).toBeVisible();
});

test("failed request keeps consumed charge and guidance without technical or copy UI", async ({ page }) => {
  await page.getByRole("group", { name: "Представление использования" }).getByRole("button", { name: "Ошибки", exact: true }).click();
  await page.getByRole("button", { name: /Детали запроса demo_request_/ }).first().click();
  const dialog = page.getByRole("dialog");
  await expect(dialog).toContainText("Перед повтором проверьте результат");
  await expect(dialog.getByTestId("request-charge-total")).not.toHaveText("—");
  await expect(dialog.getByRole("button", { name: /Скопировать/ })).toHaveCount(0);
  await expect(dialog).not.toContainText("HTTP-статус");
  await expect(dialog).not.toContainText("Код ошибки");
  await expect(dialog.locator(".request-id code")).toBeVisible();
});

test("real entry is not the synthetic cabinet", async ({ page }) => {
  await page.goto("/");
  await expect(page.getByRole("heading", { name: "Relay Hosted", exact: true })).toBeVisible();
  await expect(page.getByText("Подключение кабинета к Hosted API ещё не реализовано.", { exact: true })).toBeVisible();
  await expect(page.locator(".request-table")).toHaveCount(0);
});

test("outlined controls share the quiet header border in both themes", async ({ page }, testInfo) => {
  const border = await page.locator(".topbar .notification-button").evaluate(element => getComputedStyle(element).borderTopColor);
  const controls = page.locator(".usage-viewbar .icon-button, .usage-filters .icon-button, .usage-filters .option-trigger, .pagination .icon-button");
  expect(await controls.count()).toBeGreaterThan(5);
  expect(await controls.evaluateAll((elements, expected) => elements.every(element => getComputedStyle(element).borderTopColor === expected), border)).toBe(true);
  await page.getByRole("button", { name: "Дополнительные фильтры", exact: true }).click();
  const search = page.getByRole("searchbox", { name: "ID запроса или клиент", exact: true });
  await expect(search).toHaveCSS("border-top-color", border);
  await page.screenshot({ path: testInfo.outputPath("soft-control-borders.png") });
  await page.getByRole("button", { name: "Баланс: 42,80 USD", exact: true }).click();
  await page.getByRole("button", { name: "Пополнить", exact: true }).click();
  await expect(page.getByRole("spinbutton", { name: "Сумма, USD", exact: true })).toHaveCSS("border-top-color", border);
});
