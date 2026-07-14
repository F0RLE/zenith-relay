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
              expect(await page.locator(".relay-page-actions .relay-button.primary").count()).toBeLessThanOrEqual(1);
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
              expect(await page.evaluate(() => [...document.querySelectorAll<HTMLElement>(".metric-band > div, .pool-summary > div")].every((cell) => {
                const children = [...cell.children] as HTMLElement[];
                if (!children.length) return true;
                const cellRect = cell.getBoundingClientRect();
                const centerX = (cellRect.left + cellRect.right) / 2;
                const centerY = (cellRect.top + cellRect.bottom) / 2;
                const first = children[0].getBoundingClientRect();
                const last = children[children.length - 1].getBoundingClientRect();
                return Math.abs((first.left + last.right) / 2 - centerX) <= 1
                  && children.every((child) => {
                    const rect = child.getBoundingClientRect();
                    return Math.abs((rect.top + rect.bottom) / 2 - centerY) <= 1;
                  });
              }))).toBe(true);
              expect(await page.evaluate(() => [...document.querySelectorAll<HTMLElement>(".model-rules header h2")].every((heading) => heading.scrollHeight <= 21))).toBe(true);
              expect(await page.evaluate(() => [...document.querySelectorAll<HTMLElement>(".relay-page-header p")].every((subtitle) => {
                const header = subtitle.closest<HTMLElement>(".relay-page-header")?.getBoundingClientRect();
                return Boolean(header && header.bottom - subtitle.getBoundingClientRect().bottom >= 8);
              }))).toBe(true);
              expect(await page.evaluate(() => [...document.querySelectorAll<HTMLElement>(".history-repair > .inline-actions")].every((actions) => {
                const field = actions.previousElementSibling?.getBoundingClientRect();
                return Boolean(field && actions.getBoundingClientRect().top - field.bottom >= 8);
              }))).toBe(true);

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

for (const theme of themes) {
  for (const viewport of viewports) {
    test(`account import ${theme} ${viewport.width}x${viewport.height}`, async ({ page }) => {
      await installTauriMock(page, { locale: "ru", mode: "local", theme, populated: true });
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

      await dialog.getByRole("button", { name: "Выбрать JSON-файлы" }).click();
      await expect(dialog.getByLabel("Выбрать Imported account для импорта")).toBeChecked();
      await expect(dialog.getByLabel("Выбрать Second imported account для импорта")).toBeChecked();
      expect(await dialog.locator(".relay-dialog-body").evaluate((body) => {
        const preview = body.querySelector<HTMLElement>(".import-preview")!;
        const table = preview.querySelector<HTMLElement>(".relay-table")!;
        return {
          body: body.scrollWidth - body.clientWidth,
          preview: preview.scrollWidth - preview.clientWidth,
          table: table.scrollWidth - table.clientWidth,
        };
      })).toEqual({ body: 0, preview: 0, table: 0 });
      await page.screenshot({ path: `output/playwright/account-import-preview-ru-${theme}-${viewport.width}x${viewport.height}.png` });
    });
  }
}

test("disabled model state stays readable in the compact dark window", async ({ page }) => {
  await installTauriMock(page, { locale: "ru", mode: "local", theme: "dark", populated: true });
  await page.setViewportSize({ width: 840, height: 560 });
  await page.goto("/");
  await page.getByRole("button", { name: "Пул", exact: true }).click();
  await page.getByRole("tab", { name: "Правила моделей" }).click();
  const model = page.locator('.model-rules li[data-model-id="gpt-5.4-mini"]');
  await model.getByRole("button", { name: "Отключить gpt-5.4-mini" }).click();
  await expect(model).toHaveAttribute("data-enabled", "false");
  await expect(model).toContainText("Отключена");
  expect(await page.locator(".model-rules li").evaluateAll((items) => items.every((item) => item.scrollWidth <= item.clientWidth))).toBe(true);
  expect(await page.locator(".model-sort-select").evaluate((element) => {
    const rect = element.getBoundingClientRect();
    return rect.left >= 0 && rect.right <= innerWidth && rect.top >= 36 && rect.bottom <= innerHeight;
  })).toBe(true);
  await page.screenshot({ path: "output/playwright/model-rules-disabled-ru-dark-840x560.png" });
});

