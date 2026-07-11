import { expect, test } from "@playwright/test";
import { installTauriMock } from "./tauri-mock";

const modes = ["local", "remote", "zenith"] as const;
const themes = ["light", "dark"] as const;
const locales = ["en", "ru"] as const;
const viewports = [{ width: 1160, height: 760 }, { width: 840, height: 560 }] as const;

for (const locale of locales) {
  for (const mode of modes) {
    for (const theme of themes) {
      for (const viewport of viewports) {
        test(`${locale} ${mode} ${theme} ${viewport.width}x${viewport.height}`, async ({ page }) => {
          await installTauriMock(page, { locale, mode, theme, populated: true });
          await page.setViewportSize(viewport);
          await page.goto("/");
          const nav = page.locator(".relay-sidebar nav button");
          const expectedPages = mode === "zenith" ? 6 : 7;
          await expect(nav).toHaveCount(expectedPages);
          for (let index = 0; index < expectedPages; index += 1) {
            await nav.nth(index).click();
            const settingsSections = page.locator(".settings-layout > nav button");
            const tabControls = page.locator(".relay-tabs [role=tab]");
            const stateControls = await settingsSections.count() ? settingsSections : tabControls;
            const stateCount = Math.max(1, await stateControls.count());

            for (let state = 0; state < stateCount; state += 1) {
              if (await stateControls.count()) await stateControls.nth(state).click();
              await expect(page.locator(".relay-page-header h1")).toBeVisible();
              await expect(page.locator("body")).not.toContainText(/(?:common|nav|overview|connections|pool|gateway|usage|profiles|settings)\.[a-z]/);
              expect(await page.evaluate(() => document.documentElement.scrollWidth <= document.documentElement.clientWidth)).toBe(true);
              expect(await page.evaluate(() => [...document.querySelectorAll<HTMLElement>(".relay-page button, .relay-page input, .relay-page select")].filter((element) => {
                const rect = element.getBoundingClientRect();
                const intersectsViewport = rect.right > 0 && rect.left < innerWidth && rect.bottom > 36 && rect.top < innerHeight;
                return intersectsViewport && (rect.left < 0 || rect.right > innerWidth);
              }).map((element) => element.outerHTML.slice(0, 160)))).toEqual([]);
              expect(await page.evaluate(() => [...document.querySelectorAll<HTMLElement>(".relay-table td.row-actions, [data-page='profiles'] .relay-table tbody td:last-child")].filter((cell) => {
                const wrap = cell.closest<HTMLElement>(".relay-table-wrap");
                if (!wrap) return false;
                const cellRect = cell.getBoundingClientRect();
                const wrapRect = wrap.getBoundingClientRect();
                return cellRect.left < wrapRect.left - 1 || cellRect.right > wrapRect.right + 1;
              }).map((cell) => cell.outerHTML.slice(0, 160)))).toEqual([]);

              const stateSuffix = stateCount > 1 ? `-tab-${state + 1}` : "";
              await page.screenshot({ path: `output/playwright/${locale}-${mode}-${theme}-${viewport.width}x${viewport.height}-page-${index + 1}${stateSuffix}.png` });
              if (state === 0 && stateCount > 1) {
                await page.screenshot({ path: `output/playwright/${locale}-${mode}-${theme}-${viewport.width}x${viewport.height}-page-${index + 1}.png` });
              }
            }
          }
        });
      }
    }
  }
}

