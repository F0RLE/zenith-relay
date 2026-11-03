import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { previewSnapshot } from "../../src/preview/snapshot";
import { formatMoney, usageTotals } from "../../src/features/usage/model";
import { overviewTimeline } from "../../src/features/overview/timeline";

async function navigate(page: Page, name: string) {
  const nav = page.locator(".relay-sidebar");
  if (!await nav.isVisible()) await page.getByRole("button", { name: "Меню", exact: true }).click();
  await nav.getByRole("button", { name, exact: true }).click();
}

test("owner navigation is compact and removed screens cannot be opened directly", async ({ page }) => {
  await page.goto("/admin/overview");
  const nav = page.locator(".relay-sidebar");
  await expect(nav.locator(".sidebar-main-nav button")).toHaveText(["Обзор", "Подключения", "Пул", "Модели", "Использование", "Уведомления"]);
  for (const id of ["tariffs", "access", "customers", "payments", "operations", "audit"]) {
    await page.goto(`/admin/${id}`);
    await expect(page.getByRole("heading", { name: "Страница не найдена", exact: true })).toBeVisible();
  }
  await page.goto("/admin/models");
  await expect(page.locator(".models-page:visible").getByRole("heading", { name: "Модели и цены", exact: true })).toBeVisible();
});

test("owner overview shows API charges as a chart with an all-time range and no summary cards", async ({ page }, testInfo) => {
  await page.goto("/admin/overview");
  const cost = page.locator(".hosted-chart.cost:visible");
  await expect(page.locator(".owner-overview-summary")).toHaveCount(0);
  await expect(cost.getByRole("heading", { name: "Списано за API", exact: true })).toBeVisible();
  for (const [range, label] of [["24h", "24 часа"], ["week", "7 дней"], ["month", "30 дней"], ["all", "Всё время"]] as const) {
    await page.getByRole("button", { name: label, exact: true }).click();
    const { rows, buckets } = overviewTimeline(previewSnapshot.requests, range, previewSnapshot.asOf, "all");
    const charge = rows.filter(row => row.accounting === "posted").reduce((sum, row) => sum + (row.billing.postedMicroUsd ?? 0n), 0n);
    expect(usageTotals(rows).charge).toBe(charge);
    await expect(cost.locator(".overview-chart-summary")).toHaveText(`${formatMoney(charge, 6, "ru")} USD`);
    await expect(cost.locator(".hosted-chart-points > li")).toHaveCount(buckets.length);
    await expect(page.locator(".hosted-chart.requests:visible .overview-chart-summary")).toHaveText(String(rows.length));
    await expect(page.getByRole("button", { name: label, exact: true })).toHaveAttribute("aria-pressed", "true");
  }
  expect(await page.locator(".relay-content").evaluate(element => element.scrollWidth <= element.clientWidth)).toBe(true);
  const controls = page.getByRole("group", { name: "Период статистики", exact: true });
  const controlsBounds = await controls.boundingBox();
  expect(await controls.evaluate(element => element.scrollHeight <= element.clientHeight)).toBe(true);
  for (const button of await controls.getByRole("button").all()) {
    const bounds = await button.boundingBox();
    expect(bounds!.x).toBeGreaterThanOrEqual(0);
    expect(bounds!.x + bounds!.width).toBeLessThanOrEqual(page.viewportSize()!.width + 1);
    expect(bounds!.y).toBeGreaterThanOrEqual(controlsBounds!.y - 1);
    expect(bounds!.y + bounds!.height).toBeLessThanOrEqual(controlsBounds!.y + controlsBounds!.height + 1);
  }
  await page.screenshot({ path: testInfo.outputPath("owner-overview.png"), fullPage: true });
  await page.goto("/app/overview");
  await expect(page.locator(".owner-overview-summary")).toHaveCount(0);
  await expect(page.getByRole("group", { name: "Период статистики", exact: true }).getByRole("button", { name: "Всё время", exact: true })).toHaveCount(0);
  await expect(page.locator(".hosted-chart.cost")).toContainText("Расходы");
});