for (const viewport of viewports) {
  test(`empty pool and quota policy ${viewport.width}x${viewport.height}`, async ({ page }) => {
    await installTauriMock(page, { locale: "ru", mode: "local", theme: "dark", populated: true, accountCount: 4, poolMembers: false, gatewayRunning: false });
    await page.setViewportSize(viewport);
    await page.goto("/");
    await page.getByRole("button", { name: "Пул", exact: true }).click();
    await expect(page.getByText("В пуле нет участников", { exact: true })).toBeVisible();
    await page.screenshot({ path: `output/playwright/pool-empty-ru-dark-${viewport.width}x${viewport.height}.png` });

    await page.getByRole("button", { name: "Добавить участника", exact: true }).first().click();
    let dialog = page.getByRole("dialog", { name: "Добавить подключения в пул" });
    await expect(dialog).toBeVisible();
    await page.screenshot({ path: `output/playwright/pool-add-members-ru-dark-${viewport.width}x${viewport.height}.png` });
    const accountSearch = dialog.getByLabel("Найти учётную запись");
    await accountSearch.fill("pro");
    await expect(dialog.locator(".pool-member-options > label").first()).toContainText("Pro account");
    await expect(dialog.locator(".pool-member-options em")).toHaveText("Pro");
    await page.screenshot({ path: `output/playwright/pool-add-pro-ru-dark-${viewport.width}x${viewport.height}.png` });
    await accountSearch.fill("");
    await dialog.getByText("Business Workspace", { exact: true }).click();
    await dialog.getByRole("button", { name: "Добавить выбранные (1)" }).click();

    const poolToolbar = page.locator(".pool-member-toolbar");
    await expect(poolToolbar).toBeVisible();
    expect(await poolToolbar.evaluate((toolbar) => {
      const tabs = document.querySelector<HTMLElement>(".relay-tabs");
      const select = toolbar.querySelector<HTMLElement>("select");
      if (!tabs || !select) return false;
      return select.getBoundingClientRect().top - tabs.getBoundingClientRect().bottom >= 9;
    })).toBe(true);
    expect(await page.locator(".pool-summary > div").evaluateAll((cells) => cells.every((cell) => {
      const cellRect = cell.getBoundingClientRect();
      const center = (cellRect.top + cellRect.bottom) / 2;
      return Array.from(cell.children).every((child) => {
        const rect = child.getBoundingClientRect();
        return Math.abs((rect.top + rect.bottom) / 2 - center) <= 1;
      });
    }))).toBe(true);
    await page.screenshot({ path: `output/playwright/pool-members-ru-dark-${viewport.width}x${viewport.height}.png` });

    await page.getByRole("button", { name: "Настройки обновления квот", exact: true }).click();
    dialog = page.getByRole("dialog", { name: "Обновление квот" });
    await expect(dialog.getByLabel("Интервал фонового обновления")).toHaveValue("300");
    await expect(dialog.getByLabel("Таймаут запроса")).toHaveValue("20");
    await page.screenshot({ path: `output/playwright/pool-quota-policy-ru-dark-${viewport.width}x${viewport.height}.png` });
    expect(await dialog.evaluate((element) => {
      const rect = element.getBoundingClientRect();
      return rect.left >= 0 && rect.right <= innerWidth && rect.top >= 36 && rect.bottom <= innerHeight;
    })).toBe(true);
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= document.documentElement.clientWidth)).toBe(true);
  });

  test(`pool member layouts ${viewport.width}x${viewport.height}`, async ({ page }) => {
    await installTauriMock(page, { locale: "ru", mode: "local", theme: "dark", populated: true, accountCount: 4 });
    await page.setViewportSize(viewport);
    await page.goto("/");
    await page.getByRole("button", { name: "Пул", exact: true }).click();
    const members = page.locator(".pool-member-list");
    await expect(members.locator(".pool-member-card")).toHaveCount(5);
    await expect(page.locator(".pool-summary > div")).toHaveCount(3);
    await expect(members).toHaveAttribute("data-layout", "list");
    await expect(members).toContainText("Pro account · Pro");
    await expect(members).toContainText("API-экв.");
    await expect(members).not.toContainText("Доля");
    const listHeight = await members.locator(".pool-member-card").first().evaluate((item) => item.getBoundingClientRect().height);
    await page.screenshot({ path: `output/playwright/pool-layout-list-${viewport.width}x${viewport.height}.png` });

    await page.getByRole("button", { name: "Компактный вид пула" }).click();
    await expect(members).toHaveAttribute("data-layout", "compact");
    const compactHeight = await members.locator(".pool-member-card").first().evaluate((item) => item.getBoundingClientRect().height);
    expect(compactHeight).toBeLessThan(listHeight);
    await page.screenshot({ path: `output/playwright/pool-layout-compact-${viewport.width}x${viewport.height}.png` });

    await page.getByRole("button", { name: "Сетка участников пула" }).click();
    await expect(members).toHaveAttribute("data-layout", "grid");
    const gridHeight = await members.locator(".pool-member-card").first().evaluate((item) => item.getBoundingClientRect().height);
    expect(gridHeight).toBeGreaterThan(compactHeight);
    await page.screenshot({ path: `output/playwright/pool-layout-grid-${viewport.width}x${viewport.height}.png` });
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= document.documentElement.clientWidth)).toBe(true);
    expect(await members.locator(".pool-member-card-main").evaluateAll((items) => items.every((item) => item.scrollWidth <= item.clientWidth))).toBe(true);
    expect(await members.locator(".quota-meter-heading small").evaluateAll((items) => items.every((item) => item.scrollWidth <= item.clientWidth))).toBe(true);
  });

  test(`pool member layouts en light ${viewport.width}x${viewport.height}`, async ({ page }) => {
    await installTauriMock(page, { locale: "en", mode: "local", theme: "light", populated: true, accountCount: 4 });
    await page.setViewportSize(viewport);
    await page.goto("/");
    await page.getByRole("button", { name: "Pool", exact: true }).click();
    const members = page.locator(".pool-member-list");

    await expect(members).toHaveAttribute("data-layout", "list");
    await expect(members).toContainText("Pro account · Pro");
    await expect(members).toContainText("API equiv.");
    await page.screenshot({ path: `output/playwright/pool-layout-list-en-light-${viewport.width}x${viewport.height}.png` });

    await page.getByRole("button", { name: "Compact pool view" }).click();
    await expect(members).toHaveAttribute("data-layout", "compact");
    await page.screenshot({ path: `output/playwright/pool-layout-compact-en-light-${viewport.width}x${viewport.height}.png` });

    await page.getByRole("button", { name: "Pool card grid" }).click();
    await expect(members).toHaveAttribute("data-layout", "grid");
    await page.screenshot({ path: `output/playwright/pool-layout-grid-en-light-${viewport.width}x${viewport.height}.png` });
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= document.documentElement.clientWidth)).toBe(true);
    expect(await members.locator(".pool-member-card-main").evaluateAll((items) => items.every((item) => item.scrollWidth <= item.clientWidth))).toBe(true);
  });

  test(`proxy controls ${viewport.width}x${viewport.height}`, async ({ page }) => {
    await installTauriMock(page, { locale: "en", mode: "local", theme: "light", populated: true });
    await page.setViewportSize(viewport);
    await page.goto("/");
    await page.getByRole("button", { name: "Gateway", exact: true }).click();
    await page.locator(".proxy-settings").scrollIntoViewIfNeeded();
    await expect(page.locator(".proxy-settings")).toBeVisible();
    await page.screenshot({ path: `output/playwright/proxy-common-${viewport.width}x${viewport.height}.png` });

    await page.getByRole("button", { name: "Connections", exact: true }).click();
    await page.getByRole("button", { name: "Common", exact: true }).click();
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
    await expect(menu.getByRole("menuitem")).toHaveText(["Export", "Disable", "Delete"]);
    await page.screenshot({ path: `output/playwright/account-actions-${viewport.width}x${viewport.height}.png` });
    expect(await menu.evaluate((element) => {
      const rect = element.getBoundingClientRect();
      return rect.left >= 0 && rect.right <= innerWidth && rect.top >= 36 && rect.bottom <= innerHeight;
    })).toBe(true);
  });

  test(`account error details ${viewport.width}x${viewport.height}`, async ({ page }) => {
    await installTauriMock(page, { locale: "ru", mode: "local", theme: "dark", populated: true, accountCount: 3 });
    await page.setViewportSize(viewport);
    await page.goto("/");
    await page.getByRole("button", { name: "Подключения", exact: true }).click();
    await page.getByRole("group", { name: "Фильтр по подписке" }).getByRole("button", { name: "С ошибками (1)" }).click();
    await page.locator(".account-error-line").click();
    const dialog = page.getByRole("dialog", { name: "Технические детали ошибки" });
    await expect(dialog.locator("pre")).toContainText('"code": "quota_transport"');
    await page.screenshot({ path: `output/playwright/account-error-details-${viewport.width}x${viewport.height}.png` });
    expect(await dialog.evaluate((element) => {
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

  for (const theme of themes) {
    test(`icon tooltip ${theme} ${viewport.width}x${viewport.height}`, async ({ page }) => {
      await installTauriMock(page, { locale: "ru", mode: "local", theme, populated: true, accountCount: 3 });
      await page.setViewportSize(viewport);
      await page.goto("/");
      await page.getByRole("button", { name: "Подключения", exact: true }).click();
      await page.getByRole("button", { name: "Обновить и удалить нерабочие записи" }).hover();
      const tooltip = page.getByRole("tooltip");
      await expect(tooltip).toBeVisible();
      await page.screenshot({ path: `output/playwright/icon-tooltip-ru-${theme}-${viewport.width}x${viewport.height}.png` });
      expect(await tooltip.evaluate((element) => {
        const rect = element.getBoundingClientRect();
        return rect.left >= 8 && rect.right <= innerWidth - 8 && rect.top >= 36 && rect.bottom <= innerHeight;
      })).toBe(true);
    });
  }

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
      const count = bar.querySelector<HTMLElement>(".account-command-context > span")?.getBoundingClientRect();
      const actions = bar.lastElementChild?.getBoundingClientRect();
      const bounds = bar.getBoundingClientRect();
      const separated = count && actions && (count.right <= actions.left || actions.right <= count.left || count.bottom <= actions.top || actions.bottom <= count.top);
      const verticallyAligned = count && actions && Math.abs(count.top + count.height / 2 - actions.top - actions.height / 2) <= 1;
      return Boolean(separated && verticallyAligned && count.left >= bounds.left && count.right <= bounds.right && actions.left >= bounds.left && actions.right <= bounds.right);
    })).toBe(true);
    expect(await page.locator(".account-card").evaluate((card) => {
      const cardRect = card.getBoundingClientRect();
      const actions = card.querySelector<HTMLElement>(".account-row-action-list")?.getBoundingClientRect();
      return Boolean(actions && actions.left >= cardRect.left && actions.right <= cardRect.right && actions.top >= cardRect.top && actions.bottom <= cardRect.bottom);
    })).toBe(true);
    expect(await page.locator(".account-card-main").evaluate((main) => getComputedStyle(main).backgroundColor)).toBe("rgba(0, 0, 0, 0)");
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
    await expect(page.getByRole("group", { name: "Фильтр по подписке" }).getByRole("button")).toHaveCount(5);
    const business = cards.filter({ has: page.getByText("Business Workspace", { exact: true }) });
    const backup = cards.filter({ has: page.getByText("Backup account", { exact: true }) });
    await expect(business).toContainText("Business");
    await expect(business).toContainText("5 недель");
    await expect(business.locator(".quota-meter")).toHaveCount(1);
    await expect(backup.locator(".quota-meter")).toHaveCount(1);
    await expect(backup.locator(".account-error-line")).toContainText("Ошибка подключения");
    await expect(backup.locator(".account-error-line code")).toHaveText("quota_transport");
    await expect(business.locator(".account-subscription-line")).toContainText("Действует до");
    await expect(business.locator(".account-subscription-countdown")).toHaveText(/через \d+/);
    await expect(backup.locator(".account-subscription-line")).toHaveText("Дата окончания подписки не указана");
    expect(await cards.evaluateAll((items) => items.every((item) => !item.textContent?.includes("Модели")))).toBe(true);
    await page.screenshot({ path: `output/playwright/multiple-accounts-ru-${viewport.width}x${viewport.height}.png` });
    expect(await cards.evaluateAll((items) => {
      const columns = items.map((item) => [".account-identity", ".account-facts", ".account-row-action-list"].map((selector) => item.querySelector<HTMLElement>(selector)?.getBoundingClientRect().left ?? -1));
      return columns.every((row) => row.every((left, index) => Math.abs(left - columns[0][index]) <= 1));
    })).toBe(true);
    expect(await cards.evaluateAll((items, narrow) => items.every((item) => {
      const identity = item.querySelector<HTMLElement>(".account-identity")?.getBoundingClientRect();
      const facts = item.querySelector<HTMLElement>(".account-facts")?.getBoundingClientRect();
      if (!identity || !facts) return false;
      return narrow
        ? facts.top >= identity.bottom
        : Math.abs(identity.top - facts.top) <= 1 && Math.abs(identity.height - facts.height) <= 1;
    }), viewport.width <= 1050)).toBe(true);
    expect(await page.locator(".account-plan-filters").evaluateAll((items) => items.every((element) => element.scrollWidth >= element.clientWidth))).toBe(true);
    expect(await page.locator(".account-error-line").evaluate((element) => element.scrollWidth <= element.clientWidth)).toBe(true);
    expect(await page.locator(".account-subscription-line").evaluateAll((items) => items.every((item) => item.scrollWidth <= item.clientWidth))).toBe(true);
    expect(await cards.evaluateAll((items) => {
      const rows = items.map((item) => [...item.querySelectorAll<HTMLElement>(".account-facts > div")].map((fact) => fact.getBoundingClientRect().width));
      return rows.every((row) => row.length === 3 && row.every((width, index) => Math.abs(width - rows[0][index]) <= 1));
    })).toBe(true);
    const overflowingLabels = await page.locator(".account-facts span").evaluateAll((labels) => labels.flatMap((label) => {
      if (label.scrollWidth <= label.clientWidth) return [];
      const style = getComputedStyle(label);
      return [{
        text: label.textContent?.trim() ?? "",
        className: label.className,
        clientWidth: label.clientWidth,
        scrollWidth: label.scrollWidth,
        renderedWidth: label.getBoundingClientRect().width,
        fontFamily: style.fontFamily,
        fontSize: style.fontSize,
      }];
    }));
    expect(overflowingLabels).toEqual([]);
    await expect(page.locator(".proxy-status-button > svg")).toHaveCount(0);

    await page.getByRole("button", { name: "Компактный вид учётных записей" }).click();
    await expect(page.locator(".account-list")).toHaveAttribute("data-layout", "compact");
    await page.screenshot({ path: `output/playwright/account-layout-compact-ru-dark-${viewport.width}x${viewport.height}.png` });
    await page.getByRole("button", { name: "Сетка учётных записей" }).click();
    await expect(page.locator(".account-list")).toHaveAttribute("data-layout", "grid");
    await page.screenshot({ path: `output/playwright/account-layout-grid-ru-dark-${viewport.width}x${viewport.height}.png` });
    expect(await cards.locator(".account-card-main").evaluateAll((items) => items.every((item) => item.scrollWidth <= item.clientWidth))).toBe(true);
  });

  test(`account layouts en light ${viewport.width}x${viewport.height}`, async ({ page }) => {
    await installTauriMock(page, { locale: "en", mode: "local", theme: "light", populated: true, accountCount: 4 });
    await page.setViewportSize(viewport);
    await page.goto("/");
    await page.getByRole("button", { name: "Connections", exact: true }).click();
    const accounts = page.locator(".account-list");
    await expect(accounts.locator(".account-card")).toHaveCount(4);

    await expect(accounts).toHaveAttribute("data-layout", "list");
    await expect(accounts).toContainText("Pro account");
    const listHeight = await accounts.locator(".account-card").first().evaluate((item) => item.getBoundingClientRect().height);
    await page.screenshot({ path: `output/playwright/account-layout-list-en-light-${viewport.width}x${viewport.height}.png` });

    await page.getByRole("button", { name: "Compact account view" }).click();
    await expect(accounts).toHaveAttribute("data-layout", "compact");
    const compactHeight = await accounts.locator(".account-card").first().evaluate((item) => item.getBoundingClientRect().height);
    expect(compactHeight).toBeLessThan(listHeight);
    await page.screenshot({ path: `output/playwright/account-layout-compact-en-light-${viewport.width}x${viewport.height}.png` });

    await page.getByRole("button", { name: "Account card grid" }).click();
    await expect(accounts).toHaveAttribute("data-layout", "grid");
    const gridHeight = await accounts.locator(".account-card").first().evaluate((item) => item.getBoundingClientRect().height);
    expect(gridHeight).toBeGreaterThan(compactHeight);
    if (viewport.width > 1023) {
      expect(await accounts.locator(".account-card").evaluateAll((items) => Math.abs(items[0].getBoundingClientRect().height - items[1].getBoundingClientRect().height))).toBeLessThanOrEqual(1);
    }
    await page.screenshot({ path: `output/playwright/account-layout-grid-en-light-${viewport.width}x${viewport.height}.png` });
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= document.documentElement.clientWidth)).toBe(true);
    expect(await accounts.locator(".account-card-main").evaluateAll((items) => items.every((item) => item.scrollWidth <= item.clientWidth))).toBe(true);
    expect(await accounts.locator(".quota-meter-heading small").evaluateAll((items) => items.every((item) => item.scrollWidth <= item.clientWidth))).toBe(true);
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

  test(`routing distribution ru dark ${viewport.width}x${viewport.height}`, async ({ page }) => {
    await installTauriMock(page, { locale: "ru", mode: "local", theme: "dark", populated: true, accountCount: 3 });
    await page.setViewportSize(viewport);
    await page.goto("/");
    await page.getByRole("button", { name: "Пул", exact: true }).click();
    const header = page.locator(".relay-page-header");
    await header.locator(".relay-action-menu summary").click();
    await header.getByRole("menuitem", { name: "Настройки распределения", exact: true }).click();
    const dialog = page.getByRole("dialog", { name: "Распределение запросов" });
    await dialog.getByLabel("Закреплять чат за аккаунтом").uncheck();
    await expect(dialog.getByLabel("Срок закрепления")).toBeDisabled();
    await expect(dialog).toContainText("может перейти на другой аккаунт");
    await expect(dialog).toContainText("Активны разные ручные приоритеты");
    await expect(dialog.getByRole("button", { name: "Уравнять приоритеты аккаунтов" })).toBeVisible();
    expect(await dialog.evaluate((element) => element.scrollWidth <= element.clientWidth)).toBe(true);
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= document.documentElement.clientWidth)).toBe(true);
    await page.screenshot({ path: `output/playwright/routing-distribution-ru-dark-${viewport.width}x${viewport.height}.png` });
  });

  test(`shell disclosure controls ${viewport.width}x${viewport.height}`, async ({ page }) => {
    await installTauriMock(page, { locale: "en", mode: "local", theme: "light", populated: true });
    await page.setViewportSize(viewport);
    await page.goto("/");
    const shell = page.locator(".relay-shell");
    const modeButton = page.getByRole("button", { name: "Mode: This computer" });
    await modeButton.click();
    const modeMenu = page.getByRole("menu");
    await expect(modeMenu.getByRole("menuitemradio")).toHaveCount(3);
    await expect(modeMenu.getByRole("menuitemradio", { name: "This computer" })).toHaveAttribute("aria-checked", "true");
    expect(await modeMenu.evaluate((element) => {
      const rect = element.getBoundingClientRect();
      return rect.left >= 0 && rect.right <= innerWidth && rect.top >= 36 && rect.bottom <= innerHeight;
    })).toBe(true);
    await page.screenshot({ path: `output/playwright/shell-mode-menu-${viewport.width}x${viewport.height}.png` });
    await page.keyboard.press("Escape");
    await expect(modeMenu).toBeHidden();

    if (await shell.evaluate((element) => element.classList.contains("sidebar-collapsed"))) {
      await page.getByRole("button", { name: "Expand sidebar" }).click();
    } else {
      await page.getByRole("button", { name: "Collapse sidebar" }).click();
      await expect(shell).toHaveClass(/sidebar-collapsed/);
      await page.getByRole("button", { name: "Expand sidebar" }).click();
    }
    await expect(shell).not.toHaveClass(/sidebar-collapsed/);
    await expect(page.locator(".relay-sidebar nav button span").first()).toBeVisible();
    await expect(page.locator(".sidebar-help-copy small")).toHaveText("v1.0.5");
    expect(await page.locator(".sidebar-footer").evaluate((footer) => {
      const bounds = footer.getBoundingClientRect();
      return [...footer.children].every((child) => {
        const rect = child.getBoundingClientRect();
        return rect.left >= bounds.left && rect.right <= bounds.right && rect.top >= bounds.top && rect.bottom <= bounds.bottom;
      });
    })).toBe(true);
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= document.documentElement.clientWidth)).toBe(true);
    await page.screenshot({ path: `output/playwright/shell-expanded-${viewport.width}x${viewport.height}.png` });
  });

  test(`secondary actions and dialogs ${viewport.width}x${viewport.height}`, async ({ page }) => {
    await installTauriMock(page, { locale: "en", mode: "local", theme: "light", populated: true });
    await page.setViewportSize(viewport);
    await page.goto("/");

    await page.getByRole("button", { name: "Connections", exact: true }).click();
    await page.getByRole("button", { name: "Sign in", exact: true }).first().click();
    let dialog = page.getByRole("dialog", { name: "Sign in" });
    await expect(dialog.getByText("Waiting for sign-in", { exact: true })).toBeVisible();
    await page.screenshot({ path: `output/playwright/oauth-dialog-${viewport.width}x${viewport.height}.png` });
    await dialog.getByText("Sign-in did not finish automatically", { exact: true }).click();
    await expect(dialog.getByLabel(/Callback URL/)).toBeVisible();
    expect(await dialog.getByLabel(/Callback URL/).evaluate((element) => element.getBoundingClientRect().width >= 180)).toBe(true);
    await page.screenshot({ path: `output/playwright/oauth-resume-dialog-${viewport.width}x${viewport.height}.png` });
    await dialog.getByRole("button", { name: "Close" }).click();

    await page.getByRole("tab", { name: "Sources" }).click();
    const sourceActions = page.locator(".relay-table .row-actions");
    expect(await sourceActions.locator(":scope > *").evaluateAll((items) => items.map((item) => item.tagName === "DETAILS" ? item.querySelector("summary")?.getAttribute("aria-label") : item.getAttribute("aria-label")))).toEqual(["Test", "Edit", "Actions"]);
    await sourceActions.locator("summary").click();
    const sourceMenu = page.getByRole("menu");
    await expect(sourceMenu.getByRole("menuitem")).toHaveCount(2);
    await expect(sourceMenu.getByRole("menuitem", { name: "Delete" })).toBeVisible();
    expect(await sourceMenu.evaluate((element) => {
      const rect = element.getBoundingClientRect();
      return rect.left >= 0 && rect.right <= innerWidth && rect.top >= 36 && rect.bottom <= innerHeight;
    })).toBe(true);
    await page.screenshot({ path: `output/playwright/source-actions-${viewport.width}x${viewport.height}.png` });
    await page.keyboard.press("Escape");

    await page.getByRole("button", { name: "Pool", exact: true }).click();
    await page.getByRole("button", { name: "Pool member policy: Personal Plus", exact: true }).click();
    dialog = page.getByRole("dialog", { name: /Pool member policy/ });
    await expect(dialog).toBeVisible();
    await page.screenshot({ path: `output/playwright/pool-member-dialog-${viewport.width}x${viewport.height}.png` });
    await dialog.getByRole("button", { name: "Close" }).first().click();

    await page.getByRole("button", { name: "Usage", exact: true }).click();
    await page.getByRole("button", { name: "Request details: req_synthetic_local" }).click();
    dialog = page.getByRole("dialog", { name: "Request details" });
    await expect(dialog).toContainText("req_synthetic_local");
    await page.screenshot({ path: `output/playwright/request-details-dialog-${viewport.width}x${viewport.height}.png` });
    await dialog.getByRole("button", { name: "Close" }).first().click();

    await page.getByRole("button", { name: "Gateway", exact: true }).click();
    await page.getByRole("tab", { name: "Diagnostics" }).click();
    await page.getByRole("button", { name: "Preview" }).click();
    dialog = page.getByRole("dialog", { name: "Support bundle preview" });
    await expect(dialog).toBeVisible();
    await page.screenshot({ path: `output/playwright/support-preview-dialog-${viewport.width}x${viewport.height}.png` });
    expect(await dialog.evaluate((element) => {
      const rect = element.getBoundingClientRect();
      return rect.left >= 0 && rect.right <= innerWidth && rect.top >= 36 && rect.bottom <= innerHeight;
    })).toBe(true);
  });
}

