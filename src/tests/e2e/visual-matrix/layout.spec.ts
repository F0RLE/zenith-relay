import { expect, test } from "../../bun-playwright";
import { installTauriMock } from "../tauri-mock";
import { themes, viewports } from "./fixtures";

for (const locale of ["ru", "en"] as const) for (const theme of themes) for (const viewport of viewports) {
  test(`segmented summaries ${locale} ${theme} ${viewport.width}`, async ({ page }) => {
    await installTauriMock(page, { locale, mode: "local", theme, populated: true, accountCount: 3, quotaAvailable: true, providerCredits: 2.5 });
    await page.setViewportSize(viewport);
    await page.goto("/");
    const assertBand = async (selector: string, count: number) => {
      const band = page.locator(selector);
      await expect(band.locator(":scope > div")).toHaveCount(count);
      const layout = await band.evaluate((element) => {
        const bounds = element.getBoundingClientRect();
        return { display: getComputedStyle(element).display, gap: getComputedStyle(element).gap,
          cells: Array.from(element.children).map((child) => {
            const cell = child.getBoundingClientRect();
            return { text: child.textContent, height: cell.height,
              fits: cell.left >= bounds.left - 1 && cell.right <= bounds.right + 1
                && child.scrollWidth <= child.clientWidth && child.scrollHeight <= child.clientHeight
                && Array.from(child.querySelectorAll("span, strong")).every((text) => text.scrollWidth <= text.clientWidth) };
          }) };
      });
      expect(layout.display).toBe("grid");
      expect(layout.gap).toBe("1px");
      for (const cell of layout.cells) {
        expect(cell.height, `${selector}: ${JSON.stringify(cell)}`).toBeLessThanOrEqual(40);
        expect(cell.fits, `${selector}: ${JSON.stringify(cell)}`).toBe(true);
      }
    };
    const overviewCells = page.locator(".overview-metrics > div");
    await expect(overviewCells).toHaveCount(4);
    await expect(overviewCells.locator("span")).toHaveText(locale === "ru"
      ? ["Запросы", "Подключения", "Модели", "Ошибки"]
      : ["Requests", "Connections", "Models", "Errors"]);
    expect(await overviewCells.evaluateAll((cells) => cells.every((cell) => cell.getBoundingClientRect().height === 58))).toBe(true);
    await page.screenshot({ path: `output/playwright/summary-overview-${locale}-${theme}-${viewport.width}.png` });
    await page.getByRole("button", { name: locale === "ru" ? "Пул" : "Pool", exact: true }).click();
    await page.screenshot({ path: `output/playwright/summary-pool-${locale}-${theme}-${viewport.width}.png` });
    await assertBand(".pool-summary", 5);
    await assertBand(".pool-economics", 3);
    const poolCells = page.locator(".pool-summary-band .relay-status-summary > div");
    await expect(poolCells.locator("span")).toHaveCount(0);
    expect(await poolCells.evaluateAll((cells) => {
      const top = cells[0].getBoundingClientRect().top;
      return cells.every((cell) => Math.abs(cell.getBoundingClientRect().top - top) <= 1);
    })).toBe(true);
    await page.getByRole("button", { name: locale === "ru" ? "Подключения" : "Connections", exact: true }).click();
    await assertBand(".connections-account-summary", 5);
    await page.screenshot({ path: `output/playwright/summary-accounts-${locale}-${theme}-${viewport.width}.png` });
    await page.getByRole("tab", { name: locale === "ru" ? "Источники API" : "Sources", exact: true }).click();
    await assertBand(".sources-controls .relay-status-summary", 4);
    await page.screenshot({ path: `output/playwright/summary-sources-${locale}-${theme}-${viewport.width}.png` });
    await page.getByRole("tab", { name: locale === "ru" ? "Прокси" : "Proxies", exact: true }).click();
    await assertBand(".proxy-storage-counts", 3);
    await page.screenshot({ path: `output/playwright/summary-proxies-${locale}-${theme}-${viewport.width}.png` });
  });
}

