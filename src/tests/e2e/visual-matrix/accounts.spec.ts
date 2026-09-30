import { expect, test } from "../../bun-playwright";
import { installTauriMock } from "../tauri-mock";
import { modes, themes, locales, viewports } from "./fixtures";

test("connection account actions use full-width zones and centered dates", async ({ page }) => {
  await installTauriMock(page, { locale: "ru", mode: "local", theme: "dark", populated: true, accountCount: 6, quotaAvailable: true });
  await page.setViewportSize({ width: 1648, height: 1168 });
  await page.goto("/");
  await page.getByRole("button", { name: "Подключения", exact: true }).click();

  const card = page.locator(".account-card").first();
  const actions = card.locator(".account-card-actions .relay-icon-button");
  const summary = page.locator(".connections-account-summary > div");
  await expect(summary).toHaveCount(4);
  await expect(page.locator(".connections-account-controls")).toBeVisible();
  expect(await summary.evaluateAll((items) => items.every((item) => {
    const value = item.querySelector("strong")!.getBoundingClientRect();
    const label = item.querySelector("span")!.getBoundingClientRect();
    return label.left >= value.right;
  }))).toBe(true);
  await expect(actions).toHaveCount(3);
  const [cardBox, dateBox, actionBoxes] = await Promise.all([
    card.boundingBox(),
    card.locator(".account-subscription-line").boundingBox(),
    actions.evaluateAll((items) => items.map((item) => item.getBoundingClientRect().toJSON())),
  ]);
  expect(cardBox).not.toBeNull();
  expect(dateBox).not.toBeNull();
  expect(Math.abs(dateBox!.x + dateBox!.width / 2 - (cardBox!.x + cardBox!.width / 2))).toBeLessThanOrEqual(1);
  expect(Math.max(...actionBoxes.map((box) => box.width)) - Math.min(...actionBoxes.map((box) => box.width))).toBeLessThanOrEqual(2);
  expect(Math.abs(actionBoxes[0].x - cardBox!.x)).toBeLessThanOrEqual(1);
  expect(Math.abs(actionBoxes.at(-1)!.x + actionBoxes.at(-1)!.width - (cardBox!.x + cardBox!.width))).toBeLessThanOrEqual(1);

  await actions.first().hover();
  const dangerHover = await actions.first().evaluate((button) => ({
    background: getComputedStyle(button).backgroundColor,
    buttonColor: getComputedStyle(button).color,
    iconColor: getComputedStyle(button.querySelector("svg")!).color,
  }));
  expect(dangerHover.background).toBe("rgba(0, 0, 0, 0)");
  expect(dangerHover.iconColor).not.toBe(dangerHover.buttonColor);

  await actions.nth(1).click();
  await expect.poll(() => page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: { accountId?: string } }> }).__TAURI_TEST_INVOKES__.some((call) => call.command === "refresh_local_account_quota" && call.args.accountId === "account_synthetic"))).toBe(true);
  await page.mouse.move(1200, 1000);
  await page.waitForTimeout(180);
  await page.screenshot({ path: "output/playwright/connections-header-ru-dark.png", clip: { x: 0, y: 0, width: 1648, height: 300 } });
  await page.screenshot({ path: "output/playwright/connections-account-actions-ru-dark-1648x1168.png" });
});

