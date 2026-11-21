import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { customerPages, ownerPages } from "../../src/features/app/routes";
import messages from "../../src/i18n/messages/ru.json" with { type: "json" };

async function navigate(page: Page, name: string) {
  await expect(page.locator(".relay-shell")).toBeVisible();
  const nav = page.locator(".relay-sidebar");
  if (!await nav.isVisible()) await page.getByRole("button", { name: "Меню", exact: true }).click();
  await nav.getByRole("button", { name, exact: true }).click();
}

test("Hosted customer routes and owner routes stay separate with no desktop or backend calls", async ({ page }) => {
  const errors: string[] = [], requests: string[] = [];
  page.on("pageerror", error => errors.push(error.message));
  page.on("request", request => { if (request.method() !== "GET" || !request.url().startsWith("http://127.0.0.1:1442/")) requests.push(`${request.method()} ${request.url()}`); });
  for (const [base, pages] of [["app", customerPages], ["admin", ownerPages]] as const) {
    await page.goto(`/${base}/overview`);
    for (const destination of pages) {
      await navigate(page, messages[destination.name]);
      await expect(page).toHaveURL(new RegExp(`/${base}/${destination.id}$`));
      await expect(page.getByRole("heading", { level: 1 })).toHaveText(messages[destination.id === "models" ? "modelCatalog.title" : destination.name]);
      expect(await page.locator("body").innerText()).not.toMatch(/демо|вымышл|макет|учебн|до перезагрузки/iu);
      await expect(page.locator(".preview-note")).toHaveCount(0);
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    }
    await expect(page.getByRole("button", { name: "Локально", exact: true, includeHidden: true })).toHaveCount(0);
    await expect(page.getByRole("button", { name: "Приложения", exact: true, includeHidden: true })).toHaveCount(0);
    await expect(page.getByRole("button", { name: "Профили", exact: true, includeHidden: true })).toHaveCount(0);
    await expect(page.getByRole("button", { name: "Настройки", exact: true, includeHidden: true })).toHaveCount(0);
  }
  expect(errors).toEqual([]); expect(requests).toEqual([]);
});

test("Hosted links support reload, Back and unknown-route handling", async ({ page }) => {
  await page.goto("/app");
  await expect(page).toHaveURL(/\/app\/overview$/);
  await navigate(page, "API-ключи");
  await navigate(page, "Баланс");
  await page.goBack();
  await expect(page.getByRole("heading", { name: "API-ключи", exact: true })).toBeVisible();
  await page.reload();
  await expect(page.getByRole("heading", { name: "API-ключи", exact: true })).toBeVisible();
  await page.goto("/app/connections");
  await expect(page.getByRole("heading", { name: "Страница не найдена" })).toBeVisible();
  await expect(page.locator(".connection-list")).toHaveCount(0);
  for (const path of ["/app/settings", "/admin/settings", "/preview.html?page=settings"]) {
    await page.goto(path);
    await expect(page.getByRole("heading", { name: "Страница не найдена" })).toBeVisible();
    await expect(page.locator(".settings-page")).toHaveCount(0);
  }
});

test("demo key creation, one-time display, edit and revoke never persist credentials", async ({ page }) => {
  await page.goto("/app/keys");
  await page.getByRole("button", { name: "Создать ключ", exact: true }).click();
  await page.getByLabel("Название", { exact: true }).fill("Учебный клиент");
  await page.getByLabel("Дневной лимит, USD", { exact: true }).fill("5");
  await page.getByRole("form", { name: "Создать ключ", exact: true }).getByRole("button", { name: "Создать ключ", exact: true }).click();
  const created = page.getByRole("region", { name: "Ключ создан", exact: true });
  await expect(created).toBeVisible();
  await expect(created).not.toContainText(/демо|вымышл|макет|учебн/iu);
  await expect(created.locator("code")).toContainText("demo_not_a_real_key_");
  await created.getByRole("button", { name: "Готово", exact: true }).click();
  const card = page.getByRole("row", { name: "Учебный клиент", exact: true });
  await expect(card).toContainText("5 USD");
  await navigate(page, "Модели"); await navigate(page, "API-ключи");
  await expect(card).toBeVisible();
  await expect(page.getByText(/demo_not_a_real_key_/)).toHaveCount(0);
  await card.getByRole("button", { name: "Изменить", exact: true }).click();
  await page.getByLabel("Дневной лимит, USD", { exact: true }).fill("7");
  await page.getByRole("button", { name: "Сохранить", exact: true }).click();
  await expect(card).toContainText("7 USD");
  await card.getByRole("button", { name: "Отозвать", exact: true }).click();
  await card.getByRole("button", { name: "Да, отозвать", exact: true }).click();
  await expect(card).toContainText("Отозван");
  expect(await page.evaluate(() => JSON.stringify({ ...localStorage, ...sessionStorage }))).not.toMatch(/Учебный клиент|demo_not_a_real_key_|demo_customer/);
  await page.reload(); await expect(card).toHaveCount(0);
});