for (const viewport of viewports) {
  test(`empty pool and quota policy ${viewport.width}x${viewport.height}`, async ({ page }) => {
    await installTauriMock(page, { locale: "ru", mode: "local", theme: "dark", populated: true, accountCount: 4, accountOAuthClientKind: "excel_bps", poolMembers: false, gatewayRunning: false });
    await page.setViewportSize(viewport);
    await page.goto("/");
    await page.getByRole("button", { name: "Пул", exact: true }).click();
    await expect(page.getByText("В пуле нет участников", { exact: true })).toBeVisible();
    await page.screenshot({ path: `output/playwright/pool-empty-ru-dark-${viewport.width}x${viewport.height}.png` });

    await page.getByRole("button", { name: "Добавить участника", exact: true }).first().click();
    let dialog = page.getByRole("dialog", { name: "Добавить подключения в пул" });
    await expect(dialog).toBeVisible();
    await page.screenshot({ path: `output/playwright/pool-add-members-ru-dark-${viewport.width}x${viewport.height}.png` });
    const accountSearch = dialog.getByLabel("Найти подключение");
    await accountSearch.fill("pro");
    await expect(dialog.locator(".pool-member-options > label").first()).toContainText("Pro account");
    const planBadge = dialog.locator(".pool-member-options .account-plan-badge");
    await expect(planBadge).toHaveText("Pro 200");
    expect(await planBadge.evaluate((badge) => badge.scrollWidth <= badge.clientWidth && badge.scrollHeight <= badge.clientHeight)).toBe(true);
    await page.screenshot({ path: `output/playwright/pool-add-pro-ru-dark-${viewport.width}x${viewport.height}.png` });
    await accountSearch.fill("");
    await dialog.getByText("Business Workspace", { exact: true }).click();
    await dialog.getByRole("button", { name: "Добавить выбранные (1)" }).click();
    await expect(page.getByRole("dialog", { name: "Добавить обычный аккаунт в пул?", exact: true })).toHaveCount(0);

    const memberActions = page.locator(".pool-member-card .pool-member-actions");
    await expect(memberActions.getByRole("button")).toHaveCount(3);
    expect(await memberActions.evaluate((actions) => {
      const card = actions.closest(".pool-member-card")?.getBoundingClientRect();
      const bounds = actions.getBoundingClientRect();
      return Boolean(card && bounds.left >= card.left && bounds.right <= card.right && bounds.top >= card.top && bounds.bottom <= card.bottom);
    })).toBe(true);

    const poolToolbar = page.locator(".pool-member-toolbar");
    await expect(poolToolbar).toBeVisible();
    expect(await poolToolbar.evaluate((toolbar) => {
      const tabs = document.querySelector<HTMLElement>(".relay-tabs");
      const priority = toolbar.querySelector<HTMLElement>(".pool-priority-label");
      if (!tabs || !priority) return false;
      return priority.getBoundingClientRect().top - tabs.getBoundingClientRect().bottom >= 9;
    })).toBe(true);
    const headerActions = page.locator(".pool-header-actions");
    await expect(headerActions.locator(":scope > *")).toHaveCount(3);
    await headerActions.locator(".pool-preset-menu summary").click();
    await expect(headerActions.getByRole("menuitem", { name: "Сохранить пресет", exact: true })).toBeVisible();
    await expect(headerActions.getByRole("menuitem", { name: "Применить пресет", exact: true })).toBeVisible();
    await page.keyboard.press("Escape");
    await expect(headerActions.getByRole("menu")).toBeHidden();
    await expect(headerActions.getByRole("button", { name: "Добавить участника", exact: true })).toBeVisible();
    await expect(headerActions.getByRole("button", { name: "Запустить пул", exact: true })).toHaveCount(0);
    await expect(headerActions.getByRole("button", { name: "Подключить", exact: true })).toBeDisabled();
    const actionBoxes = await headerActions.locator(":scope > .relay-button").evaluateAll((buttons) => buttons.map((button) => {
      const rect = button.getBoundingClientRect();
      return { width: rect.width, height: rect.height, overflow: button.scrollWidth - button.clientWidth };
    }));
    expect(Math.max(...actionBoxes.map((box) => box.height)) - Math.min(...actionBoxes.map((box) => box.height))).toBeLessThanOrEqual(1);
    expect(actionBoxes.every((box) => box.height <= 36 && box.overflow === 0)).toBe(true);
    expect(actionBoxes.reduce((total, box) => total + box.width, 0)).toBeLessThan(380);
    await page.screenshot({ path: `output/playwright/pool-header-actions-ru-dark-${viewport.width}x${viewport.height}.png` });

    await expect(page.locator(".pool-sort-menu")).toHaveCount(0);
    await expect(page.locator(".pool-priority-label")).toContainText("Распределение запросов");
    await expect(page.getByRole("button", { name: "Настройки ротации пула", exact: true })).toBeVisible();
    const poolToolbarGroups = page.locator(".pool-quota-actions > .pool-control-group");
    await expect(poolToolbarGroups).toHaveCount(2);
    await expect(poolToolbarGroups.evaluateAll((groups) => groups.map((group) => group.getAttribute("data-toolbar-group")))).resolves.toEqual(["routing", "refresh"]);
    await expect(poolToolbarGroups.nth(0).getByRole("radiogroup", { name: "Скорость запроса" })).toBeVisible();
    await expect(poolToolbarGroups.nth(0).getByRole("button", { name: "Настройки ротации пула", exact: true })).toBeVisible();
    await expect(poolToolbarGroups.nth(1).locator(":scope > *")).toHaveCount(2);
    await expect(poolToolbarGroups.nth(1).getByRole("button")).toHaveCount(2);
    await page.screenshot({ path: `output/playwright/pool-priority-ru-dark-${viewport.width}x${viewport.height}.png` });
    expect(await page.locator(".pool-summary > div").evaluateAll((cells) => cells.every((cell) => {
      const value = cell.querySelector("strong")!.getBoundingClientRect();
      const bounds = cell.getBoundingClientRect();
      return Boolean(cell.getAttribute("aria-label")) && value.left >= bounds.left && value.right <= bounds.right && cell.scrollWidth <= cell.clientWidth;
    }))).toBe(true);
    await page.screenshot({ path: `output/playwright/pool-members-ru-dark-${viewport.width}x${viewport.height}.png` });

    expect(await page.evaluate(() => document.documentElement.scrollWidth <= document.documentElement.clientWidth)).toBe(true);
  });

  test(`pool member cards ${viewport.width}x${viewport.height}`, async ({ page }) => {
    await installTauriMock(page, { locale: "ru", mode: "local", theme: "dark", populated: true, accountCount: 5, usageAccountIndex: 3 });
    await page.setViewportSize(viewport);
    await page.goto("/");
    await page.getByRole("button", { name: "Пул", exact: true }).click();
    await expect(page.locator(".relay-tabs").getByRole("tab")).toHaveText(["Участники", "Правила моделей"]);
    const speed = page.locator(".pool-speed-control");
    await expect(speed.getByRole("radio", { name: "Обычная", exact: true })).toBeChecked();
    await speed.getByRole("radio", { name: "Быстрая", exact: true }).click();
    await expect(speed.getByRole("radio", { name: "Быстрая", exact: true })).toBeEnabled();
    await expect(speed).toHaveAttribute("data-speed-tier", "fast");
    await expect(speed.locator("button.active")).toHaveText("Быстрая");
    await page.screenshot({ path: `output/playwright/pool-speed-slider-ru-dark-${viewport.width}x${viewport.height}.png` });
    await page.getByRole("button", { name: "Настройки ротации пула", exact: true }).click();
    const distribution = page.getByRole("dialog", { name: "Ротация пула" });
    await expect(distribution).not.toContainText("Скорость запроса");
    await distribution.getByRole("radio", { name: "По кругу", exact: true }).click();
    await expect(distribution.getByRole("listitem")).toHaveCount(6);
    expect(await distribution.evaluate((element) => element.scrollWidth <= element.clientWidth && element.getBoundingClientRect().bottom <= innerHeight)).toBe(true);
    await page.screenshot({ path: `output/playwright/pool-member-order-${viewport.width}x${viewport.height}.png` });
    await distribution.getByRole("button", { name: "Закрыть", exact: true }).last().click();
    await page.mouse.move(1, 1);
    await expect(page.getByRole("tooltip")).toHaveCount(0);
    const members = page.locator(".pool-member-list");
    await expect(members.locator(".pool-member-card")).toHaveCount(6);
    await expect(members.locator('.pool-member-card[data-current="true"]')).toHaveCount(1);
    expect(await members.locator('.pool-member-card[data-current="true"]').evaluate((element) => {
      const style = getComputedStyle(element);
      const other = element.parentElement?.querySelector('.pool-member-card[data-current="false"]');
      return getComputedStyle(element, "::before").content === "none"
        && other != null && style.borderTopColor !== getComputedStyle(other).borderTopColor
        && style.backgroundColor === getComputedStyle(other).backgroundColor;
    })).toBe(true);
    await expect(page.locator(".pool-summary > div")).toHaveCount(4);
    await expect(members.getByText("Pro account", { exact: true })).toBeVisible();
    await expect(members.locator('.account-plan-badge[data-plan="pro-200"]')).toHaveText("Pro 200");
    const apiCard = members.locator('.pool-member-card[data-member-kind="source"]');
    await expect(apiCard).toContainText("42,50");
    await expect(apiCard).toContainText("7,50");
    await expect(apiCard).toContainText("128");
    await expect(apiCard.getByRole("button", { name: "Обновить баланс" })).toBeVisible();
    await expect(apiCard.locator(".pool-member-runtime-meta")).toContainText("Режим работы");
    await expect(apiCard.locator(".pool-member-runtime-meta")).toContainText("Активных запросов");
    await expect(apiCard.locator(".pool-member-active-runtime")).toHaveCount(0);
    expect(await apiCard.locator(".pool-member-runtime-meta > div").evaluateAll((items) => items.every((item) => getComputedStyle(item).textAlign === "center"))).toBe(true);
    await expect(members.getByRole("button", { name: "Обновить", exact: true })).toHaveCount(5);
    expect(await members.locator(".pool-member-context").evaluateAll((items) => items.every((item) => getComputedStyle(item).justifyContent === "center" && getComputedStyle(item).textAlign === "center"))).toBe(true);
    await expect(members).not.toContainText("Доля");
    expect(await page.getByRole("button", { name: "Настройки ротации пула", exact: true }).evaluate((control) => control.scrollWidth <= control.clientWidth)).toBe(true);
    await expect(page.getByRole("radio", { name: "Компактный вид пула" })).toHaveCount(0);
    await expect(members.locator(".pool-member-card-quota").first()).toBeVisible();
    await page.mouse.move(1, 1);
    await page.screenshot({ path: `output/playwright/pool-members-${viewport.width}x${viewport.height}.png` });
    if (viewport.width === 1160) {
      await page.setViewportSize({ width: 2048, height: 1152 });
      await page.evaluate(() => { document.documentElement.dataset.theme = "light"; });
      await page.waitForTimeout(200);
      expect(await members.locator(".pool-member-card").evaluateAll((items) => items.every((item) => item.scrollWidth <= item.clientWidth))).toBe(true);
      await page.screenshot({ path: "output/playwright/pool-members-ru-light-2048x1152.png" });
    }
    expect(await page.evaluate(() => localStorage.getItem("relay.poolLayout"))).toBeNull();
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= document.documentElement.clientWidth)).toBe(true);
    expect(await members.locator(".pool-member-card").evaluateAll((items) => items.every((item) => item.scrollWidth <= item.clientWidth))).toBe(true);
  });

  test(`pool member cards en light ${viewport.width}x${viewport.height}`, async ({ page }) => {
    await installTauriMock(page, { locale: "en", mode: "local", theme: "light", populated: true, accountCount: 4, usageAccountIndex: 3 });
    await page.setViewportSize(viewport);
    await page.goto("/");
    await page.getByRole("button", { name: "Pool", exact: true }).click();
    await expect(page.locator(".relay-tabs").getByRole("tab")).toHaveText(["Members", "Model Rules"]);
    const members = page.locator(".pool-member-list");

    await expect(members.getByText("Pro account", { exact: true })).toBeVisible();
    await expect(members.locator('.account-plan-badge[data-plan="pro-200"]')).toHaveText("Pro 200");
    const apiCard = members.locator('.pool-member-card[data-member-kind="source"]');
    await expect(apiCard).toContainText("$42.50");
    await expect(apiCard).toContainText("$7.50");
    await expect(apiCard.getByRole("button", { name: "Refresh balance" })).toBeVisible();
    await expect(page.getByRole("radio", { name: "Compact pool view" })).toHaveCount(0);
    expect(await page.evaluate(() => localStorage.getItem("relay.poolLayout"))).toBeNull();
    await page.mouse.move(1, 1);
    await page.screenshot({ path: `output/playwright/pool-members-en-light-${viewport.width}x${viewport.height}.png` });
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= document.documentElement.clientWidth)).toBe(true);
    expect(await members.locator(".pool-member-card").evaluateAll((items) => items.every((item) => item.scrollWidth <= item.clientWidth))).toBe(true);
  });

  test(`proxy controls ${viewport.width}x${viewport.height}`, async ({ page }) => {
    await installTauriMock(page, { locale: "en", mode: "local", theme: "light", populated: true });
    await page.setViewportSize(viewport);
    await page.goto("/");
    await page.getByRole("button", { name: "Connections", exact: true }).click();
    await page.locator(".account-card").first().locator(".account-row-menu summary").click();
    await page.getByRole("menuitem", { name: "Proxy: Common", exact: true }).click();
    const accountProxy = page.getByRole("dialog", { name: "Account proxy" });
    await expect(accountProxy).toBeVisible();
    await expect(accountProxy.getByRole("radio", { name: /Assign automatically/ })).toBeVisible();
    await expect(accountProxy.getByRole("radio", { name: /Choose from storage/ })).toBeVisible();
    await page.screenshot({ path: `output/playwright/proxy-account-${viewport.width}x${viewport.height}.png` });
    await accountProxy.getByRole("button", { name: "Cancel" }).click();

    await page.getByRole("tab", { name: "Proxies" }).click();
    await expect(page.locator(".proxy-storage-counts strong").first()).toHaveText("3");
    await page.screenshot({ path: `output/playwright/proxy-storage-${viewport.width}x${viewport.height}.png` });
    await page.getByRole("button", { name: "Import", exact: true }).click();
    const proxyImport = page.getByRole("dialog", { name: "Import proxies" });
    await expect(proxyImport.getByLabel("Proxy list")).toBeVisible();
    await page.screenshot({ path: `output/playwright/proxy-import-${viewport.width}x${viewport.height}.png` });
    await proxyImport.getByRole("button", { name: "Cancel" }).click();

    await page.getByRole("tab", { name: "Accounts" }).click();
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
    await dialog.getByLabel("Markdown description").fill("## Package contents\n\n- Two Business accounts\n- Active subscription");
    await dialog.getByRole("button", { name: "Preview", exact: true }).click();
    await expect(dialog.getByRole("heading", { name: "Package contents" })).toBeVisible();
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
    await expect(menu.getByRole("menuitem")).toHaveText(["Notes", "Proxy: Common", "Export", "Disable", "Delete"]);
    await page.screenshot({ path: `output/playwright/account-actions-${viewport.width}x${viewport.height}.png` });
    expect(await menu.evaluate((element) => {
      const rect = element.getBoundingClientRect();
      return rect.left >= 0 && rect.right <= innerWidth && rect.top >= 36 && rect.bottom <= innerHeight;
    })).toBe(true);
    expect(await menu.evaluate((element) => {
      const rect = element.getBoundingClientRect();
      return document.elementFromPoint(rect.left + rect.width / 2, rect.bottom - 4)?.closest("[role=menu]") === element;
    })).toBe(true);
  });

  test(`account error details ${viewport.width}x${viewport.height}`, async ({ page }) => {
    await installTauriMock(page, { locale: "ru", mode: "local", theme: "dark", populated: true, accountCount: 3, quotaAvailable: true, accountAuthReason: "invalid_grant" });
    await page.setViewportSize(viewport);
    await page.goto("/");
    await page.getByRole("button", { name: "Подключения", exact: true }).click();
    await page.locator('.account-filter-menu').filter({ has: page.getByRole("button", { name: /^Фильтр по подписке:/ }) }).getByRole("button").click();
    await page.locator('[role="option"][data-value="errors"]').click();
    await page.locator(".account-card").filter({ hasText: "Personal Plus" }).locator(".account-status-button").click();
    const dialog = page.getByRole("dialog", { name: "Технические детали ошибки" });
    await expect(dialog.locator("pre")).toContainText('"code": "auth_invalid_grant"');
    await expect(dialog.locator(".form-note")).toHaveCount(0);
    await expect(dialog.getByRole("button", { name: "Копировать", exact: true })).toBeVisible();
    await expect(dialog.locator("footer").getByRole("button", { name: "Закрыть", exact: true })).toBeVisible();
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
    await expect(page.locator(".account-command-actions").getByRole("button", { name: "Refresh", exact: true })).toBeVisible();
    await page.locator(".account-bulk-menu summary").click();
    const menu = page.locator(".account-bulk-menu [role=menu]");
    await expect(menu.getByRole("menuitem")).toHaveCount(2);
    await expect(menu.getByRole("menuitem", { name: "Refresh", exact: true })).toHaveCount(0);
    await expect(menu.getByRole("menuitem", { name: "Refresh and delete non-working accounts" })).toHaveCount(0);
    await page.screenshot({ path: `output/playwright/account-bulk-actions-${viewport.width}x${viewport.height}.png` });
    expect(await menu.evaluate((element) => {
      const rect = element.getBoundingClientRect();
      return rect.left >= 0 && rect.right <= innerWidth && rect.top >= 36 && rect.bottom <= innerHeight;
    })).toBe(true);
    expect(await menu.evaluate((element) => {
      const rect = element.getBoundingClientRect();
      return document.elementFromPoint(rect.left + rect.width / 2, rect.bottom - 4)?.closest("[role=menu]") === element;
    })).toBe(true);
  });

  for (const theme of themes) {
    test(`icon tooltip ${theme} ${viewport.width}x${viewport.height}`, async ({ page }) => {
      await installTauriMock(page, { locale: "ru", mode: "local", theme, populated: true, accountCount: 3 });
      await page.setViewportSize(viewport);
      await page.goto("/");
      await page.getByRole("button", { name: "Подключения", exact: true }).click();
      await page.locator(".account-command-actions").getByRole("button", { name: "Обновить", exact: true }).hover();
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
    const selection = page.locator(".account-card").filter({ hasText: "Personal Plus" }).locator(".account-select-button");
    await expect(selection).toHaveAttribute("aria-pressed", "false");
    await selection.click();
    await expect(selection).toHaveAttribute("aria-pressed", "true");
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
      const actions = card.querySelector<HTMLElement>(".account-card-actions")?.getBoundingClientRect();
      return Boolean(actions && actions.left >= cardRect.left && actions.right <= cardRect.right && actions.top >= cardRect.top && actions.bottom <= cardRect.bottom);
    })).toBe(true);
    expect(await page.locator(".account-card-main").evaluate((main) => getComputedStyle(main).backgroundColor)).toBe("rgba(0, 0, 0, 0)");
  });

  test(`account identity ru ${viewport.width}x${viewport.height}`, async ({ page }) => {
    await installTauriMock(page, { locale: "ru", mode: "local", theme: "dark", populated: true });
    await page.setViewportSize(viewport);
    await page.goto("/");
    await page.locator(".relay-sidebar nav button").nth(1).click();
    await page.getByRole("button", { name: "Показать все аккаунты полностью" }).click();
    const identity = page.locator(".account-card").first().locator(".account-identity > strong");
    await expect(identity).toHaveText("person@example.test");
    await expect(page.locator(".account-card").first().getByText("Personal Plus", { exact: true })).toHaveCount(0);
    await expect(page.getByRole("button", { name: "Скрыть все аккаунты" })).toBeVisible();
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
    await expect(page.locator(".account-filter-menu")).toHaveCount(2);
    const business = cards.filter({ has: page.getByText("Business Workspace", { exact: true }) });
    const backup = cards.filter({ has: page.getByText("Backup account", { exact: true }) });
    await expect(business).toContainText("Business");
    await expect(business).toContainText("5 недель");
    await expect(business.locator(".quota-meter")).toHaveCount(1);
    await expect(backup.locator(".quota-meter")).toHaveCount(1);
    await expect(backup.locator(".account-status-button")).toHaveCount(1);
    await expect(backup.locator(".account-kind-icon")).toHaveAttribute("aria-label", "Не удалось подключиться для обновления квоты");
    await expect(backup).not.toContainText("quota_transport");
    await expect(business.locator(".account-subscription-line")).toContainText(/\d{2}\.\d{2}\.\d{4}, \d{2}:\d{2}/);
    await expect(business.locator(".account-subscription-countdown")).toHaveText(/^\d+ дн\. \d+ ч \d+ мин$/);
    await expect(backup.locator(".account-subscription-line")).toHaveCount(0);
    expect(await cards.evaluateAll((items) => items.every((item) => !item.textContent?.includes("Модели")))).toBe(true);
    await page.screenshot({ path: `output/playwright/multiple-accounts-ru-${viewport.width}x${viewport.height}.png` });
    expect(await page.locator(".account-list").evaluate((list, narrow) => getComputedStyle(list).gridTemplateColumns.split(" ").length === (narrow ? 2 : 3), viewport.width <= 900)).toBe(true);
    expect(await page.locator(".account-filter-stack").evaluate((element) => element.scrollWidth <= element.clientWidth)).toBe(true);
    expect(await page.locator(".account-subscription-line").evaluateAll((items) => items.every((item) => item.scrollWidth <= item.clientWidth))).toBe(true);
    await expect(cards.locator(".account-card-actions")).toHaveCount(3);
    await expect(cards.locator(".account-card-actions > .relay-icon-button")).toHaveCount(9);
    // Codex accounts can participate in the image-only pool and still launch the app.
    await expect(cards.getByRole("button", { name: "Убрать из пула", exact: true })).toHaveCount(3);
    await expect(cards.getByRole("button", { name: "Запустить в ChatGPT", exact: true })).toHaveCount(3);

    await expect(page.getByRole("button", { name: "Список" })).toHaveCount(0);
    expect(await cards.locator(".account-card-main").evaluateAll((items) => items.every((item) => item.scrollWidth <= item.clientWidth))).toBe(true);
  });

  test(`account cards en light ${viewport.width}x${viewport.height}`, async ({ page }) => {
    await installTauriMock(page, { locale: "en", mode: "local", theme: "light", populated: true, accountCount: 4 });
    await page.setViewportSize(viewport);
    await page.goto("/");
    await page.getByRole("button", { name: "Connections", exact: true }).click();
    const accounts = page.locator(".account-list");
    await expect(accounts.locator(".account-card")).toHaveCount(4);

    await expect(accounts).toContainText("Pro account");
    expect(await page.locator(".account-filter-menu .relay-option-trigger").evaluateAll((items) => items.every((item) => {
      const label = item.querySelector<HTMLElement>("span");
      return Boolean(label && item.getBoundingClientRect().width <= label.scrollWidth + 52);
    }))).toBe(true);
    await page.mouse.move(1, 1);
    await page.screenshot({ path: `output/playwright/account-cards-en-light-${viewport.width}x${viewport.height}.png` });
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= document.documentElement.clientWidth)).toBe(true);
    expect(await accounts.locator(".account-card").evaluateAll((items) => items.every((item) => item.scrollWidth <= item.clientWidth))).toBe(true);
    expect(await accounts.locator(".account-card-main").evaluateAll((items) => items.every((item) => item.scrollWidth <= item.clientWidth))).toBe(true);
    expect(await accounts.locator(".quota-meter-heading small").evaluateAll((items) => items.every((item) => item.scrollWidth <= item.clientWidth))).toBe(true);
  });

  test(`quota windows ${viewport.width}x${viewport.height}`, async ({ page }) => {
    await installTauriMock(page, { locale: "en", mode: "local", theme: "light", populated: true, supplementalQuota: true });
    await page.setViewportSize(viewport);
    await page.goto("/");
    await page.getByRole("button", { name: "Connections", exact: true }).click();
    const meters = page.locator(".account-list .quota-meter");
    await expect(meters).toHaveCount(4);
    await expect(page.locator(".account-list")).toContainText("5 hours");
    await expect(page.locator(".account-list")).toContainText("Weekly");
    await expect(page.locator(".account-list")).toContainText("Code Review");
    await expect(page.locator(".account-list")).not.toContainText("GPT-5.4 priority");
    await expect(page.locator(".account-list")).not.toContainText("GPT-5.4 · Fast tier");
    expect(await meters.evaluateAll((items) => items.every((item) => item.scrollWidth <= item.clientWidth))).toBe(true);
    expect(await meters.evaluateAll((items) => {
      const parent = items[0]?.parentElement?.getBoundingClientRect();
      return Boolean(parent && items.every((item) => {
        const current = item.getBoundingClientRect();
        return current.left >= parent.left - 1 && current.right <= parent.right + 1;
      }));
    })).toBe(true);
    await expect(page.locator(".quota-display-menu")).toHaveCount(0);
    await page.screenshot({ path: `output/playwright/quota-windows-${viewport.width}x${viewport.height}.png` });
  });

  for (const theme of themes) {
    test(`routing distribution ru ${theme} ${viewport.width}x${viewport.height}`, async ({ page }) => {
      await installTauriMock(page, { locale: "ru", mode: "local", theme, populated: true, accountCount: 3 });
      await page.setViewportSize(viewport);
      await page.goto("/");
      await page.getByRole("button", { name: "Пул", exact: true }).click();
      await page.getByRole("button", { name: "Настройки ротации пула", exact: true }).click();
      const dialog = page.getByRole("dialog", { name: "Ротация пула" });
      await expect(dialog.getByRole("radio")).toHaveCount(3);
      await expect(dialog.getByRole("radio", { name: "Автоматически", exact: true })).toHaveAttribute("aria-checked", "true");
      await dialog.getByRole("radio", { name: "По кругу", exact: true }).click();
      await expect(dialog).not.toContainText("Закреплять один чат за аккаунтом");
      await expect(dialog).not.toContainText("Аккаунтов для повтора при ошибке");
      await expect(dialog).not.toContainText("Скорость запроса");
      await expect(dialog.getByRole("listitem")).toHaveCount(4);
      expect(await dialog.evaluate((element) => element.scrollWidth <= element.clientWidth)).toBe(true);
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= document.documentElement.clientWidth)).toBe(true);
      await page.screenshot({ path: `output/playwright/routing-distribution-ru-${theme}-${viewport.width}x${viewport.height}.png` });
    });
  }

  test(`shell disclosure controls ${viewport.width}x${viewport.height}`, async ({ page }) => {
    await installTauriMock(page, { locale: "en", mode: "local", theme: "light", populated: true });
    await page.setViewportSize(viewport);
    await page.goto("/");
    const shell = page.locator(".relay-shell");
    const modeButton = page.getByRole("button", { name: "Mode: Computer" });
    await modeButton.click();
    const modeMenu = page.getByRole("menu");
    await expect(modeMenu.getByRole("menuitemradio")).toHaveCount(2);
    await expect(modeMenu.getByRole("menuitemradio")).toHaveText(["Computer", "Relay Hosted"]);
    await expect(modeMenu.getByRole("menuitemradio", { name: "Computer" })).toHaveAttribute("aria-checked", "true");
    expect(await modeMenu.evaluate((element) => {
      const rect = element.getBoundingClientRect();
      return rect.left >= 0 && rect.right <= innerWidth && rect.top >= 36 && rect.bottom <= innerHeight;
    })).toBe(true);
    await page.screenshot({ path: `output/playwright/shell-mode-menu-${viewport.width}x${viewport.height}.png` });
    await page.keyboard.press("Escape");
    await expect(modeMenu).toBeHidden();

    if (await shell.evaluate((element) => element.classList.contains("sidebar-collapsed"))) {
      await expect(page.getByRole("button", { name: "Help" })).toBeVisible();
      await page.getByRole("button", { name: "Expand sidebar" }).click();
    } else {
      await page.getByRole("button", { name: "Collapse sidebar" }).click();
      await expect(shell).toHaveClass(/sidebar-collapsed/);
      await page.getByRole("button", { name: "Expand sidebar" }).click();
    }
    await expect(shell).not.toHaveClass(/sidebar-collapsed/);
    await expect(page.locator(".relay-sidebar nav button span").first()).toBeVisible();
    await expect(page.locator(".sidebar-version")).toHaveText(/^v\d+\.\d+\.\d+$/);
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
    await installTauriMock(page, { locale: "en", mode: "local", theme: "light", populated: true, diagnosticDebug: true });
    await page.setViewportSize(viewport);
    await page.goto("/");

    await page.getByRole("button", { name: "Connections", exact: true }).click();
    await page.getByRole("button", { name: "Sign in", exact: true }).first().click();
    let dialog = page.getByRole("dialog", { name: "Sign in" });
    await expect(dialog.getByRole("timer")).toBeVisible();
    await expect(dialog.getByText("Time remaining", { exact: true })).toBeVisible();
    await expect(dialog.getByRole("button", { name: "Copy sign-in link" })).toBeVisible();
    await page.screenshot({ path: `output/playwright/oauth-dialog-${viewport.width}x${viewport.height}.png` });
    await page.locator(".relay-modal-backdrop").click({ position: { x: 2, y: 2 } });
    await expect(dialog).toHaveCount(0);

    await page.getByRole("tab", { name: "Sources" }).click();
    const sourceActions = page.locator(".source-card .account-card-actions");
    expect(await sourceActions.locator(":scope > *").evaluateAll((items) => items.map((item) => item.getAttribute("aria-label")))).toEqual(["Remove from pool", "Refresh API data", "Edit", "Launch"]);
    await page.locator(".source-card .account-row-menu summary").click();
    const sourceMenu = page.getByRole("menu");
    await expect(sourceMenu.getByRole("menuitem")).toHaveText(["Disable", "Delete"]);
    await expect(sourceMenu.getByRole("menuitem", { name: "Delete" })).toBeVisible();
    // The popover is placed after it is inserted, so poll the settled rect
    // instead of reading one frame that may still be unplaced.
    await expect.poll(() => sourceMenu.evaluate((element) => {
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
    await page.locator(".relay-modal-backdrop").click({ position: { x: 2, y: 2 } });
    await expect(dialog).toHaveCount(0);

    await page.getByRole("button", { name: "Usage", exact: true }).click();
    await page.getByRole("button", { name: "Request details: req_synthetic_local" }).click();
    dialog = page.getByRole("dialog", { name: "Request details" });
    await dialog.locator(".request-technical-details > summary").click();
    await expect(dialog).toContainText("req_synthetic_local");
    await expect(dialog).toContainText("Selection reasonFresh quota headroom band");
    await expect(dialog).toContainText("Eligible participants4");
    await expect(dialog).toContainText("Quota at selection63.00%");
    expect(await dialog.locator(".request-details-list > div").evaluateAll((items) => items.every((item) => item.scrollWidth <= item.clientWidth))).toBe(true);
    expect(await dialog.evaluate((element) => {
      const rect = element.getBoundingClientRect();
      return rect.left >= 0 && rect.right <= innerWidth && rect.top >= 36 && rect.bottom <= innerHeight;
    })).toBe(true);
    await page.screenshot({ path: `output/playwright/request-details-dialog-${viewport.width}x${viewport.height}.png` });
    await dialog.getByRole("button", { name: "Close" }).first().click();
  });
}
