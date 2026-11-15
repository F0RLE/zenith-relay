import { expect, test, type Page } from "@playwright/test";
import { readFile } from "node:fs/promises";

test.beforeEach(async ({ page }) => { await page.goto("/preview.html?page=usage"); });

test("refresh adds no unavailable-server notice and preserves displayed usage", async ({ page }) => {
  const summary = page.getByRole("region", { name: "Сводка использования", exact: true });
  await expect(summary).toBeVisible();
  const totals = await summary.textContent();
  const rows = page.locator(".request-table tbody tr");
  const records = await rows.allTextContents();
  const refresh = page.getByRole("button", { name: "Обновить использование", exact: true });
  await expect(page.locator(".usage-filters .usage-refresh")).toBeVisible();
  await expect(page.locator(".usage-page > header").getByRole("button", { name: "Обновить использование", exact: true })).toHaveCount(0);
  const settings = page.getByRole("button", { name: "Дополнительные фильтры", exact: true });
  if (await settings.isVisible()) {
    const left = (await settings.boundingBox())!;
    const right = (await refresh.boundingBox())!;
    expect(right.x - left.x - left.width).toBe(8);
    expect(right.y).toBe(left.y);
    expect(right.width).toBe(left.width);
    expect(right.height).toBe(left.height);
  }
  await refresh.click();
  await refresh.press("Enter");
  await expect(page.getByText("Обновление с сервера пока недоступно.", { exact: true })).toHaveCount(0);
  await expect(page.locator(".usage-page .form-note[role='status']")).toHaveCount(0);
  await expect(summary).toHaveText(totals!);
  await expect(rows).toHaveText(records);
});

test("usage summary centers every label and value without overflow", async ({ page }, testInfo) => {
  const summary = page.getByRole("region", { name: "Сводка использования", exact: true });
  const cells = summary.locator(".usage-metrics > .usage-metric, .usage-token-totals > div, .usage-secondary-metrics > .usage-metric");
  await expect(cells).toHaveCount(10);
  const issues = await cells.evaluateAll(elements => elements.flatMap(cell => {
    const bounds = cell.getBoundingClientRect();
    return Array.from(cell.querySelectorAll(".usage-metric-copy > span, .usage-metric-copy > strong, :scope > dt, :scope > dd")).flatMap(item => {
      const rect = item.getBoundingClientRect();
      const centered = Math.abs(rect.x + rect.width / 2 - bounds.x - bounds.width / 2) <= 1;
      const fits = item.scrollWidth <= item.clientWidth + 1 && rect.left >= bounds.left && rect.right <= bounds.right;
      return centered && fits && getComputedStyle(item).textAlign === "center" ? [] : [{ text: item.textContent, centered, fits, alignment: getComputedStyle(item).textAlign }];
    });
  }));
  expect(issues).toEqual([]);
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  await summary.screenshot({ path: testInfo.outputPath("usage-summary-centered.png") });
});

async function showExtraFilters(page: Page) {
  const more = page.getByRole("button", { name: "Дополнительные фильтры", exact: true });
  if (await more.isVisible() && await more.getAttribute("aria-expanded") !== "true") await more.click();
}