for (const viewport of viewports) {
  test(`profile switch repair dialog ${viewport.width}x${viewport.height}`, async ({ page }) => {
    await installTauriMock(page, { locale: "ru", mode: "local", theme: "dark", populated: true });
    await page.setViewportSize(viewport);
    await page.goto("/");
    await page.getByRole("button", { name: "Подключения", exact: true }).click();
    await page.getByRole("button", { name: "Запустить в Codex" }).click();

    const dialog = page.getByRole("dialog", { name: "Сохранить видимость чатов Codex" });
    await expect(dialog.locator("dd")).toHaveText(["2", "2", "1"]);
    expect(await dialog.evaluate((element) => {
      const rect = element.getBoundingClientRect();
      return rect.left >= 0 && rect.right <= innerWidth && rect.top >= 36 && rect.bottom <= innerHeight;
    })).toBe(true);
    expect(await dialog.locator("footer button span").evaluateAll((items) => items.every((item) => item.scrollWidth <= item.clientWidth))).toBe(true);
    await page.screenshot({ path: `output/playwright/profile-switch-repair-ru-dark-${viewport.width}x${viewport.height}.png` });
  });
}

for (const viewport of viewports) {
  test(`OpenCode-only profile actions ${viewport.width}x${viewport.height}`, async ({ page }) => {
    await installTauriMock(page, { locale: "ru", mode: "local", theme: "dark", populated: true, codexBindings: false });
    await page.setViewportSize(viewport);
    await page.goto("/");
    await page.getByRole("button", { name: "Профили", exact: true }).click();
    await expect(page.getByRole("button", { name: "Добавить профиль" })).toBeEnabled();
    await expect(page.getByRole("row").filter({ hasText: "OpenCode" })).toBeVisible();
    await page.screenshot({ path: `output/playwright/opencode-profile-${viewport.width}x${viewport.height}.png` });

    await page.getByRole("button", { name: "Добавить профиль" }).click();
    const dialog = page.getByRole("dialog", { name: "Добавить профиль" });
    await expect(dialog.getByRole("button", { name: /Codex/ })).toBeVisible();
    await expect(dialog.getByRole("button", { name: /OpenCode/ })).toBeVisible();
    expect(await dialog.evaluate((element) => {
      const rect = element.getBoundingClientRect();
      return rect.left >= 0 && rect.right <= innerWidth && rect.top >= 36 && rect.bottom <= innerHeight;
    })).toBe(true);
    await page.screenshot({ path: `output/playwright/add-profile-dialog-${viewport.width}x${viewport.height}.png` });
  });
}