test("Back and Forward preserve the model catalog without dialogs or scroll locks", async ({ page }) => {
  await page.goto("/app/overview");
  await navigate(page, "Модели");
  await expect(page.getByRole("heading", { name: "Модели и цены", exact: true })).toBeVisible();
  await expect(page.locator(".models-page:visible")).toContainText("demo-gpt");
  await expect(page.getByRole("dialog")).toHaveCount(0);
  await page.goBack();
  await expect(page.getByRole("heading", { name: "Обзор", exact: true })).toBeVisible();
  await expect(page.locator("dialog[open]")).toHaveCount(0);
  await expect(page.locator("body")).not.toHaveCSS("overflow", "hidden");
  await page.goForward();
  await expect(page.getByRole("heading", { name: "Модели и цены", exact: true })).toBeVisible();
  await expect(page.locator(".models-page:visible")).toContainText("demo-gpt");
  await expect(page.getByRole("dialog")).toHaveCount(0);
  await expect(page.locator("body")).not.toHaveCSS("overflow", "hidden");
});

test("connection inventory and pool share only the in-memory demo state", async ({ page }) => {
  await page.goto("/admin/connections");
  await page.getByRole("button", { name: "API-провайдеры", exact: true }).click();
  await page.getByRole("button", { name: "Добавить подключение", exact: true }).click();
  await expect(page.getByRole("dialog")).toContainText("ключи провайдеров не принимаются, запросы не отправляются");
  await page.getByLabel("Имя", { exact: true }).fill("Учебный резерв");
  await page.getByRole("textbox", { name: "Адрес API", exact: true }).fill("https://synthetic.example.com/v1");
  const dialog = page.getByRole("dialog");
  await dialog.getByRole("button", { name: "Модели", exact: true }).click();
  await dialog.getByRole("textbox", { name: "ID модели", exact: true }).fill("synthetic-pool-model");
  await dialog.getByRole("button", { name: "Добавить модель", exact: true }).click();
  await page.getByRole("button", { name: "Добавить", exact: true }).click();
  const card = page.getByRole("article", { name: "Учебный резерв", exact: true });
  await expect(card).toHaveAttribute("data-status", "idle");
  await card.getByRole("button", { name: "Добавить в пул", exact: true }).click();
  await navigate(page, "Пул"); await expect(card).toBeVisible();
  await card.getByRole("button", { name: "Убрать из пула", exact: true }).click();
  await expect(card).not.toBeVisible();
  await navigate(page, "Подключения");
  await page.getByRole("button", { name: "API-провайдеры", exact: true }).click();
  await expect(card).toHaveAttribute("data-status", "idle");
  await page.reload();
  await page.getByRole("button", { name: "API-провайдеры", exact: true }).click();
  await expect(card).toHaveCount(0);
});

test("mobile navigation traps focus, dismisses and returns to its trigger", async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto("/app/keys");
  const trigger = page.getByRole("button", { name: "Меню", exact: true });
  await trigger.click();
  const drawer = page.getByRole("dialog", { name: "Навигация", exact: true });
  await expect(drawer).toBeVisible();
  await expect(page.locator(".relay-content")).toHaveAttribute("inert", "");
  await drawer.getByRole("button", { name: "Свернуть меню", exact: true }).focus();
  await page.keyboard.press("Shift+Tab");
  expect(await drawer.evaluate(element => element.contains(document.activeElement))).toBe(true);
  await page.keyboard.press("Escape");
  await expect(drawer).toHaveCount(0);
  await expect(trigger).toBeFocused();
  await expect(page.locator("body")).not.toHaveCSS("overflow", "hidden");
});