test("hosted request window uses a compact charge-first layout with customer-only network details", async ({ page }, testInfo) => {
  const errors: string[] = [];
  page.on("pageerror", error => errors.push(error.message));
  await page.getByRole("button", { name: "Детали запроса demo_request_0003", exact: true }).click();
  const dialog = page.getByRole("dialog");
  await expect(dialog.getByRole("heading", { name: "Сведения о запросе" })).toBeVisible();
  const tokens = dialog.getByRole("region", { name: "Токены" });
  await expect(tokens).toBeVisible();
  await expect(tokens.locator(".token-charge")).toHaveCount(5);
  await expect(dialog.getByTestId("request-charge-total")).toHaveText("0,001251 $");
  await expect(dialog.getByText("Списано", { exact: true })).toHaveCount(1);
  await expect(dialog.getByRole("img", { name: "Завершён", exact: true })).toBeVisible();
  await expect(dialog.locator(".request-details-subtitle")).toContainText("API-ключ");
  await expect(dialog.locator(".request-details-metrics dt")).toHaveText(["Списано", "Первый вывод", "Время ответа", "Скорость генерации"]);
  await expect(dialog.locator(".request-details-identity .request-details-subtitle")).toBeVisible();
  await expect(dialog.locator(".request-token-total > div")).toHaveCSS("justify-content", "flex-start");
  await expect(tokens.locator(".request-token-header > span")).toHaveText(["Количество", "Стоимость"]);
  await expect(dialog.locator(".request-details-export")).toHaveCSS("min-height", testInfo.project.use.viewport!.width < 640 ? "44px" : "40px");
  const identity = (await dialog.locator(".request-details-model").boundingBox())!;
  const subtitle = (await dialog.locator(".request-details-subtitle").boundingBox())!;
  expect(subtitle.y).toBeGreaterThanOrEqual(identity.y + identity.height);
  await expect(dialog.locator(".request-parameters-section")).toHaveCount(0);
  await expect(dialog).not.toContainText("Эквивалент API");
  const technical = dialog.locator(".request-technical-details");
  await expect(technical).not.toHaveAttribute("open");
  await expect(dialog.locator(".request-id code")).not.toBeVisible();
  const sections = await dialog.locator(".dialog-body > *").evaluateAll(elements => elements.map(element => ({ name: element.className, top: element.getBoundingClientRect().top })));
  expect(sections.map(section => section.name)).toEqual(["request-overview", "request-token-details", "request-technical-details"]);
  expect(sections.every((section, index) => index === 0 || section.top > sections[index - 1]!.top)).toBe(true);
  const metrics = await dialog.locator(".request-details-metrics > div").evaluateAll(elements => elements.map(element => Math.round(element.getBoundingClientRect().top)));
  expect(new Set(metrics).size).toBe(testInfo.project.use.viewport!.width < 640 ? 2 : 1);
  const tokenRows = await tokens.locator(".token-heading, .token-row").evaluateAll(elements => elements.map(element => {
    const values = [...element.querySelectorAll("dd")].map(value => value.getBoundingClientRect());
    return { countCenter: values[0]!.top + values[0]!.height / 2, chargeCenter: values[1]!.top + values[1]!.height / 2, countRight: values[0]!.right, chargeRight: values[1]!.right, overflow: element.scrollWidth > element.clientWidth };
  }));
  for (const row of tokenRows) {
    expect(Math.abs(row.countCenter - row.chargeCenter)).toBeLessThanOrEqual(1);
    expect(Math.abs(row.countRight - tokenRows[0]!.countRight)).toBeLessThanOrEqual(1);
    expect(Math.abs(row.chargeRight - tokenRows[0]!.chargeRight)).toBeLessThanOrEqual(1);
    expect(row.overflow).toBe(false);
  }
  await expect(technical).toHaveCSS("margin-top", "0px");
  await expect(page.locator(".relay-app")).toHaveJSProperty("scrollWidth", await page.locator(".relay-app").evaluate(element => element.clientWidth));
  await expect(dialog).toHaveJSProperty("scrollWidth", await dialog.evaluate(element => element.clientWidth));
  await dialog.screenshot({ path: testInfo.outputPath("request-details-hosted.png") });
  await technical.locator("summary").click();
  await expect(technical).toHaveAttribute("open", "");
  await expect(dialog.locator(".request-id code")).toHaveText("demo_request_0003");
  await expect(dialog.locator(".request-id code")).toBeVisible();
  await expect(dialog.locator(".request-id code")).toHaveCSS("user-select", "all");
  await expect(dialog.getByRole("region", { name: "Тариф на момент запроса" })).toHaveCount(0);
  await expect(dialog.getByRole("region", { name: "Размер запроса" })).toHaveCount(0);
  await expect(dialog.getByRole("region", { name: "Инструменты", exact: true })).toHaveCount(0);
  await expect(technical.locator("dt")).toHaveText(["ID запроса", "Модель", "Протокол", "Транспорт приложения", "Режим обслуживания", "Рассуждения"]);
  await expect(technical).toContainText("Gemini");
  await expect(technical).toContainText("HTTP");
  await expect(technical).toContainText("Обычная");
  await expect(technical).toContainText("Не запрашивалось");
  await expect(dialog.getByRole("button", { name: "Скопировать ID запроса", exact: true })).toBeVisible();
  await expect(dialog).not.toContainText("Локальный");
  await expect(dialog).not.toContainText("Участник пула");
  await expect(dialog).toHaveJSProperty("scrollWidth", await dialog.evaluate(element => element.clientWidth));
  await technical.locator("summary").click();
  const downloadEvent = page.waitForEvent("download");
  await dialog.getByRole("button", { name: "Скачать диагностику запроса", exact: true }).click();
  const download = await downloadEvent;
  expect(download.suggestedFilename()).toBe("relay-request-demo_request_0003.txt");
  const diagnostic = await readFile((await download.path())!, "utf8");
  expect(diagnostic).toContain("ID: demo_request_0003");
  expect(diagnostic).toContain("Списано: 0,001251 USD");
  expect(diagnostic).toContain("API-ключ: Bot · demo_…3B8");
  expect(diagnostic).not.toMatch(/authorization|apiKey|userAgent|providerCredential|bodyBytes|tauri/i);
  await page.keyboard.press("Escape");
  await expect(dialog).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Детали запроса demo_request_0003", exact: true })).toBeFocused();
  expect(errors).toEqual([]);
});

