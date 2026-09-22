import { expect, test, type Page } from "../bun-playwright";
import { installTauriMock } from "./tauri-mock";

const modes = ["local", "remote", "zenith"] as const;
const themes = ["light", "dark"] as const;
const locales = ["en", "ru"] as const;
const viewports = [{ width: 1160, height: 760 }, { width: 840, height: 560 }] as const;
const TITLE_BAR_HEIGHT = 36;

for (const theme of themes) {
  for (const locale of locales) {
    for (const width of [1160, 840, 430]) {
      test(`Help reading layout ${locale} ${theme} ${width}`, async ({ page }) => {
        await page.setViewportSize({ width, height: 760 });
        await installTauriMock(page, { mode: "local", locale, theme, populated: true });
        await page.goto("/");
        await page.getByRole("button", { name: locale === "ru" ? "Помощь" : "Help", exact: true }).click();
        const article = page.locator(".help-document");
        const contents = page.locator(".help-contents");
        await expect(contents).toBeVisible();
        await expect(contents.locator("a[aria-current]")).toHaveCount(1);
        const [articleBox, contentsBox] = await Promise.all([article.boundingBox(), contents.boundingBox()]);
        expect(articleBox).not.toBeNull();
        expect(contentsBox).not.toBeNull();
        if (width > 760) expect(articleBox!.x + articleBox!.width).toBeLessThan(contentsBox!.x);
        else expect(contentsBox!.y + contentsBox!.height).toBeLessThan(articleBox!.y);
        expect(await article.evaluate((element) => element.scrollWidth <= element.clientWidth)).toBe(true);
        await page.screenshot({ path: `output/playwright/help-${locale}-${theme}-${width}.png` });
        const errors = article.getByRole("heading", { name: locale === "ru" ? "8. Ошибки" : "8. Errors", exact: true });
        await errors.evaluate((element) => element.scrollIntoView({ block: "start" }));
        await expect(contents.locator("a[aria-current]")).toHaveText(locale === "ru" ? "Ошибки" : "Errors");
        await expect(contents).toBeInViewport();
        expect(await article.locator(".help-table-wrap, table, td").evaluateAll((elements) => elements.every((element) => element.scrollWidth <= element.clientWidth))).toBe(true);
        if (width <= 760) {
          const [headingBox, menuBox] = await Promise.all([errors.boundingBox(), contents.boundingBox()]);
          expect(headingBox!.y).toBeGreaterThanOrEqual(menuBox!.y + menuBox!.height);
        }
        await page.screenshot({ path: `output/playwright/help-errors-${locale}-${theme}-${width}.png` });
        await article.getByRole("searchbox").fill("no_eligible_source");
        await expect(article.locator(".help-error-group[open]")).toHaveCount(1);
        await article.locator(".help-error-group").scrollIntoViewIfNeeded();
        expect(await article.locator(".help-table-wrap, table, td").evaluateAll((elements) => elements.every((element) => element.scrollWidth <= element.clientWidth))).toBe(true);
        await page.screenshot({ path: `output/playwright/help-error-result-${locale}-${theme}-${width}.png` });
      });
    }
  }
}

for (const viewport of viewports) {
  for (const theme of themes) {
    test(`provider credit rows stay balanced across account cards ${theme} ${viewport.width}`, async ({ page }) => {
      await installTauriMock(page, { mode: "local", locale: "ru", theme, populated: true, accountCount: 6, quotaAvailable: true, providerCredits: 498.2, freeAccountHealthy: true });
      await page.setViewportSize(viewport);
      await page.goto("/");
      const heights: number[] = [];
      for (const [tab, selector] of [["Подключения", ".account-card"], ["Пул", ".pool-member-card"]] as const) {
        await page.getByRole("button", { name: tab, exact: true }).click();
        const card = page.locator(selector).filter({ hasText: "Free reserve" });
        await expect(card.locator(".account-subscription-line")).toHaveCount(0);
        const credits = card.locator(".account-provider-quota-strip");
        await expect(credits.locator("dd")).toHaveText("498,2");
        await card.scrollIntoViewIfNeeded();
        const bounds = await credits.evaluate((row) => {
          const rect = row.getBoundingClientRect();
          const value = row.querySelector("dd")!.getBoundingClientRect();
          return { height: rect.height, top: value.top - rect.top, bottom: rect.bottom - value.bottom, fits: row.scrollWidth <= row.clientWidth };
        });
        expect(Math.abs(bounds.top - bounds.bottom)).toBeLessThanOrEqual(1);
        expect(bounds.fits).toBe(true);
        heights.push(bounds.height);
        await card.screenshot({ path: `output/playwright/credit-card-${selector.slice(1)}-${theme}-${viewport.width}.png` });
      }
      expect(heights[0]).toBe(heights[1]);
    });
  }
}

async function expectTopLevelEmptyCentered(page: Page) {
  const tabs = page.locator(".relay-page > .relay-tabs");
  const [pageBox, headerBox, tabCount, emptyBox, paddingBottom] = await Promise.all([
    page.locator(".relay-page").boundingBox(),
    page.locator(".relay-page-header").boundingBox(),
    tabs.count(),
    page.locator(".gateway-empty-tab-panel > .relay-empty, .relay-page > .relay-empty").boundingBox(),
    page.locator(".relay-page").evaluate((element) => Number.parseFloat(getComputedStyle(element).paddingBottom)),
  ]);
  const tabsBox = tabCount ? await tabs.boundingBox() : null;
  expect(pageBox).not.toBeNull();
  expect(headerBox).not.toBeNull();
  expect(emptyBox).not.toBeNull();
  const contentTop = tabsBox ? tabsBox.y + tabsBox.height : headerBox!.y + headerBox!.height;
  const availableCenter = (contentTop + pageBox!.y + pageBox!.height - paddingBottom) / 2;
  expect(Math.abs(emptyBox!.y + emptyBox!.height / 2 - availableCenter)).toBeLessThanOrEqual(2);
}

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
  expect(await summary.evaluateAll((items) => items.map((item) => Math.round(item.getBoundingClientRect().height)))).toEqual([20, 20, 20, 20]);
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
}
}