for (const theme of themes) {
  for (const viewport of viewports) {
    test(`Codex pool account setup ${theme} ${viewport.width}x${viewport.height}`, async ({ page }) => {
      await installTauriMock(page, { locale: "ru", mode: "local", theme, populated: true, accountCount: 4 });
      await page.setViewportSize(viewport);
      await page.goto("/");
      await page.getByRole("button", { name: "Адрес API", exact: true }).click();
      await page.getByRole("tab", { name: "Настройка Codex" }).click();

      const setup = page.locator(".client-oauth-binding");
      await expect(setup.getByRole("heading", { name: "Codex в режиме пула" })).toBeVisible();
      await expect(page.getByLabel("Аккаунт интерфейса Codex")).toHaveValue("auto");
      await expect(setup).not.toContainText("Выбран");
      await expect(page.locator(".codex-oauth-account-summary")).toHaveCount(0);
      await expect(page.locator(".client-setup button")).toHaveCount(0);
      expect(await setup.evaluate((element) => {
        const rect = element.getBoundingClientRect();
        return rect.left >= 0 && rect.right <= innerWidth && rect.top >= 36 && rect.bottom <= innerHeight;
      })).toBe(true);
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= document.documentElement.clientWidth)).toBe(true);
      await page.screenshot({ path: `output/playwright/codex-pool-account-ru-${theme}-${viewport.width}x${viewport.height}.png` });
    });
  }
}

