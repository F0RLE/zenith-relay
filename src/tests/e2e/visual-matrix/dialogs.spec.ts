import { expect, test } from "../../bun-playwright";
import { installTauriMock } from "../tauri-mock";
import { themes, viewports, TITLE_BAR_HEIGHT } from "./fixtures";

for (const scenario of [
  { locale: "en" as const, theme: "light" as const, viewport: viewports[0], section: "Routing", note: "Faster parallel fallback" },
  { locale: "en" as const, theme: "dark" as const, viewport: viewports[1], section: "Routing", note: "Faster parallel fallback" },
  { locale: "ru" as const, theme: "light" as const, viewport: viewports[0], section: "Маршрутизация", note: "Ускорено параллельное переключение" },
  { locale: "ru" as const, theme: "dark" as const, viewport: viewports[1], section: "Маршрутизация", note: "Ускорено параллельное переключение" },
]) {
  test(`manual update dialog ${scenario.locale} ${scenario.theme} ${scenario.viewport.width}x${scenario.viewport.height}`, async ({ page }) => {
    const updateBody = [
      "## Downloads",
      "",
      "Installer links are not part of the in-app changelog.",
      "<!-- relay-notes:en -->",
      "## [1.1.3] - 2026-09-04",
      "",
      "A clearer and more reliable update experience.",
      "",
      "### Routing",
      "",
      "- Faster parallel fallback",
      "- More accurate availability",
      "<!-- relay-notes:ru -->",
      "## [1.1.3] - 2026-09-04",
      "",
      "Обновление стало понятнее и надёжнее.",
      "",
      "### Маршрутизация",
      "",
      "- Ускорено параллельное переключение",
      "- Точнее отображается доступность",
    ].join("\n");
    await installTauriMock(page, { locale: scenario.locale, mode: "local", theme: scenario.theme, populated: true, bundleType: null, updateVersion: "1.1.3", updateBody });
    await page.setViewportSize(scenario.viewport);
    await page.goto("/");
    const openLabel = scenario.locale === "ru" ? "Открыть обновление 1.1.3" : "Open update 1.1.3";
    const updateButton = page.getByRole("button", { name: openLabel });
    await expect(updateButton).toBeVisible();
    await expect(updateButton).toHaveText(scenario.locale === "ru" ? "Доступно обновление" : "Update available");
    await expect(updateButton).not.toContainText("1.1.3");
    expect(await page.locator(".sidebar-bottom").evaluate((element) => {
      const feedback = element.querySelector(".sidebar-feedback")?.getBoundingClientRect();
      const update = element.querySelector(".sidebar-update-row")?.getBoundingClientRect();
      const footer = element.querySelector(".sidebar-footer")?.getBoundingClientRect();
      return Boolean(update && footer && update.bottom <= footer.top && (!feedback || feedback.bottom <= update.top));
    })).toBe(true);
    if (scenario.locale === "ru" && scenario.theme === "dark" && scenario.viewport.width === 840) {
      const collapse = page.locator(".sidebar-footer .relay-icon-button");
      await expect(page.locator(".relay-shell")).toHaveClass(/sidebar-collapsed/);
      expect(await updateButton.evaluate((element) => getComputedStyle(element, "::after").content)).toBe("none");
      await page.screenshot({ path: "output/playwright/sidebar-update-collapsed-ru-dark-840x560.png" });
      await collapse.click();
      await expect(page.locator(".relay-shell")).not.toHaveClass(/sidebar-collapsed/);
      await page.screenshot({ path: "output/playwright/sidebar-update-ru-dark-840x560.png" });
    }
    await updateButton.click();
    const dialogName = scenario.locale === "ru" ? "Обновление 1.1.3" : "Update 1.1.3";
    const dialog = page.getByRole("dialog", { name: dialogName });
    await expect(dialog.getByRole("heading", { name: scenario.section })).toBeVisible();
    await expect(dialog.getByRole("listitem").first()).toHaveText(scenario.note);
    await expect(dialog).not.toContainText("Installer links");
    await expect(dialog).not.toContainText("## [1.1.3]");
    await expect(dialog.getByRole("button", { name: scenario.locale === "ru" ? "Пропустить 1.1.3" : "Skip 1.1.3" })).toBeVisible();
    await expect(dialog.getByRole("button", { name: scenario.locale === "ru" ? "Обновить" : "Update", exact: true })).toBeVisible();
    expect(await dialog.evaluate((element) => {
      const rect = element.getBoundingClientRect();
      const body = element.querySelector(".relay-dialog-body");
      return rect.left >= 0 && rect.right <= innerWidth && rect.top >= 36 && rect.bottom <= innerHeight
        && Boolean(body && body.scrollWidth <= body.clientWidth);
    })).toBe(true);
    await page.screenshot({ path: `output/playwright/update-dialog-${scenario.locale}-${scenario.theme}-${scenario.viewport.width}x${scenario.viewport.height}.png` });
  });
}