for (const theme of ["light", "dark"] as const) {
  for (const viewport of [{ width: 1344, height: 900 }, { width: 1160, height: 760 }, { width: 840, height: 560 }] as const) {
    test(`settings layout ${theme} ${viewport.width}x${viewport.height}`, async ({ page }) => {
      await installTauriMock(page, { locale: "ru", mode: "local", theme, populated: true });
      await page.setViewportSize(viewport);
      await page.goto("/");
      await page.getByRole("button", { name: "Настройки", exact: true }).click();
      await expect(page.getByText("C:\\Users\\Test\\AppData\\Local\\Zenith Relay\\data", { exact: true })).toBeVisible();
      await expect(page.getByText("C:\\Users\\Test\\AppData\\Local\\Zenith Relay\\logs", { exact: true })).toHaveCount(0);
      await expect(page.getByText("C:\\Users\\Test\\AppData\\Local\\Zenith Relay\\logs\\errors", { exact: true })).toHaveCount(0);
      await expect(page.getByText("C:\\Users\\Test\\AppData\\Local\\Zenith Relay\\logs\\crashes", { exact: true })).toHaveCount(0);
      await expect(page.getByText("C:\\Users\\Test\\AppData\\Local\\Zenith Relay\\logs\\operations", { exact: true })).toHaveCount(0);
      const groups = page.locator(".settings-group");
      const debugToggle = page.getByLabel("Режим отладки");
      await expect(debugToggle).toBeVisible();
      await expect(debugToggle).not.toBeChecked();
      const diagnostics = groups.filter({ hasText: "Диагностика" });
      await expect(diagnostics).toHaveCount(0);
      const poolData = groups.filter({ hasText: "Данные пула" });
      await expect(poolData.locator(".settings-debug-section")).toHaveCount(1);
      await expect(poolData.locator(".settings-control-row").last()).toHaveClass(/settings-danger-row/);

      const pageBox = await page.locator(".settings-page").boundingBox();
      const headerBox = await page.locator(".settings-page > .relay-page-header").boundingBox();
      const groupsBox = await page.locator(".settings-groups").boundingBox();
      expect(pageBox).not.toBeNull();
      expect(headerBox).not.toBeNull();
      expect(groupsBox).not.toBeNull();
      expect(Math.abs(groupsBox!.x + groupsBox!.width / 2 - (pageBox!.x + pageBox!.width / 2))).toBeLessThanOrEqual(2);
      const availableBottom = pageBox!.y + pageBox!.height - 32;
      const availableHeight = availableBottom - (headerBox!.y + headerBox!.height);
      if (groupsBox!.height <= availableHeight) {
        const topGap = groupsBox!.y - (headerBox!.y + headerBox!.height);
        const bottomGap = availableBottom - (groupsBox!.y + groupsBox!.height);
        expect(Math.abs(topGap - bottomGap)).toBeLessThanOrEqual(2);
      }
      await expect(groups).toHaveCount(3);
      const boxes = await groups.evaluateAll((items) => items.map((item) => {
        const rect = item.getBoundingClientRect();
        return { left: rect.left, top: rect.top, width: rect.width, overflow: item.scrollWidth - item.clientWidth };
      }));
      expect(boxes.every((box) => box.overflow === 0)).toBe(true);
      expect(Math.max(...boxes.map((box) => box.width)) - Math.min(...boxes.map((box) => box.width))).toBeLessThanOrEqual(1);
      await page.screenshot({ path: `output/playwright/settings-ru-${theme}-${viewport.width}x${viewport.height}.png` });

      await debugToggle.check();
      await expect(page.getByRole("button", { name: "Открыть операции", exact: true })).toHaveCount(1);
      await expect(diagnostics).toHaveCount(1);
      await expect(page.getByText("C:\\Users\\Test\\AppData\\Local\\Zenith Relay\\logs", { exact: true })).toBeVisible();
      await expect(page.getByText("C:\\Users\\Test\\AppData\\Local\\Zenith Relay\\logs\\errors", { exact: true })).toBeVisible();
      await expect(page.getByText("C:\\Users\\Test\\AppData\\Local\\Zenith Relay\\logs\\crashes", { exact: true })).toBeVisible();
      await expect(diagnostics.getByText("C:\\Users\\Test\\AppData\\Local\\Zenith Relay\\logs\\operations", { exact: true })).toBeVisible();
      await expect(groups).toHaveCount(4);
      await expect(groups.last()).toContainText("Диагностика");
      await debugToggle.scrollIntoViewIfNeeded();
      const debugLayout = await poolData.evaluate((item) => ({ overflow: item.scrollWidth - item.clientWidth }));
      expect(debugLayout.overflow).toBe(0);
      await poolData.screenshot({ path: `output/playwright/settings-debug-ru-${theme}-${viewport.width}x${viewport.height}.png` });
    });
  }
}

test("disabled model state stays readable in the compact dark window", async ({ page }) => {
  await installTauriMock(page, { locale: "ru", mode: "local", theme: "dark", populated: true, mixedModels: true, quotaAvailable: true, modelSpeed: { "gpt-5.4-mini": "fast" } });
  await page.setViewportSize({ width: 840, height: 560 });
  await page.goto("/");
  await page.getByRole("button", { name: "Пул", exact: true }).click();
  await page.getByRole("tab", { name: "Правила моделей" }).click();
  const table = page.locator(".model-rules-table");
  await expect(table.getByRole("columnheader")).toHaveCount(2);
  expect(await table.getByRole("columnheader").evaluateAll((cells) => cells.map((cell) => getComputedStyle(cell).textAlign))).toEqual(["left", "center"]);
  await expect(table.locator(".model-group-row").first()).toContainText("OpenAI");
  await expect(table.locator(".model-group-row").nth(1)).toContainText("Anthropic");
  await expect(table.locator(".model-group-row").filter({ hasText: "OpenAI" })).toHaveCount(1);
  await expect(table.locator(".model-group-row").first()).toContainText("2 модели");
  const model = page.locator('.model-rules tbody tr[data-model-id="gpt-5.4-mini"]');
  await model.getByRole("checkbox", { name: "Отключить gpt-5.4-mini" }).click();
  await expect(model).toHaveAttribute("data-enabled", "false");
  await expect(model.getByRole("checkbox", { name: "Включить gpt-5.4-mini" })).not.toBeChecked();
  expect(await page.locator(".model-rules tbody tr").evaluateAll((items) => items.every((item) => item.scrollWidth <= item.clientWidth))).toBe(true);
  await expect(page.locator(".model-sort-select")).toHaveCount(0);
  await page.screenshot({ path: "output/playwright/model-rules-disabled-ru-dark-840x560.png" });
});

