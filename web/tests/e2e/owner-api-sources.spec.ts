import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";

async function openSources(page: Page) {
  await page.goto("/admin/connections");
  await page.getByRole("button", { name: "API-провайдеры", exact: true }).click();
}

async function fits(page: Page) {
  await page.evaluate(() => document.fonts.ready);
  expect(await page.locator(".relay-content").evaluate(el => el.scrollWidth <= el.clientWidth)).toBe(true);
  for (const card of await page.locator('.owner-resource-card:visible[data-kind="api"]').all()) {
    expect(await card.evaluate(el => el.scrollWidth <= el.clientWidth)).toBe(true);
    const widths = await card.locator(".owner-resource-footer > *").evaluateAll(items => items.map(el => el.getBoundingClientRect().width));
    expect(widths).toHaveLength(4);
    expect(Math.max(...widths) - Math.min(...widths)).toBeLessThan(1);
  }
}

test("API cards show endpoint, provider amounts and a compact model count in both workspaces", async ({ page }, testInfo) => {
  await openSources(page);
  const card = page.getByRole("article", { name: "Primary API", exact: true });
  await expect(card.locator(".owner-source-endpoint")).toHaveText("https://api.example.com/v1");
  await expect(card.locator(".owner-source-values dt")).toHaveText(["Баланс", "Потрачено"]);
  await expect(card.locator(".owner-source-values dd").first()).toContainText("125,80");
  await expect(card.locator(".owner-source-values dd").last()).toContainText("15,40");
  await expect(page.getByRole("article", { name: "Backup API", exact: true }).locator(".owner-source-values dd")).toHaveText(["—", "—"]);
  await expect(card.locator(".owner-resource-models")).toHaveCount(0);
  expect((await card.boundingBox())!.height).toBeLessThan(240);
  await fits(page);
  await page.screenshot({ path: testInfo.outputPath("api-connections.png"), animations: "disabled", fullPage: true });
  await card.getByRole("button", { name: "Приостановить", exact: true }).click();
  await expect(card).toHaveAttribute("data-status", "disabled");
  await card.getByRole("button", { name: "Возобновить", exact: true }).click();
  await expect(card).toHaveAttribute("data-status", "pool");
  await expect(card.getByRole("button", { name: "Обновить источник", exact: true })).toBeDisabled();
  await page.goto("/admin/pools");
  await fits(page);
  await page.screenshot({ path: testInfo.outputPath("api-pool.png"), animations: "disabled", fullPage: true });
});

test("source models expose only core-resolved protocols and support local editing", async ({ page }, testInfo) => {
  await openSources(page);
  const card = page.getByRole("article", { name: "Primary API", exact: true });
  await card.getByRole("button", { name: "Модели и протоколы: 3", exact: true }).click();
  const dialog = page.getByRole("dialog");
  await expect(dialog.getByRole("textbox", { name: "ID модели", exact: true })).toBeFocused();
  await expect(dialog.locator(".owner-source-model-list li > div > span")).toHaveText(["Responses", "Messages", "Chat Completions"]);
  await page.screenshot({ path: testInfo.outputPath("api-models.png"), animations: "disabled" });
  await dialog.getByRole("textbox", { name: "ID модели", exact: true }).fill("demo-gpt");
  await expect(dialog.getByRole("button", { name: "Добавить модель", exact: true })).toBeDisabled();
  await dialog.getByRole("textbox", { name: "ID модели", exact: true }).fill("manual-test-model");
  await dialog.getByRole("textbox", { name: "ID модели", exact: true }).press("Enter");
  await expect(dialog.locator(".owner-source-model-list li").last()).toContainText("Протокол не подтверждён");
  await dialog.getByRole("button", { name: "Сохранить", exact: true }).click();
  await expect(card.getByRole("button", { name: "Модели и протоколы: 4", exact: true })).toBeVisible();
  await card.getByRole("button", { name: "Редактировать API-подключение: Primary API", exact: true }).click();
  await dialog.getByRole("textbox", { name: "Имя", exact: true }).fill("Renamed API");
  await page.screenshot({ path: testInfo.outputPath("api-editor.png"), animations: "disabled" });
  await dialog.getByRole("button", { name: "Сохранить", exact: true }).click();
  await expect(page.getByRole("article", { name: "Renamed API", exact: true })).toBeVisible();
  await fits(page);
});