test("pool account actions match connection cards", async ({ page }) => {
  await installTauriMock(page, { locale: "ru", mode: "local", theme: "dark", populated: true, accountCount: 6, quotaAvailable: true, poolMembers: true });
  await page.setViewportSize({ width: 1648, height: 1168 });
  await page.goto("/");
  await page.getByRole("button", { name: "Пул", exact: true }).click();

  const card = page.locator('[data-member-label="Personal Plus"]');
  const actions = card.locator(".pool-member-actions .relay-icon-button");
  await expect(actions).toHaveCount(3);
  expect(await actions.evaluateAll((items) => items.map((item) => item.getAttribute("aria-label")))).toEqual([
    "Убрать из пула: Personal Plus",
    "Обновить",
    "Правила участника пула: Personal Plus",
  ]);
  const widths = await actions.evaluateAll((items) => items.map((item) => item.getBoundingClientRect().width));
  expect(Math.max(...widths) - Math.min(...widths)).toBeLessThanOrEqual(2);
  const [cardBox, dateBox] = await Promise.all([card.boundingBox(), card.locator(".account-subscription-line").boundingBox()]);
  expect(cardBox).not.toBeNull();
  expect(dateBox).not.toBeNull();
  expect(cardBox!.width).toBeLessThanOrEqual(360);
  expect(Math.abs(dateBox!.x + dateBox!.width / 2 - (cardBox!.x + cardBox!.width / 2))).toBeLessThanOrEqual(1);
  await actions.first().hover();
  const dangerHover = await actions.first().evaluate((button) => ({
    background: getComputedStyle(button).backgroundColor,
    buttonColor: getComputedStyle(button).color,
    iconColor: getComputedStyle(button.querySelector("svg")!).color,
  }));
  expect(dangerHover.background).toBe("rgba(0, 0, 0, 0)");
  expect(dangerHover.iconColor).not.toBe(dangerHover.buttonColor);
  await page.mouse.move(1200, 1000);
  await page.screenshot({ path: "output/playwright/pool-header-ru-dark.png", clip: { x: 0, y: 0, width: 1648, height: 300 } });
  await page.screenshot({ path: "output/playwright/pool-account-actions-ru-dark-1648x1168.png" });
});

for (const locale of locales) {
  for (const mode of modes) {
    for (const theme of themes) {
      for (const viewport of viewports) {
    test(`${locale} ${mode} account import ${theme} ${viewport.width}x${viewport.height}`, async ({ page }) => {
      await installTauriMock(page, { locale: "ru", mode: "local", theme, populated: true, importPreviewDelayMs: 500, importDescription: "## Состав пакета\n\n- Два Business-аккаунта\n- Подписка активна до августа" });
      await page.setViewportSize(viewport);
      await page.goto("/");
      await page.getByRole("button", { name: "Подключения", exact: true }).click();
      await page.getByRole("button", { name: "Импорт", exact: true }).click();

      const dialog = page.getByRole("dialog", { name: "Импортировать учётные записи" });
      await expect(dialog).toBeVisible();
      expect(await dialog.evaluate((element) => {
        const rect = element.getBoundingClientRect();
        return rect.left >= 0 && rect.right <= innerWidth && rect.top >= 36 && rect.bottom <= innerHeight;
      })).toBe(true);
      expect(await dialog.locator("button span").evaluateAll((items) => items.every((item) => item.scrollWidth <= item.clientWidth))).toBe(true);
      await page.screenshot({ path: `output/playwright/account-import-empty-ru-${theme}-${viewport.width}x${viewport.height}.png` });

      await dialog.getByRole("button", { name: "Выбрать файлы аккаунтов" }).click();
      await expect(dialog.getByText("Подготавливаем импорт", { exact: true })).toBeVisible();
      await page.screenshot({ path: `output/playwright/account-import-loading-ru-${theme}-${viewport.width}x${viewport.height}.png` });
      await expect(dialog.getByLabel("Выбрать Imported account для импорта")).toBeChecked();
      await expect(dialog.getByLabel("Выбрать Second imported account для импорта")).toBeChecked();
      await expect(dialog.getByLabel("Выбрать все записи")).toHaveJSProperty("indeterminate", true);
      await expect(dialog.getByText("Описание пакета", { exact: true })).toBeVisible();
      await expect(dialog.getByRole("heading", { name: "Состав пакета" })).toBeVisible();
      await expect(dialog.locator('.account-plan-badge[data-plan="k12"]')).toHaveCount(3);
      expect(await dialog.locator(".relay-dialog-body").evaluate((body) => {
        const preview = body.querySelector<HTMLElement>(".import-preview")!;
        const list = preview.querySelector<HTMLElement>(".import-account-list")!;
        return {
          body: body.scrollWidth - body.clientWidth,
          preview: preview.scrollWidth - preview.clientWidth,
          list: list.scrollWidth - list.clientWidth,
        };
      })).toEqual({ body: 0, preview: 0, list: 0 });
      await page.screenshot({ path: `output/playwright/account-import-preview-ru-${theme}-${viewport.width}x${viewport.height}.png` });
    });
  }
}
}
}