test("automation table fits the standard window without horizontal scrolling", async ({ page }) => {
  await installTauriMock(page, { locale: "ru", mode: "local", theme: "dark", populated: true });
  await page.setViewportSize({ width: 1160, height: 760 });
  await page.goto("/");
  await page.getByRole("button", { name: "Подключения", exact: true }).click();
  await page.getByRole("tab", { name: "Автоматизация" }).click();
  const table = page.locator(".relay-table-wrap");
  expect(await table.evaluate((element) => element.scrollWidth <= element.clientWidth)).toBe(true);
});

test("ru compact disclosure labels stay readable", async ({ page }) => {
  await installTauriMock(page, { locale: "ru", mode: "local", theme: "dark", populated: true });
  await page.setViewportSize({ width: 840, height: 560 });
  await page.goto("/");

  await page.locator(".mode-picker > button").click();
  const modeMenu = page.getByRole("menu");
  await expect(modeMenu.getByRole("menuitemradio")).toHaveCount(3);
  expect(await modeMenu.locator("span").evaluateAll((items) => items.every((item) => item.scrollWidth <= item.clientWidth))).toBe(true);
  await page.screenshot({ path: "output/playwright/shell-mode-menu-ru-840x560.png" });
  await page.keyboard.press("Escape");

  await page.locator(".relay-sidebar nav button").nth(1).click();
  await page.getByRole("tab", { name: "Источники API" }).click();
  await page.locator(".relay-table .row-actions summary").click();
  let menu = page.getByRole("menu");
  expect(await menu.locator("span").evaluateAll((items) => items.every((item) => item.scrollWidth <= item.clientWidth))).toBe(true);
  await page.screenshot({ path: "output/playwright/source-actions-ru-840x560.png" });
  await page.keyboard.press("Escape");

  await page.locator(".relay-sidebar nav button").nth(2).click();
  await page.getByRole("button", { name: "Правила участника пула: Personal Plus", exact: true }).click();
  let dialog = page.getByRole("dialog", { name: /Правила участника пула/ });
  await expect(dialog).toBeVisible();
  await expect(dialog.getByText("Приоритет маршрутизации", { exact: true })).toBeVisible();
  await expect(dialog.getByText("Доля трафика", { exact: true })).toBeVisible();
  await page.screenshot({ path: "output/playwright/pool-member-dialog-ru-840x560.png" });
  await dialog.getByRole("button", { name: "Закрыть" }).first().click();

  await page.locator(".relay-sidebar nav button").nth(4).click();
  await page.locator(".request-disclosure").click();
  dialog = page.getByRole("dialog", { name: "Сведения о запросе" });
  await expect(dialog).toContainText("req_synthetic_local");
  await page.screenshot({ path: "output/playwright/request-details-dialog-ru-840x560.png" });
  await dialog.getByRole("button", { name: "Закрыть" }).first().click();

  await page.locator(".relay-sidebar nav button").nth(3).click();
  await page.getByRole("tab", { name: "Диагностика" }).click();
  await page.getByRole("button", { name: "Предпросмотр" }).click();
  dialog = page.getByRole("dialog", { name: "Предпросмотр пакета поддержки" });
  await expect(dialog).toBeVisible();
  await page.screenshot({ path: "output/playwright/support-preview-dialog-ru-840x560.png" });
  expect(await dialog.evaluate((element) => {
    const rect = element.getBoundingClientRect();
    return rect.left >= 0 && rect.right <= innerWidth && rect.top >= 36 && rect.bottom <= innerHeight;
  })).toBe(true);
});