test("Windows titlebar controls stay visible in the light theme", async ({ page }) => {
  await installTauriMock(page, { locale: "ru", mode: "local", theme: "light", populated: true });
  await page.setViewportSize({ width: 1160, height: 760 });
  await page.goto("/");
  const maximize = page.getByRole("button", { name: "Развернуть" });
  await maximize.hover();
  expect(await maximize.evaluate((element) => getComputedStyle(element).color)).not.toBe("rgb(255, 255, 255)");
  await expect(maximize.locator("svg")).toBeVisible();
  await page.screenshot({ path: "output/playwright/titlebar-controls-ru-light-hover.png" });
});

for (const theme of themes) {
  for (const viewport of viewports) {
    test(`app context menu ${theme} ${viewport.width}x${viewport.height}`, async ({ page }) => {
      await page.context().grantPermissions(["clipboard-read", "clipboard-write"], { origin: "http://127.0.0.1:1420" });
      await installTauriMock(page, { locale: "ru", mode: "local", theme, populated: true });
      await page.setViewportSize(viewport);
      await page.goto("/");
      await page.getByRole("button", { name: "Подключения", exact: true }).click();
      const search = page.getByPlaceholder("Поиск").first();
      await search.fill("Business");
      await search.evaluate((element) => {
        const input = element as HTMLInputElement;
        input.setSelectionRange(0, 4);
        const rect = input.getBoundingClientRect();
        input.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true, clientX: rect.left + 24, clientY: rect.bottom - 6 }));
      });

      const menu = page.getByRole("menu", { name: "Контекстное меню" });
      await expect(menu).toBeVisible();
      await expect(menu.getByRole("menuitem")).toHaveCount(4);
      expect(await menu.evaluate((element) => {
        const rect = element.getBoundingClientRect();
        return rect.left >= 0 && rect.right <= innerWidth && rect.top >= 36 && rect.bottom <= innerHeight;
      })).toBe(true);
      await page.screenshot({ path: `output/playwright/context-menu-ru-${theme}-${viewport.width}x${viewport.height}.png` });

      await menu.getByRole("menuitem", { name: "Вырезать" }).click();
      await expect(search).toHaveValue("ness");
      expect(await page.evaluate(() => navigator.clipboard.readText())).toBe("Busi");
      await search.evaluate((element) => {
        const input = element as HTMLInputElement;
        const rect = input.getBoundingClientRect();
        input.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true, clientX: rect.left + 24, clientY: rect.bottom - 6 }));
      });
      await expect(menu).toBeVisible();
      await menu.getByRole("menuitem", { name: "Вставить" }).click();
      await expect(search).toHaveValue("Business");
      await search.evaluate((element) => {
        const input = element as HTMLInputElement;
        const rect = input.getBoundingClientRect();
        input.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true, clientX: rect.left + 24, clientY: rect.bottom - 6 }));
      });
      await expect(menu).toBeVisible();
      await menu.getByRole("menuitem", { name: "Выделить всё" }).click();
      expect(await search.evaluate((element) => ({ start: (element as HTMLInputElement).selectionStart, end: (element as HTMLInputElement).selectionEnd }))).toEqual({ start: 0, end: 8 });
      await search.evaluate((element) => {
        const input = element as HTMLInputElement;
        const rect = input.getBoundingClientRect();
        input.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true, clientX: rect.left + 24, clientY: rect.bottom - 6 }));
      });
      await expect(menu).toBeVisible();
      await page.keyboard.press("Escape");
      await expect(menu).toBeHidden();
      await expect(search).toBeFocused();

      await search.evaluate((element) => {
        const input = element as HTMLInputElement;
        input.setSelectionRange(0, 0);
        input.blur();
        window.getSelection()?.removeAllRanges();
      });
      expect(await page.locator(".relay-page-header").evaluate((element) => {
        const event = new MouseEvent("contextmenu", { bubbles: true, cancelable: true, clientX: 300, clientY: 80 });
        element.dispatchEvent(event);
        return event.defaultPrevented;
      })).toBe(true);
      await expect(menu).toBeHidden();
    });
  }
}