test("long model and key labels keep request context below the identity without clipping", async ({ page }) => {
  await page.getByRole("button", { name: "Детали запроса demo_request_0003", exact: true }).click();
  const dialog = page.getByRole("dialog");
  // Synthetic text extremes exercise the layout without changing accounting fixtures.
  await dialog.locator(".request-details-model h3").evaluate(element => { element.textContent = "Gemini long model name / experimental reasoning edition"; });
  await dialog.locator(".request-details-subtitle dd").first().evaluate(element => { element.textContent = "Ключ для рабочего приложения с длинным названием"; });
  const identity = (await dialog.locator(".request-details-model").boundingBox())!;
  const context = (await dialog.locator(".request-details-subtitle").boundingBox())!;
  const exportButton = (await dialog.locator(".request-details-export").boundingBox())!;
  expect(context.y).toBeGreaterThanOrEqual(identity.y + identity.height);
  expect(identity.x + identity.width).toBeLessThanOrEqual(exportButton.x);
  expect(await dialog.locator(".request-details-header, .request-details-identity, .request-details-subtitle").evaluateAll(elements => elements.every(element => element.scrollWidth <= element.clientWidth))).toBe(true);
  await expect(dialog.getByTestId("request-charge-total")).toHaveText("0,001251 $");
  await expect(dialog).toHaveJSProperty("scrollWidth", await dialog.evaluate(element => element.clientWidth));
});

test("running and completed-but-pending requests never look free", async ({ page }) => {
  await page.getByRole("button", { name: "Детали запроса demo_request_0001", exact: true }).click();
  const dialog = page.getByRole("dialog");
  await expect(dialog.getByRole("heading", { name: "Запрос выполняется" })).toBeVisible();
  await expect(dialog.getByTestId("request-charge-total")).toHaveText("—");
  await expect(dialog).not.toContainText("Удержано");
  await dialog.getByRole("button", { name: "Закрыть", exact: true }).click();
  await page.getByRole("button", { name: "Детали запроса demo_request_0002", exact: true }).click();
  await expect(dialog).toContainText("Ответ завершён, расчёт ожидается");
  await expect(dialog).toContainText("Ожидаются итоговые счётчики и сумма");
  await expect(dialog.getByTestId("request-charge-total")).toHaveText("—");
  await dialog.locator(".request-technical-details > summary").click();
  await expect(dialog.locator(".request-technical-body")).toContainText("Ожидает данных");
});

test("filters survive detail close; key groups drill into requests", async ({ page }) => {
  await page.getByRole("combobox", { name: "API-ключ", exact: true }).click();
  await page.getByRole("option", { name: "Editor", exact: true }).click();
  await page.getByRole("button", { name: "Детали запроса demo_request_0001", exact: true }).click();
  await page.getByRole("button", { name: "Закрыть", exact: true }).click();
  await expect(page.getByRole("combobox", { name: "API-ключ", exact: true })).toHaveText("Editor");
  await showExtraFilters(page);
  await page.getByRole("button", { name: "Сбросить фильтры", exact: true }).click();
  await page.getByRole("group", { name: "Представление использования" }).getByRole("button", { name: "API-ключи", exact: true }).click();
  await page.getByRole("button", { name: "My project", exact: true }).click();
  await expect(page.getByRole("combobox", { name: "API-ключ", exact: true })).toHaveText("My project");
  await showExtraFilters(page);
  await page.getByLabel("ID запроса или клиент").fill("not-a-real-request");
  await expect(page.getByText("Запросов не найдено", { exact: true })).toBeVisible();
});

test("failed request preserves consumed charge and network failure details", async ({ page }) => {
  await page.getByRole("group", { name: "Представление использования" }).getByRole("button", { name: "Ошибки", exact: true }).click();
  await page.getByRole("button", { name: /Детали запроса demo_request_/ }).first().click();
  const dialog = page.getByRole("dialog");
  await expect(dialog).toContainText("Перед повтором проверьте результат");
  await expect(dialog.getByTestId("request-charge-total")).not.toHaveText("—");
  await expect(dialog.locator(".request-technical-details")).not.toHaveAttribute("open");
  await dialog.locator("summary").filter({ hasText: "Технические детали" }).click();
  await expect(dialog).toContainText("HTTP-статус");
  await expect(dialog).toContainText("response_interrupted");
  await expect(dialog.locator(".request-id code")).toBeVisible();
});

