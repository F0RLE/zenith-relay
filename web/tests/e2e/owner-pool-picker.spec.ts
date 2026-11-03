import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";

async function prepareInventory(page: Page) {
  await page.goto("/admin/pools");
  await page.getByRole("checkbox", { name: "Выбрать все показанные подключения" }).check();
  await page.locator(".owner-resource-bulk:visible").getByRole("button", { name: "Убрать из пула", exact: true }).click();
  await expect(page.locator(".owner-resource-card:visible")).toHaveCount(0);
  await page.getByRole("button", { name: "Добавить участника", exact: true }).click();
  return page.getByRole("dialog", { name: "Добавить подключения в пул", exact: true });
}

test("pool picker matches Desktop geometry with a pinned sidebar, scroll area and footer", async ({ page }, info) => {
  const dialog = await prepareInventory(page);
  await expect(dialog.getByRole("searchbox")).toBeFocused();
  await expect(dialog.locator(".owner-pool-picker-option")).toHaveCount(4);
  await expect(dialog.locator(".owner-pool-picker-sidebar nav button")).toHaveCount(4);
  await expect(dialog.getByRole("button", { name: "Добавить в пул", exact: true })).toBeDisabled();
  await expect(dialog).not.toContainText("Только сохранённые подключения");
  const box = (await dialog.boundingBox())!;
  const viewport = page.viewportSize()!;
  expect(box.x).toBeGreaterThanOrEqual(0);
  expect(box.y).toBeGreaterThanOrEqual(0);
  expect(box.x + box.width).toBeLessThanOrEqual(viewport.width);
  expect(box.y + box.height).toBeLessThanOrEqual(viewport.height);
  expect(await dialog.evaluate(el => el.scrollWidth <= el.clientWidth)).toBe(true);
  expect(await dialog.locator(".dialog-body").evaluate(el => getComputedStyle(el).rowGap)).toBe("0px");
  expect(await dialog.locator(".dialog-header").evaluate(el => getComputedStyle(el).borderBottomWidth)).toBe("0px");
  if (viewport.width > 640) {
    expect(box.width).toBe(Math.min(820, viewport.width - 32));
    expect(box.height).toBe(Math.min(620, viewport.height - 84));
    expect((await dialog.locator(".owner-pool-picker-sidebar").boundingBox())!.width).toBe(200);
  }
  const footer = (await dialog.locator(".owner-pool-picker-footer").boundingBox())!;
  if (viewport.width > 640) expect(footer.height).toBe(68);
  const list = (await dialog.locator(".owner-pool-picker-list").boundingBox())!;
  expect(list.y + list.height).toBeLessThanOrEqual(footer.y);
  await page.screenshot({ path: info.outputPath("pool-picker.png"), animations: "disabled" });
  if (viewport.width <= 640) {
    await dialog.getByRole("checkbox", { name: "Primary API, Вне пула", exact: true }).focus();
    await page.keyboard.press("Space");
    await expect(dialog.getByRole("checkbox", { name: "Primary API, Вне пула", exact: true })).toBeChecked();
    expect(await dialog.locator(".owner-pool-picker-list").evaluate(el => el.scrollTop)).toBeGreaterThan(0);
    expect((await dialog.locator(".owner-pool-picker-footer").boundingBox())!.y).toBe(footer.y);
  }
});

test("selection survives views and search, select shown only affects visible rows, confirm is batched", async ({ page }, info) => {
  const dialog = await prepareInventory(page);
  await dialog.getByRole("checkbox", { name: "Code account, Вне пула", exact: true }).check();
  await expect(dialog.getByRole("checkbox", { name: "Выбрать показанные", exact: true })).toHaveJSProperty("indeterminate", true);
  await dialog.getByRole("button", { name: "Источники API", exact: true }).click();
  await dialog.getByRole("searchbox").fill("backup.example.com");
  await expect(dialog.locator(".owner-pool-picker-option")).toHaveCount(1);
  await dialog.getByRole("checkbox", { name: "Выбрать показанные", exact: true }).check();
  await dialog.getByRole("searchbox").fill("");
  await dialog.getByRole("button", { name: /^Выбранные/ }).click();
  await expect(dialog.locator(".owner-pool-picker-option")).toHaveCount(2);
  await expect(dialog.locator(".owner-pool-picker-summary")).toHaveText("Выбрано: 2");
  await page.screenshot({ path: info.outputPath("pool-picker-selected.png"), animations: "disabled" });
  await dialog.getByRole("button", { name: "Добавить в пул", exact: true }).click();
  await expect(dialog).toHaveCount(0);
  await expect(page.locator(".owner-resource-card:visible h2")).toHaveText(["Code account", "Backup API"]);
});