for (const viewport of viewports) {
  test(`profile switch ${viewport.width}x${viewport.height}`, async ({ page }) => {
    await installTauriMock(page, { locale: "ru", mode: "local", theme: "dark", populated: true });
    await page.setViewportSize(viewport);
    await page.goto("/");
    await page.getByRole("button", { name: "Подключения", exact: true }).click();
    await page.getByRole("button", { name: "Запустить в ChatGPT" }).click();

    await expect(page.getByText("Клиент запущен.")).toBeVisible();
    await expect(page.getByRole("dialog", { name: /видимость чатов/i })).toHaveCount(0);
    const commands = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string }> }).__TAURI_TEST_INVOKES__.map((item) => item.command).filter((command) => ["launch_codex_account", "launch_managed_codex_profile"].includes(command)));
    expect(commands).toEqual(["launch_codex_account", "launch_managed_codex_profile"]);
    await page.screenshot({ path: `output/playwright/profile-switch-ru-dark-${viewport.width}x${viewport.height}.png` });
  });
}

for (const scenario of [
  { name: "expanded", viewport: { width: 1160, height: 760 }, collapsed: false },
  { name: "compact", viewport: { width: 840, height: 560 }, collapsed: true },
] as const) {
  test(`feedback error opens without layout shift ${scenario.name}`, async ({ page }) => {
    await installTauriMock(page, { locale: "en", mode: "local", theme: "dark", populated: true, gatewayRunning: true, profileSwitchError: true });
    await page.setViewportSize(scenario.viewport);
    await page.goto("/");
    await page.getByRole("button", { name: "Pool", exact: true }).click();
    await page.getByRole("button", { name: "Connect", exact: true }).click();
    await page.getByRole("dialog", { name: "What do you want to connect?" }).getByRole("button", { name: "ChatGPT", exact: true }).click();

    const shell = page.locator(".relay-shell");
    const feedback = page.locator(".global-feedback.error");
    await expect(feedback).toContainText("Something went wrong. Click to view details.");
    await expect(feedback).not.toContainText("The profile changed during the operation.");
    await expect(feedback).not.toContainText("profile_restore_blocked");
    if (scenario.collapsed) await expect(shell).toHaveClass(/sidebar-collapsed/);
    else await expect(shell).not.toHaveClass(/sidebar-collapsed/);

    const readGeometry = () => feedback.evaluate((element) => {
      const box = element.getBoundingClientRect();
      const header = document.querySelector<HTMLElement>(".relay-page-header")!.getBoundingClientRect();
      const message = element.querySelector<HTMLElement>(".global-feedback-message")!;
      const messageBox = message.getBoundingClientRect();
      const style = getComputedStyle(message);
      return {
        x: box.x,
        y: box.y,
        width: box.width,
        height: box.height,
        headerY: header.y,
        messageHidden: messageBox.width <= 1 && messageBox.height <= 1 && style.clipPath === "inset(50%)",
      };
    });
    const initialGeometry = await readGeometry();
    const footerBox = await page.locator(".sidebar-footer").boundingBox();
    expect(footerBox).not.toBeNull();
    expect(initialGeometry.y + initialGeometry.height).toBeLessThanOrEqual(footerBox!.y + 1);
    if (scenario.collapsed) {
      expect(initialGeometry.width).toBeLessThanOrEqual(38);
      expect(initialGeometry.messageHidden).toBe(true);
    } else {
      expect(initialGeometry.width).toBeGreaterThan(89);
      expect(initialGeometry.messageHidden).toBe(false);
    }

    await feedback.hover();
    await expect.poll(async () => Math.abs(((await feedback.boundingBox())?.width ?? 0) - initialGeometry.width) <= 1, { timeout: 1_000 }).toBe(true);
    const hoveredGeometry = await readGeometry();
    expect(Math.abs(hoveredGeometry.x - initialGeometry.x)).toBeLessThanOrEqual(1);
    expect(Math.abs(hoveredGeometry.y - initialGeometry.y)).toBeLessThanOrEqual(1);
    expect(Math.abs(hoveredGeometry.height - initialGeometry.height)).toBeLessThanOrEqual(1);
    expect(Math.abs(hoveredGeometry.headerY - initialGeometry.headerY)).toBeLessThanOrEqual(1);
    await expect(page.locator(".global-feedback-details")).toHaveCount(0);

    await feedback.locator(".global-feedback-error-trigger").click();
    const details = page.getByRole("dialog", { name: "Error details" });
    await expect(details).toBeVisible();
    const detailsGeometry = await details.evaluate((element, titleBarHeight) => {
      const detailsBox = element.getBoundingClientRect();
      const centerOffset = Math.abs((detailsBox.left + detailsBox.width / 2) - innerWidth / 2);
      const verticalCenterOffset = Math.abs((detailsBox.top + detailsBox.height / 2) - (innerHeight + titleBarHeight) / 2);
      return {
        withinViewport: detailsBox.left >= 0 && detailsBox.right <= innerWidth && detailsBox.top >= titleBarHeight && detailsBox.bottom <= innerHeight,
        centered: centerOffset <= 2 && verticalCenterOffset <= 2,
      };
    }, TITLE_BAR_HEIGHT);
    expect(detailsGeometry).toEqual({ withinViewport: true, centered: true });
    await expect(page.locator(".global-feedback")).toHaveCount(0);
    await expect(page.locator(".global-feedback-error-trigger")).toHaveCount(0);
    await page.screenshot({ path: `output/playwright/feedback-error-details-${scenario.name}.png` });
    await details.locator("header .relay-icon-button").click();
    await expect(page.locator(".mode-picker > button")).toBeFocused();
    await expect(feedback).toHaveCount(0);
  });
}