for (const scenario of [
  { locale: "en" as const, theme: "light" as const, width: 1160, label: "First / total", file: "usage-timing-en-light-1160x760.png" },
  { locale: "ru" as const, theme: "dark" as const, width: 840, label: "Первый / всего", file: "usage-timing-ru-dark-840x560.png" },
]) {
  test(`usage timing ${scenario.locale} ${scenario.theme} ${scenario.width}`, async ({ page }) => {
    await installTauriMock(page, { locale: scenario.locale, mode: "local", theme: scenario.theme, populated: true });
    await page.setViewportSize({ width: scenario.width, height: scenario.width === 840 ? 560 : 760 });
    await page.goto("/");
    await page.locator(".relay-sidebar nav button").nth(4).click();

    await expect(page.getByRole("columnheader", { name: scenario.label })).toBeVisible();
    const timing = page.getByRole("row").filter({ hasText: "req_synthetic_local" }).getByRole("cell").nth(4);
    await expect(timing).toHaveText("128 / 428 ms");
    expect(await timing.evaluate((element) => element.scrollWidth <= element.clientWidth)).toBe(true);
    const clippedHeaders = await page.locator(".usage-request-table th").evaluateAll((items) => items.filter((item) => item.scrollWidth > item.clientWidth || item.scrollHeight > item.clientHeight).map((item) => item.textContent));
    expect(clippedHeaders).toEqual([]);
    await page.screenshot({ path: `output/playwright/${scenario.file}` });
  });
}