test("cancel and Escape discard selection and restore the original trigger", async ({ page }) => {
  const dialog = await prepareInventory(page);
  const open = page.getByRole("button", { name: "Добавить участника", exact: true });
  await dialog.getByRole("checkbox", { name: "Выбрать показанные", exact: true }).check();
  await dialog.getByRole("button", { name: "Отмена", exact: true }).click();
  await expect(open).toBeFocused();
  await expect(page.locator(".owner-resource-card:visible")).toHaveCount(0);
  await open.click();
  await expect(dialog.getByRole("button", { name: "Добавить в пул", exact: true })).toBeDisabled();
  await page.keyboard.press("Escape");
  await expect(dialog).toHaveCount(0);
  await expect(open).toBeFocused();
});

test("new provider returns to the picker without losing selection or silently joining the pool", async ({ page }) => {
  const picker = await prepareInventory(page);
  await picker.getByRole("checkbox", { name: "Code account, Вне пула", exact: true }).check();
  await picker.getByRole("button", { name: "Новый провайдер", exact: true }).click();
  const source = page.getByRole("dialog", { name: "Добавить API-подключение", exact: true });
  await expect(source).toBeVisible();
  await expect(page.locator("dialog[open]")).toHaveCount(1);
  await source.getByRole("button", { name: "Отмена", exact: true }).click();
  await expect(picker.getByRole("button", { name: "Новый провайдер", exact: true })).toBeFocused();
  await expect(picker.getByRole("checkbox", { name: "Code account, Вне пула", exact: true })).toBeChecked();
  await picker.getByRole("button", { name: "Новый провайдер", exact: true }).click();
  await source.getByRole("textbox", { name: "Имя", exact: true }).fill("Picker API");
  await source.getByRole("textbox", { name: "Адрес API", exact: true }).fill("https://picker.example.com/v1");
  await source.getByRole("button", { name: "Добавить", exact: true }).click();
  await expect(picker.locator(".owner-pool-picker-summary")).toHaveText("Выбрано: 2");
  await expect(picker.getByRole("checkbox", { name: "Picker API, Вне пула", exact: true })).toBeChecked();
  await expect(page.locator(".owner-resource-card:visible")).toHaveCount(0);
  await picker.getByRole("button", { name: "Добавить в пул", exact: true }).click();
  await expect(page.locator(".owner-resource-card:visible h2")).toHaveText(["Code account", "Picker API"]);
});

test("selected, search and exhausted inventory have honest empty states", async ({ page }) => {
  const dialog = await prepareInventory(page);
  await dialog.getByRole("button", { name: "Выбранные", exact: true }).click();
  await expect(dialog.getByText("Нет выбранных подключений", { exact: true })).toBeVisible();
  await dialog.getByRole("button", { name: "Все подключения", exact: true }).click();
  await dialog.getByRole("searchbox").fill("missing-provider");
  await expect(dialog.getByText("Подключения не найдены", { exact: true })).toBeVisible();
  await expect(dialog.getByRole("checkbox", { name: "Выбрать показанные", exact: true })).toBeDisabled();
  await dialog.getByRole("searchbox").fill("");
  await dialog.getByRole("checkbox", { name: "Выбрать показанные", exact: true }).check();
  await dialog.getByRole("button", { name: "Снять выбор", exact: true }).click();
  await expect(dialog.getByRole("button", { name: "Добавить в пул", exact: true })).toBeDisabled();
  await dialog.getByRole("checkbox", { name: "Выбрать показанные", exact: true }).check();
  await dialog.getByRole("button", { name: "Добавить в пул", exact: true }).click();
  await page.getByRole("button", { name: "Добавить участника", exact: true }).click();
  await expect(dialog.getByText("Нет подключений вне пула", { exact: true })).toBeVisible();
  await expect(dialog.getByRole("button", { name: "Новый провайдер", exact: true })).toBeEnabled();
});

for (const [locale, title] of [["en", "Add connections to pool"], ["zh-CN", "添加连接到资源池"]] as const) {
  test(`pool picker keeps ${locale} text and controls inside the dialog`, async ({ page }) => {
    await page.addInitScript(value => localStorage.setItem("relay-hosted-locale", value), locale);
    await page.goto("/admin/pools");
    await page.locator(".relay-page-actions:visible > button").click();
    const dialog = page.getByRole("dialog", { name: title, exact: true });
    await expect(dialog).toBeVisible();
    expect(await dialog.evaluate(el => el.scrollWidth <= el.clientWidth)).toBe(true);
    for (const button of await dialog.locator(".owner-pool-picker-sidebar button, .owner-pool-picker-footer button").all()) {
      expect(await button.evaluate(el => el.scrollWidth <= el.clientWidth)).toBe(true);
    }
    await expect(dialog.locator(".owner-pool-picker-option")).toHaveCount(1);
    await dialog.locator(".owner-pool-picker-option input").check();
    await expect(dialog.locator(".owner-pool-picker-footer .primary-button")).toBeEnabled();
  });
}