test("model rules show backend reasoning but keep image pricing outside operational rules", async ({ page }) => {
  await installTauriMock(page, {
    locale: "ru",
    mode: "local",
    theme: "light",
    populated: true,
    serverModelOrder: ["gpt-5.4", "gpt-image-2"],
    modelReasoning: { "gpt-image-2": ["low", "medium", "high"] },
  });
  await page.setViewportSize({ width: 1268, height: 720 });
  await page.goto("/");
  await page.getByRole("button", { name: "Пул", exact: true }).click();
  await page.getByRole("tab", { name: "Правила моделей" }).click();

  const model = page.locator('.model-rules tbody tr[data-model-id="gpt-image-2"]');
  await expect(model).toBeVisible();
  await expect(model.locator('[data-model-reasoning-edit="gpt-image-2"]')).toHaveCount(1);
  await expect(model.locator(".model-image-price-summary")).toHaveCount(0);
  await expect(model.locator(".model-image-price-item")).toHaveCount(0);
  await page.screenshot({ path: "output/playwright/model-rules-image-pricing-ru-light-1268x720.png" });
});

test("sparse reference tables stay compact and centered in a wide window", async ({ page }) => {
  await installTauriMock(page, { locale: "ru", mode: "local", theme: "dark", populated: true, mixedModels: true });
  await page.setViewportSize({ width: 1648, height: 1168 });
  await page.goto("/");
  await page.getByRole("button", { name: "Пул", exact: true }).click();
  await page.getByRole("tab", { name: "Правила моделей" }).click();

  const pageBox = await page.locator(".relay-page").boundingBox();
  const modelBox = await page.locator(".model-rules.relay-compact-content").boundingBox();
  expect(pageBox).not.toBeNull();
  expect(modelBox).not.toBeNull();
  expect(modelBox!.width).toBeLessThanOrEqual(1080);
  expect(Math.abs(modelBox!.x + modelBox!.width / 2 - (pageBox!.x + pageBox!.width / 2))).toBeLessThanOrEqual(1);
  await page.screenshot({ path: "output/playwright/model-rules-centered-ru-dark-1648x1168.png" });

  await page.getByRole("button", { name: "Подключения", exact: true }).click();
  await page.getByRole("tab", { name: "Источники API" }).click();
  const sourceBoxes = await page.locator(".relay-page[data-view='sources'] > .relay-compact-content").evaluateAll((items) => items.map((item) => item.getBoundingClientRect().toJSON()));
  expect(sourceBoxes).toHaveLength(2);
  expect(sourceBoxes.every((box) => box.width <= 1080 && Math.abs(box.x + box.width / 2 - (pageBox!.x + pageBox!.width / 2)) <= 1)).toBe(true);
  await page.screenshot({ path: "output/playwright/api-sources-centered-ru-dark-1648x1168.png" });
});

test("source prices are grouped by metadata and Messages models expose cache TTLs", async ({ page }) => {
  await installTauriMock(page, {
    locale: "ru",
    mode: "local",
    theme: "dark",
    populated: true,
    mixedModels: true,
    sourceProtocolBindings: [
      { wireApi: "responses", adapter: "native", reasoningMode: "disabled", modelIds: ["gpt-5.4", "gemini-3.1-pro-preview", "grok-4.5", "glm-5.2", "private-model"] },
      { wireApi: "messages", adapter: "native", reasoningMode: "disabled", modelIds: ["claude-opus-4-8"] },
    ],
  });
  await page.setViewportSize({ width: 1280, height: 900 });
  await page.goto("/");
  await page.getByRole("button", { name: "Подключения", exact: true }).click();
  await page.getByRole("tab", { name: "Источники API" }).click();
  await page.getByRole("row").filter({ hasText: "Example compatible API" }).getByRole("button", { name: "Изменить" }).click();
  const dialog = page.getByRole("dialog", { name: "Изменить источник" });
  await dialog.getByRole("tab", { name: "Цены" }).click();
  await dialog.locator(".source-price-group > summary").filter({ hasText: "OpenAI" }).click();
  await dialog.locator(".source-price-group > summary").filter({ hasText: "Anthropic" }).click();
  await expect(dialog.locator(".member-price-grid-head").getByText("Кэш запись 5 мин", { exact: true })).toBeVisible();
  await expect(dialog.locator(".member-price-grid-head").getByText("Кэш запись 1 ч", { exact: true })).toBeVisible();
  expect(await dialog.evaluate((element) => element.scrollWidth <= element.clientWidth)).toBe(true);
  await page.screenshot({ path: "output/playwright/source-pricing-groups-ru-dark-1280x900.png" });
  await dialog.locator(".source-price-group > summary").filter({ hasText: "OpenAI" }).click();
  for (const viewport of [{ width: 840, height: 560 }, { width: 390, height: 844 }]) {
    await page.setViewportSize(viewport);
    const anthropic = dialog.locator(".source-price-group").filter({ hasText: "Anthropic" });
    await anthropic.scrollIntoViewIfNeeded();
    expect(await anthropic.locator("input").evaluateAll((inputs) => inputs.every((input) => {
      const rect = input.getBoundingClientRect();
      return rect.left >= 0 && rect.right <= innerWidth && rect.width >= 60;
    }))).toBe(true);
    expect(await dialog.locator(".relay-dialog-body").evaluate((body) => body.scrollWidth <= body.clientWidth)).toBe(true);
    await page.screenshot({ path: `output/playwright/source-cache-prices-ru-dark-${viewport.width}.png` });
  }
});

