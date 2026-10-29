import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";

async function navigate(page: Page, name: string) {
  const navigation = page.getByRole("navigation", { name: "Разделы", exact: true });
  if (!await navigation.isVisible()) await page.getByRole("button", { name: "Меню", exact: true }).click();
  await navigation.getByRole("button", { name, exact: true }).click();
}

test.beforeEach(async ({ page }) => {
  await page.goto("/preview.html");
  await expect(page.getByRole("heading", { name: "Обзор", exact: true })).toBeVisible();
});

test("all four screens navigate without overflow or real API calls", async ({ page }) => {
  const errors: string[] = [];
  const external: string[] = [];
  page.on("pageerror", error => errors.push(error.message));
  page.on("request", request => { if (!request.url().startsWith("http://127.0.0.1:1442/")) external.push(request.url()); });
  await expect(page.getByRole("heading", { name: "Обзор", exact: true })).toBeVisible();
  await expect(page.getByRole("img", { name: /Точные значения доступны/ })).toHaveCount(5);
  for (const name of ["Использование", "Модели", "Баланс", "Обзор"]) {
    await navigate(page, name);
    await expect(page.getByRole("heading", { name, exact: true })).toBeVisible();
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  }
  await navigate(page, "Модели");
  await page.getByRole("button", { name: "Подробнее о GPT · демо", exact: true }).click();
  await expect(page.getByRole("dialog")).toContainText("Запись кэша");
  await page.getByRole("button", { name: "Как подключить клиент", exact: true }).click();
  await expect(page.getByRole("dialog")).toContainText("relay.example.test");
  await page.keyboard.press("Escape");
  expect(errors).toEqual([]);
  expect(external).toEqual([]);
});

test("Overview keeps its filters when navigating away without changing the wallet", async ({ page }) => {
  await page.getByRole("combobox", { name: "Модель статистики", exact: true }).click();
  await page.getByRole("option", { name: "GPT · демо", exact: true }).click();
  await page.getByRole("group", { name: "Период статистики" }).getByRole("button", { name: "7 дней", exact: true }).click();
  await navigate(page, "Использование");
  await expect(page.getByRole("combobox", { name: "Модель", exact: true })).toHaveText("Все модели");
  await expect(page.getByRole("combobox", { name: "Период, UTC", exact: true })).toHaveText("Сегодня");
  await expect(page.getByRole("button", { name: "Баланс: 42,80 USD", exact: true })).toBeVisible();
  await page.goBack();
  await expect(page.getByRole("heading", { name: "Обзор", exact: true })).toBeVisible();
  await expect(page.getByRole("combobox", { name: "Модель статистики", exact: true })).toHaveText("GPT · демо");
  await expect(page.getByRole("group", { name: "Период статистики" }).getByRole("button", { name: "7 дней", exact: true })).toHaveAttribute("aria-pressed", "true");
});

test("top-up is an explicit local simulation; Billing never becomes request history", async ({ page }) => {
  await navigate(page, "Баланс");
  const operations = page.getByRole("region", { name: "Платёжные операции", exact: true });
  await expect(operations).not.toContainText(/API-ключ|demo_request_|Токены/);
  await page.getByRole("button", { name: "Пополнить", exact: true }).click();
  await page.getByRole("spinbutton", { name: "Сумма, USD", exact: true }).fill("1e2");
  await page.getByRole("button", { name: "Посмотреть подтверждение", exact: true }).click();
  await expect(page.getByRole("dialog")).toContainText("100,00 USD");
  await page.getByRole("button", { name: "Добавить демо-пополнение", exact: true }).click();
  await expect(page.getByRole("button", { name: "Баланс: 142,80 USD", exact: true })).toBeVisible();
  await expect(operations).toContainText("Демо-пополнение");
  await page.reload();
  await expect(page.getByRole("button", { name: "Баланс: 42,80 USD", exact: true })).toBeVisible();
});

test("notification details remain a clickable demo flow", async ({ page }) => {
  await page.getByRole("button", { name: "Уведомления: одно новое", exact: true }).click();
  await page.getByRole("button", { name: "Открыть запрос", exact: true }).click();
  await expect(page.getByRole("dialog")).toContainText("Ответ завершён, расчёт ожидается");
  await page.keyboard.press("Escape");
  await expect(page.getByRole("button", { name: "Уведомления: нет новых", exact: true })).toBeVisible();
});