for (const viewport of viewports) {
  test(`proxy controls ${viewport.width}x${viewport.height}`, async ({ page }) => {
    await installTauriMock(page, { locale: "en", mode: "local", theme: "light", populated: true });
    await page.setViewportSize(viewport);
    await page.goto("/");
    await page.getByRole("button", { name: "Gateway", exact: true }).click();
    await page.locator(".proxy-settings").scrollIntoViewIfNeeded();
    await expect(page.locator(".proxy-settings")).toBeVisible();
    await page.screenshot({ path: `output/playwright/proxy-common-${viewport.width}x${viewport.height}.png` });

    await page.getByRole("button", { name: "Connections", exact: true }).click();
    await page.getByRole("button", { name: "Common proxy", exact: true }).click();
    await expect(page.getByRole("dialog", { name: "Proxy for Personal Plus" })).toBeVisible();
    await page.screenshot({ path: `output/playwright/proxy-account-${viewport.width}x${viewport.height}.png` });
    await page.getByRole("dialog").getByRole("button", { name: "Close" }).click();
    await page.locator(".account-bulk-menu summary").click();
    await page.getByRole("menuitem", { name: "Assign proxies" }).click();
    await expect(page.getByRole("dialog", { name: "Assign account proxies" })).toBeVisible();
    await page.screenshot({ path: `output/playwright/proxy-bulk-${viewport.width}x${viewport.height}.png` });

    expect(await page.evaluate(() => document.documentElement.scrollWidth <= document.documentElement.clientWidth)).toBe(true);
    expect(await page.evaluate(() => {
      const dialog = document.querySelector<HTMLElement>(".relay-dialog");
      if (!dialog) return false;
      const rect = dialog.getBoundingClientRect();
      return rect.left >= 0 && rect.right <= innerWidth && rect.top >= 0 && rect.bottom <= innerHeight;
    })).toBe(true);
  });

  test(`account export ${viewport.width}x${viewport.height}`, async ({ page }) => {
    await installTauriMock(page, { locale: "en", mode: "local", theme: "light", populated: true });
    await page.setViewportSize(viewport);
    await page.goto("/");
    await page.getByRole("button", { name: "Connections", exact: true }).click();
    await page.locator(".account-bulk-menu summary").click();
    await page.getByRole("menuitem", { name: "Export all" }).click();
    const dialog = page.getByRole("dialog", { name: "Export accounts" });
    await expect(dialog).toBeVisible();
    await expect(dialog.getByRole("button", { name: "Copy JSON" })).toBeVisible();
    await expect(dialog.getByRole("button", { name: "Download JSON" })).toBeVisible();
    await page.screenshot({ path: `output/playwright/account-export-${viewport.width}x${viewport.height}.png` });
    expect(await dialog.evaluate((element) => {
      const rect = element.getBoundingClientRect();
      return rect.left >= 0 && rect.right <= innerWidth && rect.top >= 36 && rect.bottom <= innerHeight;
    })).toBe(true);
  });

  test(`account actions ${viewport.width}x${viewport.height}`, async ({ page }) => {
    await installTauriMock(page, { locale: "en", mode: "local", theme: "dark", populated: true });
    await page.setViewportSize(viewport);
    await page.goto("/");
    await page.getByRole("button", { name: "Connections", exact: true }).click();
    await page.locator(".account-card .account-row-menu summary").click();
    const menu = page.locator(".account-card .account-row-menu [role=menu]");
    await expect(menu).toBeVisible();
    await expect(menu.getByRole("menuitem")).toHaveCount(4);
    await page.screenshot({ path: `output/playwright/account-actions-${viewport.width}x${viewport.height}.png` });
    expect(await menu.evaluate((element) => {
      const rect = element.getBoundingClientRect();
      return rect.left >= 0 && rect.right <= innerWidth && rect.top >= 36 && rect.bottom <= innerHeight;
    })).toBe(true);
  });

  test(`account bulk actions ${viewport.width}x${viewport.height}`, async ({ page }) => {
    await installTauriMock(page, { locale: "en", mode: "local", theme: "light", populated: true });
    await page.setViewportSize(viewport);
    await page.goto("/");
    await page.getByRole("button", { name: "Connections", exact: true }).click();
    await page.locator(".account-bulk-menu summary").click();
    const menu = page.locator(".account-bulk-menu [role=menu]");
    await expect(menu.getByRole("menuitem")).toHaveCount(2);
    await page.screenshot({ path: `output/playwright/account-bulk-actions-${viewport.width}x${viewport.height}.png` });
    expect(await menu.evaluate((element) => {
      const rect = element.getBoundingClientRect();
      return rect.left >= 0 && rect.right <= innerWidth && rect.top >= 36 && rect.bottom <= innerHeight;
    })).toBe(true);
  });

  test(`account selection ru ${viewport.width}x${viewport.height}`, async ({ page }) => {
    await installTauriMock(page, { locale: "ru", mode: "local", theme: "dark", populated: true });
    await page.setViewportSize(viewport);
    await page.goto("/");
    await page.locator(".relay-sidebar nav button").nth(1).click();
    await page.getByLabel("Выбрать Personal Plus").check();
    await expect(page.getByRole("button", { name: "Экспортировать выбранные (1)" })).toBeVisible();
    await expect(page.locator(".account-command-context > span")).toHaveText("Выбрано: 1");
    await page.screenshot({ path: `output/playwright/account-selection-ru-${viewport.width}x${viewport.height}.png` });
    expect(await page.locator(".account-command-bar").evaluate((bar) => {
      const count = bar.firstElementChild?.getBoundingClientRect();
      const actions = bar.lastElementChild?.getBoundingClientRect();
      return Boolean(count && actions && count.right <= actions.left && actions.right <= innerWidth);
    })).toBe(true);
    expect(await page.locator(".account-card").evaluate((card) => {
      const cardRect = card.getBoundingClientRect();
      const actions = card.querySelector<HTMLElement>(".account-row-action-list")?.getBoundingClientRect();
      return Boolean(actions && actions.left >= cardRect.left && actions.right <= cardRect.right && actions.top >= cardRect.top && actions.bottom <= cardRect.bottom);
    })).toBe(true);
  });

  test(`account identity ru ${viewport.width}x${viewport.height}`, async ({ page }) => {
    await installTauriMock(page, { locale: "ru", mode: "local", theme: "dark", populated: true });
    await page.setViewportSize(viewport);
    await page.goto("/");
    await page.locator(".relay-sidebar nav button").nth(1).click();
    await page.getByRole("button", { name: "Показать имя полностью" }).click();
    const identity = page.locator(".account-card").first().locator(".account-identity > strong");
    await expect(identity).toHaveText("person@example.test");
    await expect(page.locator(".account-card").first().getByText("Personal Plus", { exact: true })).toHaveCount(0);
    await expect(page.getByRole("button", { name: "Скрыть полное имя" })).toBeVisible();
    await page.screenshot({ path: `output/playwright/account-identity-ru-${viewport.width}x${viewport.height}.png` });
    expect(await identity.evaluate((element) => element.scrollWidth <= element.clientWidth)).toBe(true);
  });

  test(`multiple accounts ru ${viewport.width}x${viewport.height}`, async ({ page }) => {
    await installTauriMock(page, { locale: "ru", mode: "local", theme: "dark", populated: true, accountCount: 3 });
    await page.setViewportSize(viewport);
    await page.goto("/");
    await page.locator(".relay-sidebar nav button").nth(1).click();
    const cards = page.locator(".account-card");
    await expect(cards).toHaveCount(3);
    await expect(cards.nth(1)).toContainText("Business");
    await expect(cards.nth(1)).toContainText("5 недель");
    await expect(cards.nth(1).locator(".quota-meter")).toHaveCount(1);
    await expect(cards.nth(2).locator(".quota-meter")).toHaveCount(1);
    expect(await cards.evaluateAll((items) => items.every((item) => !item.textContent?.includes("Модели")))).toBe(true);
    await page.screenshot({ path: `output/playwright/multiple-accounts-ru-${viewport.width}x${viewport.height}.png` });
    expect(await cards.evaluateAll((items) => {
      const columns = items.map((item) => [".account-identity", ".account-facts", ".account-row-action-list"].map((selector) => item.querySelector<HTMLElement>(selector)?.getBoundingClientRect().left ?? -1));
      return columns.every((row) => row.every((left, index) => Math.abs(left - columns[0][index]) <= 1));
    })).toBe(true);
    expect(await cards.evaluateAll((items) => items.every((item) => {
      const identity = item.querySelector<HTMLElement>(".account-identity")?.getBoundingClientRect();
      const facts = item.querySelector<HTMLElement>(".account-facts")?.getBoundingClientRect();
      return Boolean(identity && facts && Math.abs(identity.top - facts.top) <= 1 && Math.abs(identity.height - facts.height) <= 1);
    }))).toBe(true);
    expect(await cards.evaluateAll((items) => items.every((item) => {
      const facts = [...item.querySelectorAll<HTMLElement>(".account-facts > div")].map((fact) => fact.getBoundingClientRect().width);
      return facts.length === 2 && Math.abs(facts[0] - facts[1]) <= 1;
    }))).toBe(true);
    expect(await page.locator(".account-facts span").evaluateAll((labels) => labels.every((label) => label.scrollWidth <= label.clientWidth))).toBe(true);
  });

  test(`quota windows ${viewport.width}x${viewport.height}`, async ({ page }) => {
    await installTauriMock(page, { locale: "en", mode: "local", theme: "light", populated: true, supplementalQuota: true });
    await page.setViewportSize(viewport);
    await page.goto("/");
    await page.getByRole("button", { name: "Connections", exact: true }).click();
    const meters = page.locator(".account-list .quota-meter");
    await expect(meters).toHaveCount(5);
    await expect(page.locator(".account-list")).toContainText("5 hours");
    await expect(page.locator(".account-list")).toContainText("Weekly");
    await expect(page.locator(".account-list")).toContainText("Code Review");
    await expect(page.locator(".account-list")).toContainText("GPT-5.4 priority");
    expect(await meters.evaluateAll((items) => items.every((item) => item.scrollWidth <= item.clientWidth))).toBe(true);
    expect(await meters.last().evaluate((item) => {
      const current = item.getBoundingClientRect();
      const first = item.parentElement?.firstElementChild?.getBoundingClientRect();
      return Boolean(first && Math.abs(current.left - first.left) <= 1 && Math.abs(current.width - item.parentElement!.getBoundingClientRect().width) <= 1);
    })).toBe(true);
    await expect(page.locator(".quota-display-menu")).toHaveCount(0);
    await page.screenshot({ path: `output/playwright/quota-windows-${viewport.width}x${viewport.height}.png` });
  });
}