for (const viewport of viewports) {
  test(`empty profile recovery follows the workspace ${viewport.width}x${viewport.height}`, async ({ page }) => {
    await installTauriMock(page, { locale: "ru", mode: "local", theme: "dark", populated: true });
    await page.setViewportSize(viewport);
    await page.goto("/");
    await page.getByRole("button", { name: "Восстановление", exact: true }).click();

    const recovery = page.locator(".profile-recovery.is-empty");
    const panel = recovery.locator(".profile-recovery-panel");
    await expect(panel).toBeVisible();
    await expect(recovery.locator(":scope > .relay-empty")).toBeVisible();
    await expect(recovery.locator(".profile-recovery-empty-state")).toHaveCount(0);
    const readMetrics = () => recovery.evaluate((element) => {
      const page = element.closest(".relay-page");
      const header = page?.querySelector(".relay-page-header");
      const panel = element.querySelector(".profile-recovery-panel");
      const empty = element.querySelector(":scope > .relay-empty");
      if (!page || !header || !panel || !empty) return null;
      const pageBox = page.getBoundingClientRect();
      const headerBox = header.getBoundingClientRect();
      const panelBox = panel.getBoundingClientRect();
      const emptyBox = empty.getBoundingClientRect();
      return {
        panelAligns: Math.abs(panelBox.left - headerBox.left) <= 1 && Math.abs(panelBox.right - headerBox.right) <= 1,
        belowHeader: panelBox.top >= headerBox.bottom - 1 && panelBox.top - headerBox.bottom <= 24,
        emptyBelow: emptyBox.top >= panelBox.bottom - 1,
        withinPage: panelBox.left >= pageBox.left && panelBox.right <= pageBox.right + 1,
        horizontalOverflow: element.scrollWidth - element.clientWidth,
      };
    });
    await expect.poll(async () => (await readMetrics())?.panelAligns).toBe(true);
    const metrics = await readMetrics();
    expect(metrics).toEqual({ panelAligns: true, belowHeader: true, emptyBelow: true, withinPage: true, horizontalOverflow: 0 });
    await page.screenshot({ path: `output/playwright/profile-recovery-empty-ru-dark-${viewport.width}x${viewport.height}.png` });

    await page.getByRole("tab", { name: "OpenCode", exact: true }).click();
    const openCodeRecovery = page.locator(".profile-recovery-opencode.is-empty");
    await expect(openCodeRecovery.locator(".profile-recovery-panel")).toBeVisible();
    await expect(openCodeRecovery.locator(":scope > .relay-empty")).toBeVisible();
    await page.screenshot({ path: `output/playwright/profile-recovery-opencode-empty-ru-dark-${viewport.width}x${viewport.height}.png` });
  });
}

