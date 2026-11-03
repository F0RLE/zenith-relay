import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { previewSnapshot } from "../../src/preview/snapshot";

async function openProxies(page: Page) {
  await page.goto("/admin/connections");
  const navigation = page.getByRole("group", { name: "Тип подключения", exact: true });
  await navigation.getByRole("button", { name: "Прокси", exact: true }).click();
  return page.locator(".owner-proxies");
}

async function fits(page: Page) {
  await page.evaluate(() => document.fonts.ready);
  expect(await page.locator(".relay-content").evaluate(element => element.scrollWidth <= element.clientWidth)).toBe(true);
  const dialog = page.getByRole("dialog");
  if (await dialog.count()) {
    const bounds = await dialog.boundingBox();
    expect(bounds!.x).toBeGreaterThanOrEqual(0);
    expect(bounds!.y).toBeGreaterThanOrEqual(0);
    expect(bounds!.x + bounds!.width).toBeLessThanOrEqual(page.viewportSize()!.width + 1);
    expect(bounds!.y + bounds!.height).toBeLessThanOrEqual(page.viewportSize()!.height + 1);
    expect(await dialog.evaluate(element => element.scrollWidth <= element.clientWidth)).toBe(true);
  }
}

async function importProxies(page: Page, content: string) {
  await page.locator(".relay-page-actions").getByRole("button", { name: "Импорт прокси", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "Импорт прокси", exact: true });
  await dialog.getByLabel("Каждый прокси с новой строки", { exact: true }).fill(content);
  await dialog.getByRole("button", { name: /^Импортировать:/ }).click();
  await dialog.locator(".button-row").getByRole("button", { name: "Закрыть", exact: true }).click();
}

test("proxy workspace matches launcher flows without network side effects or stored secrets", async ({ page }, testInfo) => {
  const mutations: string[] = [];
  page.on("request", request => { if (["POST", "PUT", "PATCH", "DELETE"].includes(request.method())) mutations.push(request.url()); });
  const workspace = await openProxies(page);
  await expect(workspace.getByRole("heading", { name: "Прокси пока нет", exact: true })).toBeVisible();
  await fits(page);
  await importProxies(page, "http://demo-user:demo-pass@proxy-one.example:8080\nproxy-two.example:3128\nsocks5://[2001:db8::1]:1080");
  await expect(workspace.getByRole("article")).toHaveCount(3);
  await expect(workspace).not.toContainText("demo-user");
  await expect(workspace).not.toContainText("demo-pass");
  const first = workspace.getByRole("article", { name: "http://proxy-one.example:8080", exact: true });
  await expect(first).toContainText("Не проверен");
  await expect(first.getByRole("button", { name: "Проверить подключение", exact: true })).toBeDisabled();
  await expect(first.getByRole("button", { name: "Проверить скорость", exact: true })).toBeDisabled();
  await fits(page);
  await page.screenshot({ path: testInfo.outputPath("proxy-workspace.png"), fullPage: true });
  await first.getByRole("button", { name: "Управление аккаунтами", exact: true }).click();
  const manager = page.getByRole("dialog", { name: "Управление аккаунтами", exact: true });
  const account = previewSnapshot.owner.connections.find(item => item.kind === "account" && !item.trashed)!;
  await manager.getByRole("checkbox", { name: account.name, exact: true }).check();
  await fits(page);
  await page.screenshot({ path: testInfo.outputPath("proxy-accounts.png"), fullPage: true });
  await manager.getByRole("button", { name: "Сохранить", exact: true }).click();
  await expect(first.locator(".owner-proxy-account-count")).toHaveText("1");
  await first.getByRole("button", { name: /^Действия:/ }).click();
  await page.getByRole("menuitem", { name: "Переименовать прокси", exact: true }).click();
  const rename = page.getByRole("dialog", { name: "Переименовать прокси", exact: true });
  await rename.getByRole("textbox").fill("Work proxy");
  await rename.getByRole("button", { name: "Сохранить", exact: true }).click();
  const renamed = workspace.getByRole("article", { name: "Work proxy", exact: true });
  await expect(renamed).toContainText("http://proxy-one.example:8080");
  await renamed.getByRole("checkbox").check();
  await workspace.getByRole("button", { name: "Удалить выбранные прокси", exact: true }).click();
  const deletion = page.getByRole("dialog", { name: "Удаление прокси", exact: true });
  await expect(deletion).toContainText("прямое подключение");
  await deletion.getByRole("button", { name: "Отмена", exact: true }).click();
  await expect(renamed).toBeVisible();
  await workspace.getByRole("button", { name: "Удалить выбранные прокси", exact: true }).click();
  await page.getByRole("dialog").getByRole("button", { name: "Удалить", exact: true }).click();
  await expect(workspace.getByRole("article")).toHaveCount(2);
  await workspace.getByRole("searchbox", { name: "Поиск прокси", exact: true }).fill("proxy-two");
  await expect(workspace.getByRole("article")).toHaveCount(1);
  expect(await page.evaluate(() => JSON.stringify(localStorage))).not.toContain("demo-pass");
  expect(mutations).toEqual([]);
});

test("proxy import validates rows, masks input and keeps its dialog within the viewport", async ({ page }, testInfo) => {
  await openProxies(page);
  await page.locator(".relay-page-actions").getByRole("button", { name: "Импорт прокси", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "Импорт прокси", exact: true });
  const input = dialog.getByLabel("Каждый прокси с новой строки", { exact: true });
  await expect(input).toBeFocused();
  await expect(input).toHaveCSS("-webkit-text-security", "disc");
  await input.fill("ftp://proxy.example:21");
  await expect(dialog.getByRole("alert")).toContainText("строках: 1");
  await expect(dialog.getByRole("button", { name: /^Импортировать:/ })).toBeDisabled();
  await input.fill("proxy.example:8080");
  await expect(dialog.getByRole("alert")).toHaveCount(0);
  await dialog.getByRole("checkbox", { name: "Показать адреса прокси", exact: true }).check();
  await expect(input).toHaveCSS("-webkit-text-security", "none");
  await fits(page);
  await page.screenshot({ path: testInfo.outputPath("proxy-import.png"), fullPage: true });
  await page.keyboard.press("Escape");
  await expect(dialog).toHaveCount(0);
  await expect(page.locator(".relay-page-actions").getByRole("button", { name: "Импорт прокси", exact: true })).toBeFocused();
});

for (const [locale, tab, importLabel, listLabel, buttonLabel] of [
  ["en", "Proxies", "Import proxies", "One proxy per line", "Import: 1"],
  ["zh-CN", "代理", "导入代理", "每行一个代理", "导入：1"],
] as const) {
  test(`proxy interface fits in ${locale}`, async ({ page }) => {
    await page.addInitScript(value => localStorage.setItem("relay-hosted-locale", value), locale);
    await page.goto("/admin/connections");
    await page.locator(".relay-page-navigation").getByRole("button", { name: tab, exact: true }).click();
    await page.locator(".relay-page-actions").getByRole("button", { name: importLabel, exact: true }).click();
    const dialog = page.getByRole("dialog");
    await dialog.getByLabel(listLabel, { exact: true }).fill("proxy.example:8080");
    await fits(page);
    await dialog.getByRole("button", { name: buttonLabel, exact: true }).click();
    await expect(dialog.getByRole("status")).toBeVisible();
    await fits(page);
  });
}