test("profile lives at the sidebar bottom and stays reachable after scrolling", async ({ page }) => {
  await page.goto("/preview.html?page=billing");
  const profile = page.getByRole("button", { name: "Открыть профиль", exact: true });
  await expect(page.locator(".topbar").getByRole("button", { name: "Открыть профиль", exact: true })).toHaveCount(0);
  if (!await profile.isVisible()) await page.getByRole("button", { name: "Меню", exact: true }).click();
  await expect(page.getByRole("button", { name: "Открыть профиль", exact: true, includeHidden: true })).toHaveCount(1);
  await expect(page.locator(".sidebar-bottom").getByRole("button", { name: "Открыть профиль", exact: true })).toBeVisible();
  expect(await profile.evaluate(element => {
    const bounds = element.getBoundingClientRect();
    const navigation = document.querySelector(".sidebar nav")!.getBoundingClientRect();
    return bounds.top >= navigation.bottom && bounds.left >= 0 && bounds.right <= innerWidth && bounds.bottom <= innerHeight;
  })).toBe(true);
  if (page.viewportSize()!.width >= 640) {
    expect(await profile.evaluate(element => element.getBoundingClientRect().bottom > innerHeight - 150)).toBe(true);
    await page.goto("/preview.html?page=overview");
    await page.evaluate(() => window.scrollTo(0, document.body.scrollHeight));
    expect(await profile.evaluate(element => {
      const bounds = element.getBoundingClientRect();
      return bounds.top >= 0 && bounds.bottom <= innerHeight && bounds.bottom > innerHeight - 150;
    })).toBe(true);
  }
  await profile.click();
  await expect(page.getByRole("menu", { name: "Профиль", exact: true })).toBeVisible();
  await expect(page.getByRole("menuitem")).toHaveText(["Выйти"]);
  await expect(page.getByRole("dialog")).toHaveCount(0);
  await page.keyboard.press("Escape");
  await expect(profile).toBeFocused();
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
});

test("header places the unread bell between balance and theme; sidebar has no help popup", async ({ page }, testInfo) => {
  await expect(page.locator(".sidebar-footer button")).toHaveCount(0);
  await expect(page.locator(".sidebar").getByRole("button", { name: "Помощь", exact: true, includeHidden: true })).toHaveCount(0);
  await expect(page.locator(".notification-dot, .topbar-divider")).toHaveCount(0);
  const header = page.locator(".topbar");
  const balance = header.getByRole("button", { name: "Баланс: 42,80 USD", exact: true });
  const bell = header.getByRole("button", { name: /^Уведомления:/ });
  const theme = header.getByRole("button", { name: /^(Тёмная|Светлая) тема$/ });
  await expect(balance).toContainText("demo");
  await expect(balance).toContainText("$42,80");
  const balanceBox = (await balance.boundingBox())!;
  const bellBox = (await bell.boundingBox())!;
  const themeBox = (await theme.boundingBox())!;
  expect(balanceBox.x + balanceBox.width).toBeLessThanOrEqual(bellBox.x);
  expect(bellBox.x + bellBox.width).toBeLessThanOrEqual(themeBox.x);
  expect(themeBox.x + themeBox.width).toBeLessThanOrEqual(page.viewportSize()!.width);
  expect(balanceBox.height).toBe(bellBox.height);
  expect(bellBox.height).toBe(themeBox.height);
  await expect(bell).toHaveAttribute("data-unread", "true");
  const unreadColor = await bell.evaluate(element => getComputedStyle(element).color);
  expect(await bell.locator("svg").evaluate(element => getComputedStyle(element).fill)).toBe(unreadColor);
  await header.screenshot({ path: testInfo.outputPath("header-unread.png") });
  await bell.click();
  await expect(page.getByRole("dialog", { name: "Уведомления", exact: true })).toBeVisible();
  await page.getByRole("button", { name: "Отметить прочитанным", exact: true }).click();
  await page.keyboard.press("Escape");
  await expect(bell).toBeFocused();
  await expect(bell).toHaveAttribute("data-unread", "false");
  await expect(bell).toHaveAccessibleName("Уведомления: нет новых");
  expect(await bell.locator("svg").evaluate(element => getComputedStyle(element).fill)).toBe("none");
  expect(await bell.evaluate(element => getComputedStyle(element).color)).not.toBe(unreadColor);
  await header.screenshot({ path: testInfo.outputPath("header-read.png") });
  const scheme = await page.locator(".relay-app").evaluate(element => getComputedStyle(element).colorScheme);
  await theme.click();
  expect(await page.locator(".relay-app").evaluate(element => getComputedStyle(element).colorScheme)).not.toBe(scheme);
  await expect(bell).toHaveAttribute("data-unread", "false");
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
});