for (const theme of themes) {
  for (const viewport of viewports) {
    test(`ChatGPT pool account setup ${theme} ${viewport.width}x${viewport.height}`, async ({ page }) => {
      await installTauriMock(page, { locale: "ru", mode: "local", theme, populated: true, accountCount: 4 });
      await page.setViewportSize(viewport);
      await page.goto("/");
      await page.getByRole("button", { name: "API", exact: true }).click();
      const connectionPanel = page.locator(".gateway-api-connection-panel");
      const endpointValue = connectionPanel.locator(".gateway-api-address");
      const portControl = connectionPanel.locator(".gateway-api-port-control");
      await expect(connectionPanel).toBeVisible();
      await expect(page.getByText("В сети", { exact: true })).toHaveCount(0);
      await expect(page.getByRole("button", { name: "Остановить API" })).toHaveClass(/secondary/);
      const portInput = page.getByRole("spinbutton", { name: "Порт" });
      const portSave = page.getByRole("button", { name: "Сохранить и перезапустить" });
      const [endpointBox, portControlBox, portBox, saveBox] = await Promise.all([
        endpointValue.boundingBox(),
        portControl.boundingBox(),
        portInput.boundingBox(),
        portSave.boundingBox(),
      ]);
      expect(endpointBox).not.toBeNull();
      expect(portControlBox).not.toBeNull();
      expect(portBox).not.toBeNull();
      expect(saveBox).not.toBeNull();
      expect(portControlBox!.y).toBeGreaterThan(endpointBox!.y + endpointBox!.height);
      expect(Math.abs(portBox!.y - saveBox!.y)).toBeLessThanOrEqual(2);
      await page.screenshot({ path: `output/playwright/gateway-api-connection-ru-${theme}-${viewport.width}x${viewport.height}.png` });

      await page.getByRole("tab", { name: "ChatGPT", exact: true }).click();
      const setup = page.locator(".client-oauth-binding");
      await expect(setup.getByRole("heading", { name: "Аккаунт ChatGPT" })).toBeVisible();
      await expect(setup.getByRole("button", { name: /^Аккаунт:/ })).toHaveAttribute("data-value", "auto");
      await expect(setup.getByRole("checkbox", { name: "Резерв 1%" })).toBeChecked();
      await expect(setup.getByText("Сохранять последний 1% выбранного аккаунта для прямого запуска ChatGPT.", { exact: true })).toBeVisible();
      const accountSelect = setup.getByRole("button", { name: /^Аккаунт:/ });
      const switchButton = setup.getByRole("button", { name: "Переключить" });
      const reserveToggle = setup.getByRole("checkbox", { name: "Резерв 1%" });
      const [accountBox, switchBox, reserveBox] = await Promise.all([accountSelect.boundingBox(), switchButton.boundingBox(), reserveToggle.boundingBox()]);
      expect(accountBox).not.toBeNull();
      expect(switchBox).not.toBeNull();
      expect(reserveBox).not.toBeNull();
      expect(switchBox!.x).toBeGreaterThanOrEqual(accountBox!.x + accountBox!.width + 8);
      expect(reserveBox!.y).toBeGreaterThanOrEqual(accountBox!.y + accountBox!.height + 8);
      expect(reserveBox!.width).toBeGreaterThanOrEqual(34);
      expect(reserveBox!.height).toBeGreaterThanOrEqual(20);
      const switchEdges = await page.locator('.gateway-tab-panel input[type="checkbox"]').evaluateAll((inputs) =>
        inputs.map((input) => input.getBoundingClientRect().right),
      );
      expect(Math.max(...switchEdges) - Math.min(...switchEdges)).toBeLessThanOrEqual(1);
      expect(await page.locator(".codex-feature-control").evaluateAll((rows) => rows.every((row) => {
        const heading = row.querySelector(".codex-feature-heading")!.getBoundingClientRect();
        const control = row.querySelector(".setting-toggle")!.getBoundingClientRect();
        return row.scrollWidth <= row.clientWidth && heading.right + 12 <= control.left;
      }))).toBe(true);
      await setup.getByRole("button", { name: /^Аккаунт:/ }).click();
      await page.locator('[role="option"][data-value="account_synthetic_2"]').click();
      await expect(setup.getByRole("button", { name: /^Аккаунт:/ })).toHaveAttribute("data-value", "account_synthetic_2");
      await expect(setup.locator(".oauth-binding-selection-hint")).toHaveCount(0);
      await expect(setup.locator(".oauth-binding-outcome")).toHaveCount(0);
      expect(await setup.evaluate((element) => {
        const rect = element.getBoundingClientRect();
        return rect.left >= 0 && rect.right <= innerWidth && rect.top >= 36 && rect.bottom <= innerHeight;
      })).toBe(true);
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= document.documentElement.clientWidth)).toBe(true);
      expect(await setup.evaluate((element) => element.scrollWidth <= element.clientWidth)).toBe(true);
      await page.mouse.move(1, 1);
      await page.waitForTimeout(180);
      await page.screenshot({ path: `output/playwright/codex-pool-account-ru-${theme}-${viewport.width}x${viewport.height}.png` });
    });
  }
}

