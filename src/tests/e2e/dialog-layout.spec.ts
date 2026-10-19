import type { Locator } from "@playwright/test";
import { expect, test } from "../bun-playwright";
import { installTauriMock } from "./tauri-mock";

async function expectDialogFits(dialog: Locator) {
  await expect(dialog).toBeVisible();
  expect(await dialog.evaluate((element) => {
    const box = element.getBoundingClientRect();
    const body = element.querySelector<HTMLElement>(".relay-dialog-body")!;
    const header = element.querySelector("header")!.getBoundingClientRect();
    const close = element.querySelector("header > button")!.getBoundingClientRect();
    const footer = element.querySelector("footer")?.getBoundingClientRect();
    return {
      contained: box.left >= 0 && box.right <= innerWidth && box.top >= 36 && box.bottom <= innerHeight,
      bodyFits: body.scrollWidth <= body.clientWidth + 1,
      closeFits: close.left >= header.left && close.right <= header.right && close.top >= header.top && close.bottom <= header.bottom,
      footerFits: !footer || footer.bottom <= box.bottom && body.getBoundingClientRect().bottom <= footer.top + 1,
    };
  })).toEqual({ contained: true, bodyFits: true, closeFits: true, footerFits: true });
}

for (const theme of ["light", "dark"] as const) {
  for (const locale of ["en", "ru"] as const) {
    test(`internal dialogs fit compact and desktop windows (${locale}, ${theme})`, async ({ page }) => {
      await installTauriMock(page, { mode: "local", locale, theme, populated: true, proxyCount: 15 });
      await page.goto("/");
      const ru = locale === "ru";
      for (const viewport of [{ width: 1160, height: 760 }, { width: 840, height: 560 }, { width: 390, height: 560 }]) {
        await page.setViewportSize(viewport);
        await page.getByRole("button", { name: ru ? "Подключения" : "Connections", exact: true }).click();
        await page.getByRole("tab", { name: ru ? "Учётные записи" : "Accounts", exact: true }).click();
        await page.getByRole("button", { name: ru ? "Импорт" : "Import", exact: true }).click();
        await expectDialogFits(page.getByRole("dialog"));
        await page.getByRole("dialog").locator("header > button").click();

        await page.locator(".account-card").first().locator(".account-row-menu summary").click();
        await page.getByRole("menuitem", { name: ru ? "Заметки" : "Notes", exact: true }).click();
        await expectDialogFits(page.getByRole("dialog"));
        await page.getByRole("dialog").locator("header > button").click();

        await page.locator(".account-card").first().locator(".account-row-menu summary").click();
        await page.getByRole("menuitem", { name: ru ? /^Прокси:/ : /^Proxy:/ }).click();
        await expectDialogFits(page.getByRole("dialog"));
        await page.getByRole("dialog").locator("header > button").click();

        await page.getByRole("tab", { name: ru ? "Источники API" : "Sources", exact: true }).click();
        await page.getByRole("button", { name: ru ? "Добавить источник" : "Add source", exact: true }).click();
        await expectDialogFits(page.getByRole("dialog"));
        await page.getByRole("dialog").locator("header > button").click();

        await page.getByRole("button", { name: ru ? "Пул" : "Pool", exact: true }).click();
        await page.getByRole("button", { name: ru ? "Правила участника пула: Example compatible API" : "Pool member policy: Example compatible API", exact: true }).click();
        const member = page.getByRole("dialog");
        for (const tab of ru ? ["Модели", "Цены", "Настройки"] : ["Models", "Pricing", "Settings"]) {
          await member.getByRole("tab", { name: tab, exact: true }).click();
          await expectDialogFits(member);
        }
        await member.locator("header > button").click();
        await page.getByRole("button", { name: ru ? "Настройки ротации пула" : "Pool rotation settings", exact: true }).click();
        await expectDialogFits(page.getByRole("dialog"));
        await page.getByRole("dialog").locator("header > button").click();
      }
    });
  }

  test(`long dropdowns stay outside the titlebar and follow dialog scrolling (${theme})`, async ({ page }) => {
    await page.setViewportSize({ width: 390, height: 560 });
    await installTauriMock(page, { mode: "local", locale: "en", theme, populated: true, accountCount: 8, sourceCount: 3, proxyCount: 20 });
    await page.goto("/");
    await page.getByRole("button", { name: "Usage", exact: true }).click();
    const trigger = page.getByRole("button", { name: /^Pool member:/ });
    await trigger.click();
    const list = page.getByRole("listbox");
    await expect(list).toBeVisible();
    const box = (await list.boundingBox())!;
    const anchor = (await trigger.boundingBox())!;
    expect(box.y).toBeGreaterThanOrEqual(44);
    expect(box.y + box.height).toBeLessThanOrEqual(552);
    expect(box.y >= anchor.y + anchor.height || box.y + box.height <= anchor.y).toBe(true);
    await list.evaluate((element) => { element.scrollTop = element.scrollHeight; });
    await expect(list).toBeVisible();
    await page.keyboard.press("Escape");
    await expect(trigger).toBeFocused();

    await page.getByRole("button", { name: "Connections", exact: true }).click();
    await page.getByRole("tab", { name: "Sources", exact: true }).click();
    await page.getByRole("button", { name: "Add source", exact: true }).click();
    const dialog = page.getByRole("dialog");
    await dialog.getByRole("button", { name: /^Proxy:/ }).click();
    await expect(list).toBeVisible();
    const body = dialog.locator(".relay-dialog-body");
    await body.evaluate((element) => { element.scrollTop = 0; });
    await expect(list).toBeHidden();
    await expect(dialog).toBeVisible();
  });

  test(`long source names wrap inside dropdown options (${theme})`, async ({ page }) => {
    await page.setViewportSize({ width: 390, height: 560 });
    await installTauriMock(page, { mode: "local", locale: "en", theme, populated: true });
    await page.goto("/");
    await page.getByRole("button", { name: "Connections", exact: true }).click();
    await page.getByRole("tab", { name: "Sources", exact: true }).click();
    await page.locator(".source-card").first().getByRole("button", { name: "Edit", exact: true }).click();
    const name = `source-${"long".repeat(60)}`;
    await page.getByRole("dialog").getByLabel("Name", { exact: true }).fill(name);
    await page.getByRole("dialog").getByRole("button", { name: "Save", exact: true }).click();
    await expect(page.getByRole("dialog")).toBeHidden();
    await page.getByRole("button", { name: "Usage", exact: true }).click();
    await page.getByRole("button", { name: /^Pool member:/ }).click();
    const list = page.getByRole("listbox");
    await list.getByRole("option", { name, exact: true }).scrollIntoViewIfNeeded();
    expect(await list.evaluate((element) => element.scrollWidth <= element.clientWidth + 1)).toBe(true);
    await list.getByRole("option", { name, exact: true }).click();
    await expect(list).toBeHidden();
  });
}