test("normal technical details stay compact and keyboard accessible even for tool requests", async ({ page }) => {
  await page.getByRole("combobox", { name: "API-ключ", exact: true }).click();
  await page.getByRole("option", { name: "Editor", exact: true }).click();
  await page.getByRole("button", { name: "Детали запроса demo_request_0010", exact: true }).click();
  const dialog = page.getByRole("dialog");
  const summary = dialog.locator(".request-technical-details > summary");
  await summary.focus();
  await page.keyboard.press("Enter");
  await expect(dialog.locator(".request-technical-body dt")).toHaveText(["ID запроса", "Модель", "Протокол", "Транспорт приложения", "Режим обслуживания", "Рассуждения"]);
  await expect(dialog.getByRole("region", { name: "Инструменты", exact: true })).toHaveCount(0);
  await expect(dialog.getByRole("region", { name: "Размер запроса", exact: true })).toHaveCount(0);
  await expect(dialog).not.toContainText("Сессия кэша");
  expect(await page.evaluate(() => "__TAURI_INTERNALS__" in window)).toBe(false);
  await page.keyboard.press("Escape");
  await expect(dialog).toHaveCount(0);
});

test("real entry is not the synthetic cabinet", async ({ page }) => {
  await page.goto("/");
  await expect(page.getByRole("heading", { level: 1 })).toHaveText("Нейросети. В ваших приложениях.");
  await expect(page.locator(".request-weave[role='img']")).toBeVisible();
  await expect(page.locator(".relay-shell, .request-table")).toHaveCount(0);
  await page.goto("/login");
  await expect(page.getByRole("heading", { name: "Relay Hosted", exact: true })).toBeVisible();
  await expect(page.getByText("Интеграция с Hosted API в разработке.", { exact: true })).toBeVisible();
  await expect(page.locator(".request-table")).toHaveCount(0);
});

test("compact desktop filters and outlined fields retain their theme borders", async ({ page }, testInfo) => {
  const border = await page.locator(".usage-page .table-shell").evaluate(element => getComputedStyle(element).borderTopColor);
  const controls = page.locator(".usage-page .pagination .icon-button");
  expect(await controls.count()).toBe(2);
  // Action buttons use the shared control edge, while input fields use the panel border.
  const controlBorder = await controls.first().evaluate(element => {
    const probe = document.createElement("span");
    probe.style.borderColor = "var(--control-edge)";
    element.parentElement!.append(probe);
    const color = getComputedStyle(probe).borderTopColor;
    probe.remove();
    return color;
  });
  expect(await controls.evaluateAll((elements, expected) => elements.every(element => getComputedStyle(element).borderTopColor === expected), controlBorder)).toBe(true);
  const filters = page.locator(".usage-filters .option-trigger:visible");
  const more = page.getByRole("button", { name: "Дополнительные фильтры", exact: true });
  await expect(filters).toHaveCount(await more.isVisible() ? 4 : 7);
  expect(await filters.evaluateAll((elements, expected) => elements.every(element => getComputedStyle(element).borderTopColor === expected), border)).toBe(true);
  if (await more.isVisible()) await expect(more).toHaveCSS("border-top-color", controlBorder);
  await showExtraFilters(page);
  const search = page.getByRole("searchbox", { name: "ID запроса или клиент", exact: true });
  await expect(search).toHaveCSS("border-top-color", border);
  await page.screenshot({ path: testInfo.outputPath("soft-control-borders.png") });
  await page.getByRole("button", { name: "Баланс: 42,80 USD", exact: true }).click();
  await expect(page.getByRole("spinbutton", { name: "Сумма, USD", exact: true })).toHaveCSS("border-top-color", "rgba(204, 222, 198, 0.4)");
  await expect(page.getByRole("dialog")).toHaveCount(0);
});