test("source editor keeps discovery compact and routing controls internal", async ({ page }) => {
  await installTauriMock(page, { locale: "en", mode: "local", theme: "light", populated: true, mixedModels: true });
  await page.setViewportSize({ width: 1160, height: 760 });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await page.getByRole("tab", { name: "Sources" }).click();
  await page.getByRole("row").filter({ hasText: "Example compatible API" }).getByRole("button", { name: "Edit" }).click();
  const dialog = page.getByRole("dialog", { name: "Edit source" });
  await expect(dialog.locator('[role="tablist"]').first().getByRole("tab")).toHaveText(["General", "Pricing"]);
  await expect(dialog.getByLabel("Name", { exact: true })).toBeVisible();
  await expect(dialog.locator(".source-protocol-availability")).toHaveCount(0);
  await expect(dialog.locator(".source-add-adapters")).toHaveCount(0);
  for (const size of [{ width: 1160, height: 760 }, { width: 390, height: 844 }]) {
    await page.setViewportSize(size);
    expect(await dialog.evaluate((element) => {
      const rect = element.getBoundingClientRect();
      return rect.left >= 0 && rect.right <= innerWidth && rect.top >= 0 && rect.bottom <= innerHeight && element.scrollWidth <= element.clientWidth;
    })).toBe(true);
    await page.screenshot({ path: `output/playwright/source-discovery-general-${size.width}.png` });
  }
  await dialog.getByRole("tab", { name: "Pricing", exact: true }).click();
  await expect(dialog.locator(".source-price-tab")).toBeVisible();
  await page.screenshot({ path: "output/playwright/source-discovery-pricing.png" });
});

test("reasoning modes stay balanced with the full backend level set", async ({ page }) => {
  await installTauriMock(page, {
    locale: "en",
    mode: "local",
    theme: "dark",
    populated: true,
    mixedModels: true,
    modelReasoning: { "claude-opus-4-8": ["none", "low", "medium", "high", "xhigh", "max"] },
  });
  await page.setViewportSize({ width: 1160, height: 760 });
  await page.goto("/");
  await page.getByRole("button", { name: "Pool", exact: true }).click();
  await page.getByRole("tab", { name: "Model Rules" }).click();
  const model = page.locator('.model-rules tbody tr[data-model-id="claude-opus-4-8"]');
  await model.getByRole("button", { name: "Set reasoning modes for claude-opus-4-8" }).click();

  const dialog = page.getByRole("dialog", { name: "Reasoning modes" });
  const options = dialog.locator(".model-reasoning-options button");
  await expect(options).toHaveCount(6);
  expect(await options.evaluateAll((items) => new Set(items.map((item) => Math.round(item.getBoundingClientRect().top))).size)).toBe(1);
  expect(await dialog.evaluate((element) => element.scrollWidth <= element.clientWidth)).toBe(true);
  await page.screenshot({ path: "output/playwright/model-reasoning-dialog-ru-dark-1160x760.png" });

  await page.setViewportSize({ width: 520, height: 620 });
  expect(await options.evaluateAll((items) => new Set(items.map((item) => Math.round(item.getBoundingClientRect().top))).size)).toBe(2);
  expect(await dialog.evaluate((element) => {
    const rect = element.getBoundingClientRect();
    return rect.left >= 0 && rect.right <= innerWidth && rect.top >= 36 && rect.bottom <= innerHeight && element.scrollWidth <= element.clientWidth;
  })).toBe(true);
  await page.screenshot({ path: "output/playwright/model-reasoning-dialog-ru-dark-520x620.png" });
});