for (const theme of themes) {
  test(`ChatGPT flat controls stay aligned in a narrow ${theme} window`, async ({ page }) => {
    await page.setViewportSize({ width: 390, height: 844 });
    await installTauriMock(page, { locale: "ru", mode: "local", theme, populated: true });
    await page.goto("/");
    await page.getByRole("button", { name: "API", exact: true }).click();
    await page.getByRole("tab", { name: "ChatGPT", exact: true }).click();
    const panel = page.getByRole("tabpanel", { name: "ChatGPT", exact: true });
    const switches = panel.getByRole("checkbox");
    await expect(switches).toHaveCount(3);
    await expect(panel.locator(".codex-background-tasks-control").getByRole("checkbox")).toHaveCount(1);
    await expect(panel.locator(".codex-websockets-control").getByRole("checkbox")).toHaveCount(1);
    const edges = await switches.evaluateAll((inputs) => inputs.map((input) => input.getBoundingClientRect().right));
    expect(Math.max(...edges) - Math.min(...edges)).toBeLessThanOrEqual(1);
    expect(await panel.evaluate((element) => [...element.querySelectorAll<HTMLElement>(".gateway-account-panel, .gateway-setting-row, .oauth-binding-settings")]
      .every((row) => row.scrollWidth <= row.clientWidth))).toBe(true);
    const reserve = panel.getByRole("checkbox", { name: "Резерв 1%" });
    await reserve.focus();
    await reserve.press("Space");
    await expect(reserve).not.toBeChecked();
    await page.screenshot({ path: `output/playwright/chatgpt-settings-${theme}-390x844.png` });
  });
}