for (const viewport of viewports) {
  test(`usage filter hierarchy ${viewport.width}x${viewport.height}`, async ({ page }) => {
    await installTauriMock(page, { locale: "ru", mode: "local", theme: "dark", populated: true });
    await page.setViewportSize(viewport);
    await page.goto("/");
    await page.getByRole("button", { name: "Использование", exact: true }).click();

    const filters = page.locator(".usage-filter-panel");
    await expect(filters.getByLabel("Период")).toBeVisible();
    await expect(filters.getByLabel("Локальный ключ")).toHaveCount(0);
    await filters.getByRole("button", { name: "Другие фильтры" }).click();
    await expect(filters.getByLabel("Локальный ключ")).toBeVisible();
    await filters.getByLabel("Локальный ключ").fill("synthetic");
    await expect(filters.locator(".usage-filter-toggle-wrap small")).toHaveText("1");
    await expect(filters.getByRole("button", { name: "Сбросить фильтры" })).toBeVisible();
    await filters.getByRole("button", { name: "Сбросить фильтры" }).click();
    await expect(filters.getByLabel("Локальный ключ")).toHaveValue("");
    await expect(filters.getByRole("button", { name: "Сбросить фильтры" })).toHaveCount(0);
    await page.screenshot({ path: `output/playwright/usage-filters-open-ru-dark-${viewport.width}x${viewport.height}.png` });

    await page.getByRole("tab", { name: "Модели" }).click();
    const aggregate = page.locator(".usage-aggregate-table");
    await expect(aggregate.getByRole("columnheader")).toHaveCount(6);
    expect(await aggregate.locator(".usage-token-breakdown").evaluate((element) => element.scrollWidth <= element.clientWidth)).toBe(true);
    expect(await aggregate.locator("xpath=..").evaluate((element) => element.scrollWidth <= element.clientWidth)).toBe(true);
  });
}