test("unread bell only nudges briefly every 45 seconds and respects reduced motion", async ({ page }) => {
  await page.emulateMedia({ reducedMotion: "no-preference" });
  const bell = page.getByRole("button", { name: /^Уведомления:/ });
  const icon = bell.locator("svg");
  await expect(icon).toHaveCSS("animation-name", "notification-nudge");
  await expect(icon).toHaveCSS("animation-duration", "45s");
  const frames = await icon.evaluate(element => {
    const animation = element.getAnimations()[0]!;
    animation.pause();
    return [135, 1000, 10000, 44999, 45135].map(time => {
      animation.currentTime = time;
      return getComputedStyle(element).transform;
    });
  });
  expect(frames[0]).not.toBe(frames[1]);
  expect(frames[1]).toBe("matrix(1, 0, 0, 1, 0, 0)");
  expect(frames[2]).toBe(frames[1]);
  expect(frames[3]).toBe(frames[1]);
  expect(frames[4]).toBe(frames[0]);
  await page.emulateMedia({ reducedMotion: "reduce" });
  await expect(icon).toHaveCSS("animation-name", "none");
  expect(await icon.evaluate(element => element.getAnimations().length)).toBe(0);
  await page.emulateMedia({ reducedMotion: "no-preference" });
  await expect(icon).toHaveCSS("animation-name", "notification-nudge");
  await bell.click();
  await page.getByRole("button", { name: "Отметить прочитанным", exact: true }).click();
  await page.keyboard.press("Escape");
  await expect(bell).toHaveAttribute("data-unread", "false");
  await expect(icon).toHaveCSS("animation-name", "none");
  expect(await icon.evaluate(element => element.getAnimations().length)).toBe(0);
});

test("profile only offers sign-out; confirmation can be cancelled and demo exit is explicit", async ({ page }, testInfo) => {
  await page.goto("/preview.html?page=billing");
  const profile = page.getByRole("button", { name: "Открыть профиль", exact: true });
  if (!await profile.isVisible()) await page.getByRole("button", { name: "Меню", exact: true }).click();
  await profile.focus();
  await profile.press("ArrowUp");
  const menu = page.getByRole("menu", { name: "Профиль", exact: true });
  const signOut = menu.getByRole("menuitem", { name: "Выйти", exact: true });
  await expect(signOut).toBeFocused();
  await expect(menu.getByRole("menuitem")).toHaveCount(1);
  await expect(page.getByRole("dialog")).toHaveCount(0);
  expect(await menu.evaluate(element => {
    const bounds = element.getBoundingClientRect();
    return bounds.top >= 0 && bounds.left >= 0 && bounds.right <= innerWidth && bounds.bottom <= innerHeight;
  })).toBe(true);
  await page.screenshot({ path: testInfo.outputPath("profile-sign-out-menu.png") });
  await page.keyboard.press("Escape");
  await expect(menu).not.toBeVisible();
  await expect(profile).toBeFocused();
  await profile.click();
  await page.getByRole("heading", { name: "Баланс", exact: true }).click();
  await expect(menu).not.toBeVisible();
  await profile.click();
  await signOut.click();
  const confirmation = page.getByRole("dialog", { name: "Выйти из аккаунта?", exact: true });
  await expect(confirmation).toBeVisible();
  await expect(confirmation).toContainText("потребуется снова войти");
  await expect(confirmation.getByRole("button", { name: "Остаться", exact: true })).toBeFocused();
  expect((await confirmation.boundingBox())!.width).toBeLessThanOrEqual(420);
  await page.screenshot({ path: testInfo.outputPath("sign-out-confirmation.png") });
  await confirmation.getByRole("button", { name: "Остаться", exact: true }).click();
  await expect(confirmation).toHaveCount(0);
  await expect(profile).toBeFocused();
  await expect(page.getByRole("button", { name: "Баланс: 42,80 USD", exact: true })).toBeVisible();
  await profile.click();
  await signOut.click();
  await page.keyboard.press("Escape");
  await expect(confirmation).toHaveCount(0);
  await expect(profile).toBeFocused();
  await profile.click();
  await signOut.click();
  await confirmation.getByRole("button", { name: "Выйти", exact: true }).click();
  await expect(page.getByRole("heading", { name: "Вы вышли из демо", exact: true })).toBeVisible();
  await expect(page.locator(".app-shell")).toHaveCount(0);
  await page.getByRole("button", { name: "Вернуться в демо", exact: true }).click();
  await expect(page.getByRole("heading", { name: "Баланс", exact: true })).toBeVisible();
  await expect(page.getByRole("button", { name: "Открыть профиль", exact: true, includeHidden: true })).toHaveCount(1);
});