test("adding an API source accepts configuration only, never keys or provider requests", async ({ page }, testInfo) => {
  const external: string[] = [];
  page.on("request", request => { if (/^https?:/.test(request.url()) && new URL(request.url()).hostname !== "127.0.0.1") external.push(request.url()); });
  await openSources(page);
  await page.getByRole("button", { name: "Добавить подключение", exact: true }).click();
  const dialog = page.getByRole("dialog");
  await expect(dialog.getByRole("heading", { name: "Добавить API-подключение", exact: true })).toBeVisible();
  await expect(dialog.locator('input[type="password"]')).toHaveCount(0);
  await expect(dialog).toContainText("ключи провайдеров не принимаются");
  await dialog.getByRole("textbox", { name: "Имя", exact: true }).fill("Team API");
  const endpoint = dialog.getByRole("textbox", { name: "Адрес API", exact: true });
  await endpoint.fill("https://user:synthetic@example.com/v1?key=synthetic");
  await endpoint.press("Tab");
  await expect(endpoint).toHaveAttribute("aria-invalid", "true");
  await expect(dialog.getByRole("button", { name: "Добавить", exact: true })).toBeDisabled();
  await endpoint.fill("https://team.example.com/v1");
  await dialog.getByRole("combobox", { name: "Протокол подключения", exact: true }).click();
  const menu = dialog.getByRole("listbox", { name: "Протокол подключения", exact: true });
  await expect(menu).toBeVisible();
  const bounds = (await menu.boundingBox())!;
  expect(bounds.x).toBeGreaterThanOrEqual(0);
  expect(bounds.x + bounds.width).toBeLessThanOrEqual(page.viewportSize()!.width);
  expect(bounds.y).toBeGreaterThanOrEqual(0);
  expect(bounds.y + bounds.height).toBeLessThanOrEqual(page.viewportSize()!.height);
  await page.screenshot({ path: testInfo.outputPath("api-protocol-menu.png"), animations: "disabled" });
  await page.getByRole("option", { name: "Messages", exact: true }).click();
  await dialog.getByRole("button", { name: "Модели", exact: true }).click();
  await dialog.getByRole("textbox", { name: "ID модели", exact: true }).fill("team-model");
  await dialog.getByRole("button", { name: "Добавить модель", exact: true }).click();
  await dialog.getByRole("button", { name: "Добавить", exact: true }).click();
  const card = page.getByRole("article", { name: "Team API", exact: true });
  await expect(card).toHaveAttribute("data-status", "idle");
  await expect(card.locator(".owner-source-values dd")).toHaveText(["—", "—"]);
  await expect(card.getByRole("button", { name: "Модели и протоколы: 1", exact: true })).toBeVisible();
  await card.getByRole("button", { name: "Добавить в пул", exact: true }).click();
  await expect(card).toHaveAttribute("data-status", "pool");
  await fits(page);
  await page.screenshot({ path: testInfo.outputPath("api-added.png"), animations: "disabled", fullPage: true });
  expect(external).toEqual([]);
});

test("source editing cancels without mutations and restores keyboard focus", async ({ page }) => {
  await page.goto("/admin/pools");
  const card = page.getByRole("article", { name: "Primary API", exact: true });
  const edit = card.getByRole("button", { name: "Редактировать API-подключение: Primary API", exact: true });
  await edit.click();
  const dialog = page.getByRole("dialog");
  await dialog.getByRole("textbox", { name: "Имя", exact: true }).fill("Unsaved name");
  await page.keyboard.press("Escape");
  await expect(dialog).toHaveCount(0);
  await expect(edit).toBeFocused();
  await expect(card.locator("h2")).toHaveText("Primary API");
  await fits(page);
});