test("prompt cache policy fits the Russian dark source editor", async ({ page }) => {
  await installTauriMock(page, {
    locale: "ru",
    mode: "local",
    theme: "dark",
    populated: true,
    sourceProtocolBindings: [{
      wireApi: "messages",
      adapter: "native",
      reasoningMode: "disabled",
      cacheWriteTtl: "1h",
      modelIds: [],
    }],
  });
  await page.setViewportSize({ width: 840, height: 560 });
  await page.goto("/");
  await page.getByRole("button", { name: "Подключения", exact: true }).click();
  await page.getByRole("tab", { name: "Источники API" }).click();
  await page.getByRole("row").filter({ hasText: "Example compatible API" }).getByRole("button", { name: "Изменить" }).click();
  const dialog = page.getByRole("dialog", { name: "Изменить источник" });
  await expect(dialog.locator(".source-add-adapters")).toHaveCount(0);
  expect(await dialog.evaluate((element) => {
    const rect = element.getBoundingClientRect();
    return rect.left >= 0 && rect.right <= innerWidth && rect.top >= 36 && rect.bottom <= innerHeight && element.scrollWidth <= element.clientWidth;
  })).toBe(true);
  await page.screenshot({ path: "output/playwright/source-cache-policy-ru-dark-840x560.png" });

  await page.setViewportSize({ width: 390, height: 844 });
  expect(await dialog.evaluate((element) => element.scrollWidth <= element.clientWidth)).toBe(true);
  await page.screenshot({ path: "output/playwright/source-cache-policy-ru-dark-390x844.png" });
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
    const accountSearch = dialog.getByLabel("Найти подключение");
    await accountSearch.fill("pro");
    await expect(dialog.locator(".pool-member-options > label").first()).toContainText("Pro account");
    const planBadge = dialog.locator(".pool-member-options .account-plan-badge");
    await expect(planBadge).toHaveText("Pro");
    expect(await planBadge.evaluate((badge) => badge.scrollWidth <= badge.clientWidth && badge.scrollHeight <= badge.clientHeight)).toBe(true);
    await page.screenshot({ path: `output/playwright/pool-add-pro-ru-dark-${viewport.width}x${viewport.height}.png` });
    await accountSearch.fill("");
    await dialog.getByText("Business Workspace", { exact: true }).click();
    await dialog.getByRole("button", { name: "Добавить выбранные (1)" }).click();

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
    await expect(headerActions.locator(".pool-preset-actions").getByRole("button", { name: "Сохранить пресет", exact: true })).toBeVisible();
    await expect(headerActions.locator(".pool-preset-actions").getByRole("button", { name: "Применить пресет", exact: true })).toBeVisible();
    await expect(headerActions.locator("button").evaluateAll((buttons) => buttons.map((button) => button.getAttribute("aria-label")))).resolves.toEqual(["Сохранить пресет", "Применить пресет", "Добавить участника", "Подключить"]);
    await expect(headerActions.getByRole("button", { name: "Добавить участника", exact: true })).toBeVisible();
    await expect(headerActions.getByRole("button", { name: "Запустить пул", exact: true })).toHaveCount(0);
    await expect(headerActions.getByRole("button", { name: "Подключить", exact: true })).toBeDisabled();
    const actionBoxes = await headerActions.locator(":scope > .relay-button").evaluateAll((buttons) => buttons.map((button) => {
      const rect = button.getBoundingClientRect();
      return { width: rect.width, height: rect.height, overflow: button.scrollWidth - button.clientWidth };
    }));
    expect(Math.max(...actionBoxes.map((box) => box.height)) - Math.min(...actionBoxes.map((box) => box.height))).toBeLessThanOrEqual(1);
    expect(actionBoxes.every((box) => box.height <= 34 && box.overflow === 0)).toBe(true);
    expect(actionBoxes.reduce((total, box) => total + box.width, 0)).toBeLessThan(380);
    await page.screenshot({ path: `output/playwright/pool-header-actions-ru-dark-${viewport.width}x${viewport.height}.png` });

    await expect(page.locator(".pool-sort-menu")).toHaveCount(0);
    await expect(page.locator(".pool-priority-label")).toContainText("Порядок использования");
    await expect(page.getByRole("button", { name: "Настройки ротации пула", exact: true })).toBeVisible();
    const poolToolbarGroups = page.locator(".pool-quota-actions > .pool-control-group");
    await expect(poolToolbarGroups).toHaveCount(2);
    await expect(poolToolbarGroups.evaluateAll((groups) => groups.map((group) => group.getAttribute("data-toolbar-group")))).resolves.toEqual(["routing", "refresh"]);
    await expect(poolToolbarGroups.nth(0).getByRole("slider", { name: "Скорость запроса" })).toBeVisible();
    await expect(poolToolbarGroups.nth(0).locator("button").evaluateAll((buttons) => buttons.map((button) => button.getAttribute("aria-label")))).resolves.toEqual(["Настройки ротации пула"]);
    await expect(poolToolbarGroups.nth(1).locator(":scope > *")).toHaveCount(2);
    await expect(poolToolbarGroups.nth(1).getByRole("button")).toHaveCount(2);
    await page.screenshot({ path: `output/playwright/pool-priority-ru-dark-${viewport.width}x${viewport.height}.png` });
    expect(await page.locator(".pool-summary > div").evaluateAll((cells) => cells.every((cell) => {
      const cellRect = cell.getBoundingClientRect();
      const center = (cellRect.top + cellRect.bottom) / 2;
      return Array.from(cell.children).every((child) => {
        const rect = child.getBoundingClientRect();
        return Math.abs((rect.top + rect.bottom) / 2 - center) <= 1;
      });
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
    const speedSlider = speed.getByRole("slider", { name: "Скорость запроса" });
    await expect(speedSlider).toHaveAttribute("aria-valuetext", "Обычная");
    await expect(speedSlider).toHaveValue("0");
    await expect(speed.locator(".pool-speed-current")).toHaveText("Обычная");
    await speedSlider.press("ArrowRight");
    await expect(speedSlider).toBeEnabled();
    await expect(speed).toHaveAttribute("data-speed-tier", "fast");
    await expect(speed.locator(".pool-speed-current")).toHaveText("Быстрая");
    await page.screenshot({ path: `output/playwright/pool-speed-slider-ru-dark-${viewport.width}x${viewport.height}.png` });
    await page.getByRole("button", { name: "Настройки ротации пула", exact: true }).click();
    const distribution = page.getByRole("dialog", { name: "Ротация пула" });
    await expect(distribution).not.toContainText("Скорость запроса");
    await distribution.getByRole("radio", { name: "По порядку", exact: true }).click();
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
      const indicator = getComputedStyle(element, "::before");
      return indicator.content !== "none" && Math.abs(Number.parseFloat(indicator.width) - element.clientWidth) <= 2;
    })).toBe(true);
    await expect(page.locator(".pool-summary > div")).toHaveCount(4);
    await expect(members.getByText("Pro account", { exact: true })).toBeVisible();
    await expect(members.locator('.account-plan-badge[data-plan="pro"]')).toHaveText("Pro");
    const apiCard = members.locator('.pool-member-card[data-member-kind="source"]');
    await expect(apiCard).toContainText("42,50");
    await expect(apiCard).toContainText("7,50");
    await expect(apiCard).toContainText("128");
    await expect(apiCard.getByRole("button", { name: "Обновить баланс" })).toBeVisible();
    await expect(apiCard.locator(".pool-member-runtime-meta")).toContainText("Режим работы");
    await expect(apiCard.locator(".pool-member-runtime-meta")).toContainText("Параллельность");
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
    await expect(members.locator('.account-plan-badge[data-plan="pro"]')).toHaveText("Pro");
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
    await expect(page.locator(".proxy-storage-counts")).toContainText("Total3");
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
    await expect(menu.getByRole("menuitem")).toHaveText(["Proxy: Common", "Export", "Disable", "Delete"]);
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
    await expect(backup.locator(".account-kind-icon")).toHaveAttribute("aria-label", "Ошибка подключения");
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
    // The pool button is a call to action, not a state label.
    await expect(cards.locator('.account-card-actions > .relay-icon-button:first-child[aria-label="Убрать из пула"]')).toHaveCount(3);

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
      await expect(dialog.getByRole("radio", { name: "Умный выбор", exact: true })).toHaveAttribute("aria-checked", "true");
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
    await expect(modeMenu.getByRole("menuitemradio")).toHaveCount(3);
    await expect(modeMenu.getByRole("menuitemradio")).toHaveText(["Computer", "Choose API", "On your server"]);
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
    await expect(page.locator(".sidebar-help-copy small")).toHaveText(/^v\d+\.\d+\.\d+$/);
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
    await expect(dialog.getByText("Time remaining", { exact: true })).toBeVisible();
    await expect(dialog.getByRole("button", { name: "Copy sign-in link" })).toBeVisible();
    await page.screenshot({ path: `output/playwright/oauth-dialog-${viewport.width}x${viewport.height}.png` });
    await dialog.getByRole("button", { name: "Close" }).click();

    await page.getByRole("tab", { name: "Sources" }).click();
    const sourceActions = page.locator(".relay-table .row-actions");
    expect(await sourceActions.locator(":scope > *").evaluateAll((items) => items.map((item) => item.tagName === "DETAILS" ? item.querySelector("summary")?.getAttribute("aria-label") : item.getAttribute("aria-label")))).toEqual(["Launch", "Edit", "Actions"]);
    await sourceActions.locator("summary").click();
    const sourceMenu = page.getByRole("menu");
    await expect(sourceMenu.getByRole("menuitem")).toHaveCount(4);
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
    await dialog.getByRole("tab", { name: "Route", exact: true }).click();
    await expect(dialog).toContainText("Selection reasonGreatest quota remaining");
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
  test(`empty profile recovery is centered ${viewport.width}x${viewport.height}`, async ({ page }) => {
    await installTauriMock(page, { locale: "ru", mode: "local", theme: "dark", populated: true });
    await page.setViewportSize(viewport);
    await page.goto("/");
    await page.getByRole("button", { name: "Восстановление", exact: true }).click();

    const recovery = page.locator(".profile-recovery.is-empty");
    await expect(recovery.locator(".profile-recovery-empty-state")).toBeVisible();
    // Read every rectangle in one layout pass: separate boundingBox() calls can straddle a reflow and disagree.
    const readMetrics = () => recovery.evaluate((element) => {
      const sections = Array.from(element.querySelectorAll(":scope > .profile-recovery-section"));
      const emptyState = element.querySelector(".profile-recovery-empty-state");
      if (sections.length !== 1 || !emptyState) return null;
      const box = element.getBoundingClientRect();
      const sectionBoxes = sections.map((section) => section.getBoundingClientRect());
      const contentCenter = (Math.min(...sectionBoxes.map((rect) => rect.top)) + Math.max(...sectionBoxes.map((rect) => rect.bottom))) / 2;
      return { centerOffset: Math.abs(contentCenter - (box.top + box.height / 2)), bottomOverflow: emptyState.getBoundingClientRect().bottom - box.bottom, horizontalOverflow: element.scrollWidth - element.clientWidth };
    });
    await expect.poll(async () => (await readMetrics())?.centerOffset).toBeLessThanOrEqual(2);
    const metrics = await readMetrics();
    expect(metrics).not.toBeNull();
    expect(metrics!.bottomOverflow).toBeLessThanOrEqual(1);
    expect(metrics!.horizontalOverflow).toBeLessThanOrEqual(0);
    await page.screenshot({ path: `output/playwright/profile-recovery-empty-ru-dark-${viewport.width}x${viewport.height}.png` });

    await page.getByRole("tab", { name: "OpenCode", exact: true }).click();
    const openCodeRecovery = page.locator(".profile-recovery-opencode.is-empty");
    await expect(openCodeRecovery.locator(".profile-recovery-empty-state")).toBeVisible();
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
    await expect(switches).toHaveCount(4);
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

test("remote connection choices use clear switches in a centered dialog", async ({ page }) => {
  await installTauriMock(page, { locale: "ru", mode: "remote", theme: "dark", populated: true, remoteConnected: false });
  await page.setViewportSize({ width: 840, height: 560 });
  await page.goto("/");
  await page.getByRole("button", { name: "Подключения", exact: true }).click();
  await page.getByRole("button", { name: "Подключить существующий сервер" }).click();
  await page.getByLabel("Адрес сервера").fill("http://127.0.0.1:14999");

  const dialog = page.getByRole("dialog", { name: "Подключить существующий сервер" });
  await expect(dialog.locator(".setting-toggle")).toHaveCount(2);
  await expect(dialog.getByLabel("Разрешить HTTP без шифрования")).toBeVisible();
  await expect(dialog.getByLabel("Доверять новой идентичности")).toBeVisible();
  await expect(dialog.getByText("Токен и трафик передаются открыто. Используйте только в доверенной локальной сети.", { exact: true })).toBeVisible();
  const [backdropBox, dialogBox] = await Promise.all([page.locator(".relay-modal-backdrop").boundingBox(), dialog.boundingBox()]);
  expect(backdropBox).not.toBeNull();
  expect(dialogBox).not.toBeNull();
  expect(Math.abs(dialogBox!.y + dialogBox!.height / 2 - (backdropBox!.y + backdropBox!.height / 2))).toBeLessThanOrEqual(2);
  expect(await dialog.locator(".setting-toggle").evaluateAll((rows) => rows.every((row) => row.scrollWidth <= row.clientWidth))).toBe(true);
  await page.screenshot({ path: "output/playwright/remote-connect-options-ru-dark-840x560.png" });
});

test("unconfigured API empty state uses the available page center", async ({ page }) => {
  await installTauriMock(page, { locale: "ru", mode: "remote", theme: "dark", populated: true, remoteConnected: false });
  await page.setViewportSize({ width: 1160, height: 760 });
  await page.goto("/");
  await page.getByRole("button", { name: "API", exact: true }).click();
  await expect(page.getByText("API не настроен", { exact: true })).toBeVisible();
  await expectTopLevelEmptyCentered(page);
  await page.screenshot({ path: "output/playwright/gateway-empty-centered-ru-dark-1160x760.png" });
});

test("unsupported usage empty state uses the available page center", async ({ page }) => {
  await installTauriMock(page, { locale: "ru", mode: "remote", theme: "dark", populated: true, remoteFeatures: ["accounts"] });
  await page.setViewportSize({ width: 1160, height: 760 });
  await page.goto("/");
  await page.getByRole("button", { name: "Использование", exact: true }).click();
  await expect(page.getByText("Не поддерживается", { exact: true })).toBeVisible();
  await expectTopLevelEmptyCentered(page);
  await page.screenshot({ path: "output/playwright/usage-unsupported-centered-ru-dark-1160x760.png" });
});

test("automation list fits the standard window without horizontal scrolling", async ({ page }) => {
  await installTauriMock(page, { locale: "ru", mode: "local", theme: "dark", populated: true });
  await page.setViewportSize({ width: 1160, height: 760 });
  await page.goto("/");
  await page.getByRole("button", { name: "Подключения", exact: true }).click();
  await page.getByRole("tab", { name: "Автоматизация" }).click();
  const table = page.locator(".automation-list");
  expect(await table.evaluate((element) => element.scrollWidth <= element.clientWidth)).toBe(true);
});

test("weekly quota reset confirmation stays concise in the compact dark window", async ({ page }) => {
  await installTauriMock(page, { locale: "ru", mode: "local", theme: "dark", populated: true, resetCreditsAvailable: 1 });
  await page.setViewportSize({ width: 840, height: 560 });
  await page.goto("/");
  await page.getByRole("button", { name: "Подключения", exact: true }).click();
  await page.getByRole("button", { name: "Доступен сброс: 1 · Сбросить недельную квоту", exact: true }).click();

  const dialog = page.getByRole("dialog", { name: "Сбросить недельную квоту" });
  await expect(dialog.getByText("Сбросить недельную квоту для этой учётной записи?", { exact: true })).toBeVisible();
  await expect(dialog.getByRole("button", { name: "Нет", exact: true })).toBeVisible();
  await expect(dialog.getByRole("button", { name: "Да, сбросить", exact: true })).toBeVisible();
  expect(await dialog.evaluate((element) => {
    const rect = element.getBoundingClientRect();
    return rect.left >= 0 && rect.right <= innerWidth && rect.top >= 36 && rect.bottom <= innerHeight
      && element.scrollWidth <= element.clientWidth;
  })).toBe(true);
  await page.screenshot({ path: "output/playwright/reset-confirm-after-ru-dark-840x560.png" });
});

test("automation editor fits the compact window without hidden controls", async ({ page }) => {
  await installTauriMock(page, { locale: "ru", mode: "local", theme: "dark", populated: true, accountCount: 3 });
  await page.setViewportSize({ width: 840, height: 560 });
  await page.goto("/");
  await page.getByRole("button", { name: "Подключения", exact: true }).click();
  await page.getByRole("tab", { name: "Автоматизация" }).click();
  await page.getByRole("button", { name: "Добавить автоматизацию" }).click();

  const dialog = page.getByRole("dialog", { name: "Добавить автоматизацию" });
  await expect(dialog.getByText("После восстановления основной квоты", { exact: true })).toBeVisible();
  await expect(dialog.getByText("Цель", { exact: true })).toHaveCount(0);
  await expect(dialog.getByText("Выполнение", { exact: true })).toHaveCount(0);
  await expect(dialog.getByText("Самая лёгкая поддерживаемая", { exact: true })).toHaveCount(0);
  await dialog.getByRole("button", { name: /^Модель:/ }).click();
  await expect(page.locator('[role="option"][data-value="gpt-5.4"]')).toBeVisible();
  await expect(page.locator('[role="option"][data-value="gpt-5.4-mini"]')).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(dialog.getByRole("button", { name: "Автоматически" })).toHaveCount(0);
  await expect(dialog.getByRole("button", { name: "Вручную" })).toHaveCount(0);
  expect(await dialog.evaluate((element) => {
    const rect = element.getBoundingClientRect();
    const body = element.querySelector(".relay-dialog-body");
    const execution = element.querySelector("footer");
    const bodyRect = body?.getBoundingClientRect();
    const executionRect = execution?.getBoundingClientRect();
    return rect.left >= 0 && rect.right <= innerWidth && rect.top >= 36 && rect.bottom <= innerHeight
      && element.scrollWidth <= element.clientWidth
      && Boolean(body && body.scrollHeight <= body.clientHeight)
      && Boolean(bodyRect && executionRect && executionRect.top >= bodyRect.bottom && executionRect.bottom <= innerHeight);
  })).toBe(true);
  await page.screenshot({ path: "output/playwright/automation-dialog-ru-dark-840x560.png" });

  await dialog.getByRole("button", { name: /^Аккаунты:/ }).click();
  await page.locator('[role="option"][data-value="account_ids"]').click();
  await dialog.getByLabel("Personal Plus").check();
  await dialog.getByLabel("Backup account").check();
  await expect(dialog.getByRole("button", { name: "Модель: gpt-5.4-mini" })).toBeVisible();
  expect(await dialog.evaluate((element) => {
    const rect = element.getBoundingClientRect();
    const body = element.querySelector(".relay-dialog-body");
    const execution = element.querySelector("footer");
    const bodyRect = body?.getBoundingClientRect();
    const executionRect = execution?.getBoundingClientRect();
    return rect.left >= 0 && rect.right <= innerWidth && rect.top >= 36 && rect.bottom <= innerHeight
      && element.scrollWidth <= element.clientWidth
      && Boolean(body && body.scrollHeight <= body.clientHeight)
      && Boolean(bodyRect && executionRect && executionRect.top >= bodyRect.bottom && executionRect.bottom <= innerHeight);
  })).toBe(true);
  await page.screenshot({ path: "output/playwright/automation-dialog-selected-ru-dark-840x560.png" });
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
  await expect(dialog).not.toContainText("Приоритет при равенстве");
  await expect(dialog).not.toContainText("Доля трафика");
  await expect(dialog.getByRole("tab", { name: "Модели", exact: true })).toHaveAttribute("aria-selected", "true");
  await expect(dialog.locator("[data-member-model-id]")).toHaveCount(2);
  expect(await dialog.locator("[data-member-model-id]").evaluateAll((rows) => rows.every((row) => row.scrollWidth <= row.clientWidth))).toBe(true);
  await page.screenshot({ path: "output/playwright/pool-member-dialog-ru-840x560.png" });
  await dialog.getByRole("button", { name: "Закрыть" }).first().click();

  await page.locator(".relay-sidebar nav button").nth(4).click();
  await page.getByRole("button", { name: "Сведения о запросе: req_synthetic_local" }).click();
  dialog = page.getByRole("dialog", { name: "Сведения о запросе" });
  await expect(dialog).toContainText("req_synthetic_local");
  await expect(dialog).toContainText("РазмышлениеЗапрошено: Максимальное, передано: Низкое");
  await dialog.getByRole("tab", { name: "Маршрут", exact: true }).click();
  await expect(dialog).toContainText("Причина выбораНаибольший остаток квоты");
  await expect(dialog).toContainText("Доступных участников4");
  await expect(dialog).toContainText("Квота при выборе63.00%");
  expect(await dialog.locator(".request-details-list > div").evaluateAll((items) => items.every((item) => item.scrollWidth <= item.clientWidth))).toBe(true);
  expect(await dialog.evaluate((element) => {
    const rect = element.getBoundingClientRect();
    return rect.left >= 0 && rect.right <= innerWidth && rect.top >= 36 && rect.bottom <= innerHeight;
  })).toBe(true);
  await page.screenshot({ path: "output/playwright/request-details-dialog-ru-840x560.png" });
  await dialog.getByRole("button", { name: "Закрыть" }).first().click();
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

    if (scenario.width === 840) {
      await page.locator(".usage-metrics > div").nth(2).evaluate((card) => {
        card.querySelector("strong")!.textContent = "49,3 млн";
        card.querySelector("small")!.textContent = "Вх. 49,2 млн · Кэш ↓ 43,6 млн · Вых. 137,3 тыс.";
      });
      await page.locator(".usage-metrics > div").nth(3).evaluate((card) => {
        card.querySelector("strong")!.textContent = "≈54,0431 $";
        card.querySelector("small")!.textContent = "Оценено токенов: 49,3 млн";
      });
    }
    await expect(page.getByRole("columnheader", { name: scenario.label })).toBeVisible();
    expect(await page.locator(".usage-metrics").evaluate((grid) => getComputedStyle(grid).gridTemplateColumns.split(" ").length)).toBe(scenario.width === 840 ? 2 : 3);
    if (scenario.width === 1160) {
      expect(await page.locator(".usage-metrics > div").evaluateAll((cards) => new Set(cards.map((card) => Math.round(card.getBoundingClientRect().top))).size)).toBe(2);
    }
    expect(await page.locator(".usage-metrics > div").evaluateAll((cards) => cards.every((card) => card.scrollWidth <= card.clientWidth))).toBe(true);
    expect(await page.locator(".usage-overview strong").evaluateAll((values) => new Set(values.map((value) => getComputedStyle(value).fontSize)).size)).toBe(1);
    expect(await page.locator(".usage-metrics > div").evaluateAll((items) => items.every((item) => getComputedStyle(item).textAlign === "center"))).toBe(true);
    expect(await page.locator(".usage-request-table th, .usage-request-table td").evaluateAll((items) => items.every((item) => getComputedStyle(item).textAlign === "center"))).toBe(true);
    const timing = page.getByRole("row").filter({ hasText: "req_synthetic_local" }).locator('td[data-column="timing"]');
    await expect(timing).toHaveText(scenario.locale === "ru" ? "128 мс / 428 мс" : "128 ms / 428 ms");
    expect(await timing.evaluate((element) => element.scrollWidth <= element.clientWidth)).toBe(true);
    const clippedHeaders = await page.locator(".usage-request-table th").evaluateAll((items) => items.filter((item) => item.scrollWidth > item.clientWidth || item.scrollHeight > item.clientHeight).map((item) => item.textContent));
    expect(clippedHeaders).toEqual([]);
    const clippedHeaderLabels = await page.locator(".usage-request-table .usage-column-heading > span").evaluateAll((items) => items.filter((item) => item.scrollWidth > item.clientWidth).map((item) => item.textContent));
    expect(clippedHeaderLabels).toEqual([]);
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
    await expect(page.locator(".usage-range-menu").getByRole("button", { name: /^Период:/ })).toBeVisible();
    await expect(filters.getByRole("button", { name: /^Модель:/ })).toBeVisible();
    await expect(filters.getByRole("button", { name: /^Участник пула:/ })).toBeVisible();
    await expect(filters.getByLabel("Локальный ключ")).toHaveCount(0);
    await filters.getByRole("button", { name: "Другие фильтры" }).click();
    await expect(filters.getByLabel("Локальный ключ")).toHaveCount(0);
    await expect(filters.getByRole("button", { name: /^Категория ошибки:/ })).toBeVisible();
    const protocol = filters.getByRole("button", { name: /^Протокол:/ });
    await protocol.click();
    await expect(protocol).toHaveAttribute("aria-expanded", "true");
    await page.getByRole("option", { name: "Responses", exact: true }).click();
    await expect(filters.locator(".usage-filter-toggle-wrap small")).toHaveText("1");
    await expect(filters.getByRole("button", { name: "Сбросить фильтры" })).toBeVisible();
    await filters.getByRole("button", { name: "Сбросить фильтры" }).click();
    await expect(filters.getByRole("button", { name: "Протокол: Любой протокол" })).toBeVisible();
    await expect(filters.getByRole("button", { name: "Сбросить фильтры" })).toHaveCount(0);
    await page.screenshot({ path: `output/playwright/usage-filters-open-ru-dark-${viewport.width}x${viewport.height}.png` });

    await page.getByRole("tab", { name: "Модели" }).click();
    const aggregate = page.locator(".usage-aggregate-table");
    await expect(aggregate.getByRole("columnheader")).toHaveText(["Модель", "Запросы", "Входные токены", "Выходные токены", "Прочитано из кэша", "API-экв."]);
    expect(await aggregate.locator("th, td").evaluateAll((cells) => cells.every((cell) => cell.scrollWidth <= cell.clientWidth && cell.scrollHeight <= cell.clientHeight + 1))).toBe(true);
    expect(await aggregate.locator("xpath=..").evaluate((element) => element.scrollWidth <= element.clientWidth)).toBe(true);
    await page.screenshot({ path: `output/playwright/usage-models-ru-dark-${viewport.width}x${viewport.height}.png` });
  });
}