test("inventory search, selection, pause, trash and explicit pool membership stay consistent", async ({ page }, testInfo) => {
  await page.goto("/admin/connections");
  const cards = page.locator(".owner-resource-card:visible");
  const code = page.getByRole("article", { name: "Code account", exact: true });
  await page.getByRole("searchbox", { name: "Поиск подключений" }).fill("Code");
  await expect(cards).toHaveCount(1);
  await page.getByRole("checkbox", { name: "Выбрать все показанные подключения" }).check();
  await page.locator(".owner-resource-bulk:visible").getByRole("button", { name: "Убрать из пула", exact: true }).click();
  await expect(code).toContainText("Вне пула");
  await page.getByRole("searchbox", { name: "Поиск подключений" }).fill("");
  await expect(cards).toHaveCount(2);
  await page.getByRole("checkbox", { name: "Выбрать все показанные подключения" }).check();
  await page.locator(".owner-resource-bulk:visible").getByRole("button", { name: "Добавить в пул", exact: true }).click();
  await expect(page.locator('.owner-resource-card:visible[data-status="pool"]')).toHaveCount(2);
  await code.getByRole("button", { name: "Действия: Code account", exact: true }).click();
  await page.getByRole("menuitem", { name: "Приостановить", exact: true }).click();
  await expect(code).toContainText("Приостановлен");
  await page.getByRole("combobox", { name: "Состояние подключений" }).click();
  await page.getByRole("option", { name: "На паузе", exact: true }).click();
  await expect(cards).toHaveCount(1);
  await code.getByRole("button", { name: "Действия: Code account", exact: true }).click();
  await page.getByRole("menuitem", { name: "В корзину", exact: true }).click();
  await expect(cards).toHaveCount(0);
  await page.getByRole("button", { name: "Корзина", exact: true }).click();
  await expect(code).toContainText("В корзине");
  await code.getByRole("button", { name: "Восстановить", exact: true }).click();
  await expect(page.getByText("Корзина пуста", { exact: true })).toBeVisible();
  await page.getByRole("button", { name: "Аккаунты", exact: true }).click();
  await code.getByRole("button", { name: "Действия: Code account", exact: true }).click();
  await page.getByRole("menuitem", { name: "Сведения", exact: true }).click();
  await expect(page.getByRole("dialog")).toContainText("Не участвует");
  await page.keyboard.press("Escape");
  await expect(code.getByRole("button", { name: "Действия: Code account", exact: true })).toBeFocused();
  await page.screenshot({ path: testInfo.outputPath("owner-connections.png"), fullPage: true });
  expect(await page.locator(".relay-content").evaluate(element => element.scrollWidth <= element.clientWidth)).toBe(true);
});

test("pool order, filtered selection and card actions are usable at every viewport", async ({ page }, testInfo) => {
  await page.goto("/admin/pools");
  const cards = page.locator(".owner-resource-card:visible");
  await page.getByRole("button", { name: "Настройки ротации пула", exact: true }).click();
  await page.getByRole("radio", { name: "По кругу", exact: true }).click();
  await page.getByRole("button", { name: "Переместить Code account вниз", exact: true }).click();
  await page.keyboard.press("Escape");
  await expect(cards.locator("h2")).toHaveText(["Text account", "Code account", "Primary API"]);
  await page.getByRole("button", { name: "Настройки ротации пула", exact: true }).click();
  await page.getByRole("button", { name: "Переместить Code account вверх", exact: true }).click();
  await page.keyboard.press("Escape");
  await expect(cards.first().locator("h2")).toHaveText("Code account");
  const edit = cards.first().getByRole("button", { name: "Редактировать Code account", exact: true });
  await edit.click();
  await expect(page.getByRole("dialog", { name: "Настройки участника", exact: true })).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(edit).toBeFocused();
  await page.getByRole("button", { name: "Поиск сохранённых подключений", exact: true }).click();
  await page.getByRole("checkbox", { name: "Выбрать все показанные подключения" }).focus();
  await page.keyboard.press("Space");
  await page.locator(".owner-resource-bulk:visible").getByRole("button", { name: "Приостановить", exact: true }).click();
  await expect(page.locator('.owner-resource-card:visible[data-status="disabled"]')).toHaveCount(3);
  await expect(page.getByRole("searchbox", { name: "Поиск подключений" })).toBeFocused();
  await page.getByRole("checkbox", { name: "Выбрать все показанные подключения" }).check();
  await page.locator(".owner-resource-bulk:visible").getByRole("button", { name: "Возобновить", exact: true }).click();
  await expect(page.locator('.owner-resource-card:visible[data-status="pool"]')).toHaveCount(3);
  await page.getByRole("searchbox", { name: "Поиск подключений" }).fill("несуществующий");
  await expect(page.getByText("Участников не найдено", { exact: true })).toBeVisible();
  await expect(page.getByRole("checkbox", { name: "Выбрать все показанные подключения" })).toBeDisabled();
  await page.getByRole("searchbox", { name: "Поиск подключений" }).fill("");
  expect(await page.locator(".relay-content").evaluate(element => element.scrollWidth <= element.clientWidth)).toBe(true);
  for (const card of await cards.all()) expect(await card.evaluate(element => element.scrollWidth <= element.clientWidth)).toBe(true);
  await page.screenshot({ path: testInfo.outputPath("owner-pool.png"), fullPage: true });
});