test("filters use available width and expose reset only for changed filters", async ({ page }, testInfo) => {
  const toolbar = page.locator(".usage-filters");
  const more = page.getByRole("button", { name: "Дополнительные фильтры", exact: true });
  const reset = page.getByRole("button", { name: "Сбросить фильтры", exact: true });
  await expect(toolbar).toBeVisible();
  await expect(toolbar.locator('[data-icon="close"]')).toHaveCount(0);
  await expect(reset).toHaveCount(0);
  const checkLayout = async () => {
    const layout = await toolbar.evaluate(element => {
      const visible = (field: Element) => field.getClientRects().length > 0;
      const fields = [...element.querySelectorAll(".option-trigger, input")].filter(visible).map(field => field.getBoundingClientRect());
      const action = element.querySelector(".usage-filter-trigger")!;
      const labels = [...element.querySelectorAll(".option-trigger > span")].filter(visible);
      return { width: element.clientWidth, mobile: innerWidth < 640, rows: new Set(fields.map(field => Math.round(field.top))).size, actionVisible: visible(action), actionTop: action.getBoundingClientRect().top, keyTop: fields[3]!.top, overflow: element.scrollWidth > element.clientWidth, labelsFit: labels.every(label => label.scrollWidth <= label.clientWidth) };
    });
    expect(layout.rows).toBe(layout.mobile ? 3 : layout.width > 760 ? 1 : 2);
    expect(layout.labelsFit).toBe(true);
    if (layout.actionVisible) expect(Math.abs(layout.actionTop - layout.keyTop)).toBeLessThanOrEqual(1);
    expect(layout.overflow).toBe(false);
  };
  await checkLayout();
  if (testInfo.project.name === "desktop-light") {
    await page.setViewportSize({ width: 940, height: 1000 });
    await expect(page.locator(".relay-sidebar")).toHaveCSS("width", "56px");
    await checkLayout();
    await page.getByRole("button", { name: "Развернуть меню", exact: true }).click();
    await checkLayout();
  }
  await more.click();
  await expect(reset).toHaveCount(0);
  await page.getByRole("searchbox", { name: "ID запроса или клиент", exact: true }).fill("not-a-real-request");
  await expect(page.getByText("Запросов не найдено", { exact: true })).toBeVisible();
  await expect(reset).toBeVisible();
  await more.click();
  await expect(more).toHaveAttribute("data-active", "true");
  await expect(reset).not.toBeVisible();
  await more.click();
  await reset.focus();
  await page.keyboard.press("Enter");
  await expect(more).toBeFocused();
  await expect(more).toHaveAttribute("aria-expanded", "false");
  await expect(more).not.toHaveAttribute("data-active", "true");
  await expect(reset).toHaveCount(0);
  await expect(page.locator(".usage-page .pagination")).toContainText("1–8 из 28");
  await page.screenshot({ path: testInfo.outputPath("compact-usage-filters.png") });
});

test("wide filters share one row and keep values across compact resizing", async ({ page }, testInfo) => {
  await page.setViewportSize({ width: 1440, height: 1000 });
  const expand = page.getByRole("button", { name: "Развернуть меню", exact: true });
  if (await expand.isVisible()) await expand.click();
  const toolbar = page.locator(".usage-filters");
  const more = page.getByRole("button", { name: "Дополнительные фильтры", exact: true });
  const protocol = page.getByRole("combobox", { name: "Протокол", exact: true });
  const search = page.getByRole("searchbox", { name: "ID запроса или клиент", exact: true });
  const reset = page.getByRole("button", { name: "Сбросить фильтры", exact: true });
  const assertOneRow = async () => {
    expect(await toolbar.evaluate(element => {
      const controls = [...element.querySelectorAll('.option-trigger, input, .usage-filter-reset, .usage-refresh')].filter(field => field.getClientRects().length);
      return new Set(controls.map(field => Math.round(field.getBoundingClientRect().top))).size === 1 && element.scrollWidth <= element.clientWidth;
    })).toBe(true);
  };
  await expect(more).not.toBeVisible();
  await expect(toolbar.getByRole("combobox")).toHaveCount(7);
  await expect(search).toBeVisible();
  await assertOneRow();
  await protocol.click();
  await page.getByRole("option", { name: "Responses", exact: true }).click();
  await search.fill("demo_request");
  await expect(reset).toBeVisible();
  await assertOneRow();
  await page.screenshot({ path: testInfo.outputPath("inline-usage-filters.png") });
  await protocol.click();
  await page.setViewportSize({ width: 900, height: 1000 });
  await expect(page.getByRole("listbox")).toHaveCount(0);
  await expect(protocol).not.toBeVisible();
  await expect(more).toHaveAttribute("data-active", "true");
  await more.click();
  await expect(protocol).toHaveText("Responses");
  await expect(search).toHaveValue("demo_request");
  await page.setViewportSize({ width: 1440, height: 1000 });
  await expect(more).not.toBeVisible();
  await assertOneRow();
  await reset.click();
  await expect(reset).toHaveCount(0);
  await expect(protocol).toHaveText("Протокол");
  await expect(search).toHaveValue("");
  await expect(page.getByRole("combobox", { name: "Период, UTC", exact: true })).toBeFocused();
  await expect(page.locator(".usage-page .pagination")).toContainText("1–8 из 28");
});
