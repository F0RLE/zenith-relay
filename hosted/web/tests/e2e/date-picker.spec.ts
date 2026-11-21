import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";

async function editor(page: Page, mode: "create" | "edit" = "edit") {
  await page.goto("/app/keys");
  if (mode === "create") await page.locator(".page-heading").getByRole("button", { name: "Создать ключ", exact: true }).click();
  else await page.getByRole("row", { name: "Editor", exact: true }).getByRole("button", { name: "Изменить", exact: true }).click();
  const form = page.getByRole("form", { name: mode === "create" ? "Создать ключ" : "Настройки ключа", exact: true });
  const input = form.getByLabel("Действует до", { exact: true });
  const trigger = form.getByRole("button", { name: "Открыть календарь: Действует до", exact: true });
  const calendar = page.getByRole("dialog", { name: "Действует до: выбрать дату", exact: true });
  return { form, input, trigger, calendar };
}

test("create chooses leap day and edit clears expiry back to account policy", async ({ page }) => {
  const { form, input, trigger, calendar } = await editor(page, "create");
  await form.getByLabel("Название", { exact: true }).fill("Календарь");
  await input.fill("28.02.2028");
  await trigger.click();
  await calendar.locator('[data-date="2028-02-29"]').click();
  await expect(calendar).toHaveCount(0);
  await expect(input).toHaveValue("29.02.2028");
  await expect(trigger).toBeFocused();
  await form.getByRole("button", { name: "Создать ключ", exact: true }).click();
  await page.getByRole("button", { name: "Готово", exact: true }).click();
  const row = page.getByRole("row", { name: "Календарь", exact: true });
  await expect(row.locator(".key-policy-cell .key-cell-value").last()).toHaveText("29.02.2028");
  await row.getByRole("button", { name: "Изменить", exact: true }).click();
  const edit = page.getByRole("form", { name: "Настройки ключа", exact: true });
  await expect(edit.getByLabel("Действует до", { exact: true })).toHaveValue("29.02.2028");
  await edit.getByRole("button", { name: "Открыть календарь: Действует до", exact: true }).click();
  await calendar.getByRole("button", { name: "Очистить дату", exact: true }).click();
  await expect(edit.getByLabel("Действует до", { exact: true })).toHaveValue("");
  await edit.getByRole("button", { name: "Сохранить", exact: true }).click();
  await expect(row.locator(".key-policy-cell .key-cell-value").last()).toHaveText("По правилам кабинета");
});

test("calendar keyboard crosses months and years without submitting the editor", async ({ page }) => {
  const { form, input, trigger, calendar } = await editor(page);
  await input.fill("2028-02-29");
  await input.press("ArrowDown");
  await expect(calendar.locator('[data-date="2028-02-29"]')).toBeFocused();
  for (const [key, date] of [["ArrowRight", "2028-03-01"], ["ArrowLeft", "2028-02-29"], ["PageDown", "2028-03-29"], ["PageUp", "2028-02-29"], ["Shift+PageDown", "2029-02-28"], ["Shift+PageUp", "2028-02-28"], ["Home", "2028-02-28"], ["End", "2028-03-05"]]) {
    await page.keyboard.press(key!);
    await expect(calendar.locator(`[data-date="${date}"]`)).toBeFocused();
  }
  await page.keyboard.press("Enter");
  await expect(input).toHaveValue("05.03.2028");
  await expect(form).toBeVisible();
  await trigger.click();
  await page.keyboard.press("Escape");
  await expect(calendar).toHaveCount(0);
  await expect(form).toBeVisible();
  await expect(trigger).toBeFocused();
  await page.keyboard.press("Escape");
  await expect(form).toHaveCount(0);
});

test("popup dismisses outside and Tab follows form order without a focus trap", async ({ page }) => {
  const { form, input, trigger, calendar } = await editor(page);
  await input.fill("2028-02-29");
  await trigger.click();
  await calendar.getByRole("button", { name: "Выбрать месяц и год: февраль 2028 г.", exact: true }).focus();
  await page.keyboard.press("Shift+Tab");
  await expect(calendar).toHaveCount(0);
  await expect(trigger).toBeFocused();
  await trigger.press("Enter");
  await calendar.getByRole("button", { name: "Сегодня", exact: true }).focus();
  await page.keyboard.press("Tab");
  await expect(calendar).toHaveCount(0);
  await expect(form.getByRole("button", { name: "Отмена", exact: true })).toBeFocused();
  await trigger.click();
  // The popup intentionally overlays earlier fields on phones; click a real outside inset.
  await page.mouse.click(4, page.viewportSize()!.height - 4);
  await expect(calendar).toHaveCount(0);
  await expect(form).toBeVisible();
  await trigger.click();
  await form.getByLabel("Название", { exact: true }).focus();
  await expect(calendar).toHaveCount(0);
  await expect(form.getByLabel("Название", { exact: true })).toBeFocused();
});

test("manual entry rejects impossible dates and clicking adjacent-month days works", async ({ page }) => {
  const { form, input, trigger, calendar } = await editor(page);
  await input.fill("31.02.2028");
  await form.getByRole("button", { name: "Сохранить", exact: true }).click();
  await expect(form).toBeVisible();
  await expect(input).toHaveAttribute("aria-invalid", "true");
  await expect(form.getByRole("alert")).toHaveText("Введите существующую дату в формате ДД.ММ.ГГГГ.");
  await input.fill("29022028");
  await trigger.click();
  await expect(form.getByRole("alert")).toHaveCount(0);
  await calendar.locator('[data-date="2028-03-01"]').click();
  await expect(input).toHaveValue("01.03.2028");
  await form.getByRole("button", { name: "Сохранить", exact: true }).click();
  await expect(page.getByRole("row", { name: "Editor", exact: true }).locator(".key-policy-cell .key-cell-value").last()).toHaveText("01.03.2028");
});

test("calendar matches both themes and fits the viewport without clipping or scroll lock", async ({ page }, testInfo) => {
  const { form, input, trigger, calendar } = await editor(page, "create");
  await input.fill("2028-02-29");
  await trigger.click();
  await expect(calendar).toBeVisible();
  await expect(calendar).toHaveCSS("background-color", await input.evaluate(element => getComputedStyle(element).backgroundColor));
  await expect(calendar).toHaveCSS("border-radius", "12px");
  await expect(calendar.locator('[data-selected="true"]')).toHaveCSS("background-color", await form.locator(".primary-button").evaluate(element => getComputedStyle(element).backgroundColor));
  await expect(calendar.getByRole("gridcell", { selected: true })).toHaveCount(1);
  await expect(calendar.getByRole("grid")).toBeVisible();
  await expect(calendar.locator("td").first()).toHaveCSS("border-bottom-width", "0px");
  await expect(calendar.locator("th").first()).toHaveCSS("background-color", "rgba(0, 0, 0, 0)");
  await expect(calendar.locator("tbody button")).toHaveCount(42);
  await expect(trigger.locator('svg[data-icon="calendar"]')).toBeVisible();
  await expect(calendar.getByRole("button", { name: "Удалить", exact: true })).toHaveCount(0);
  expect(await page.evaluate(() => document.body.style.overflow)).not.toBe("hidden");
  const metrics = await calendar.evaluate(element => {
    const rect = element.getBoundingClientRect();
    const controls = Array.from(element.querySelectorAll("button"));
    return { left: rect.left, right: rect.right, top: rect.top, bottom: rect.bottom, width: innerWidth, height: innerHeight,
      overflow: document.documentElement.scrollWidth > innerWidth || element.scrollWidth > element.clientWidth,
      targetMin: Math.min(...controls.map(control => control.getBoundingClientRect().height)),
      hit: element.contains(document.elementFromPoint(rect.x + rect.width / 2, rect.y + 24)) };
  });
  expect(metrics.left).toBeGreaterThanOrEqual(7);
  expect(metrics.right).toBeLessThanOrEqual(metrics.width - 7);
  expect(metrics.top).toBeGreaterThanOrEqual(7);
  expect(metrics.bottom).toBeLessThanOrEqual(metrics.height - 7);
  expect(metrics.overflow).toBe(false);
  expect(metrics.hit).toBe(true);
  expect(metrics.targetMin).toBeGreaterThanOrEqual(testInfo.project.name.startsWith("mobile") ? 44 : 40);
  await page.screenshot({ path: testInfo.outputPath("expiry-calendar.png") });
  await calendar.getByRole("button", { name: "Следующий месяц", exact: true }).click();
  await expect(calendar.locator(".date-calendar-period > span")).toHaveText("март 2028 г.");
  await calendar.getByRole("button", { name: "Предыдущий месяц", exact: true }).click();
  await expect(calendar.locator(".date-calendar-period > span")).toHaveText("февраль 2028 г.");
  await calendar.getByRole("button", { name: "Сегодня", exact: true }).click();
  await expect(input).toHaveValue(await page.evaluate(() => new Date().toLocaleDateString("ru-RU")));
});

test("short viewports scroll the calendar without losing its footer or date", async ({ page }) => {
  await page.setViewportSize({ width: page.viewportSize()!.width, height: 420 });
  const { input, trigger, calendar } = await editor(page, "create");
  await input.fill("29.02.2028");
  await trigger.click();
  await expect(calendar).toBeVisible();
  const bounds = (await calendar.boundingBox())!;
  expect(bounds.y).toBeGreaterThanOrEqual(7);
  expect(bounds.y + bounds.height).toBeLessThanOrEqual(413);
  const clear = calendar.getByRole("button", { name: "Очистить дату", exact: true });
  await clear.scrollIntoViewIfNeeded();
  await clear.click();
  await expect(calendar).toHaveCount(0);
  await expect(input).toHaveValue("");
  await expect(trigger).toBeFocused();
});

test("calendar keeps keyboard focus and selection in system high contrast", async ({ page }) => {
  await page.emulateMedia({ forcedColors: "active", reducedMotion: "reduce" });
  const { input, calendar } = await editor(page);
  await input.fill("29.02.2028");
  await input.press("ArrowDown");
  const selected = calendar.locator('[data-date="2028-02-29"]');
  await expect(selected).toBeFocused();
  await expect(selected).toHaveCSS("outline-width", "1px");
  await expect(selected).toHaveCSS("outline-style", "solid");
  await expect(selected).toHaveCSS("transition-duration", "0s");
  await expect(calendar.getByRole("gridcell", { selected: true })).toHaveCount(1);
  await page.keyboard.press("Escape");
  await expect(calendar).toHaveCount(0);
});

test("clickable month heading selects months and years inside the same popup", async ({ page }, testInfo) => {
  const { form, input, trigger, calendar } = await editor(page);
  await input.fill("01.11.2026");
  await trigger.click();
  const heading = calendar.getByRole("button", { name: "Выбрать месяц и год: ноябрь 2026 г.", exact: true });
  await expect(heading.locator('[data-icon="chevron-down"]')).toBeVisible();
  await heading.click();
  await expect(calendar.getByRole("grid", { name: "Месяцы 2026 года", exact: true })).toBeVisible();
  await expect(calendar.locator("[data-period]")).toHaveCount(12);
  await expect(calendar.getByRole("button", { name: "ноябрь", exact: true })).toBeFocused();
  await page.screenshot({ path: testInfo.outputPath("expiry-month-choice.png") });
  await calendar.getByRole("button", { name: "сентябрь", exact: true }).click();
  await expect(calendar.getByRole("grid", { name: "сентябрь 2026 г.", exact: true })).toBeVisible();
  await expect(input).toHaveValue("01.11.2026");
  await calendar.getByRole("button", { name: "Выбрать месяц и год: сентябрь 2026 г.", exact: true }).click();
  await calendar.getByRole("button", { name: "Выбрать год: 2026", exact: true }).click();
  await expect(calendar.getByRole("button", { name: "2026", exact: true })).toBeFocused();
  await calendar.getByRole("button", { name: "Следующие годы", exact: true }).click();
  await expect(calendar.getByRole("grid", { name: "Годы 2029–2040", exact: true })).toBeVisible();
  await page.screenshot({ path: testInfo.outputPath("expiry-year-choice.png") });
  await calendar.getByRole("button", { name: "2030", exact: true }).click();
  await expect(calendar.getByRole("grid", { name: "Месяцы 2030 года", exact: true })).toBeVisible();
  await calendar.getByRole("button", { name: "февраль", exact: true }).click();
  await expect(page.getByRole("dialog")).toHaveCount(1);
  await calendar.locator('[data-date="2030-02-15"]').click();
  await expect(input).toHaveValue("15.02.2030");
  await form.getByRole("button", { name: "Сохранить", exact: true }).click();
  await expect(page.getByRole("row", { name: "Editor", exact: true }).locator(".key-policy-cell .key-cell-value").last()).toHaveText("15.02.2030");
});

test("period selection keeps arrow navigation and Escape returns one level at a time", async ({ page }) => {
  const { form, input, trigger, calendar } = await editor(page);
  await input.fill("29.02.2028");
  await trigger.click();
  await calendar.getByRole("button", { name: "Выбрать месяц и год: февраль 2028 г.", exact: true }).click();
  for (const [key, month] of [["ArrowRight", "март"], ["ArrowDown", "июнь"], ["Home", "апрель"], ["End", "июнь"]]) {
    await page.keyboard.press(key!);
    await expect(calendar.getByRole("button", { name: month!, exact: true })).toBeFocused();
  }
  await page.keyboard.press("PageDown");
  await expect(calendar.getByRole("grid", { name: "Месяцы 2029 года", exact: true })).toBeVisible();
  await page.keyboard.press("Enter");
  await expect(calendar.getByRole("grid", { name: "июнь 2029 г.", exact: true })).toBeVisible();
  await calendar.getByRole("button", { name: "Выбрать месяц и год: июнь 2029 г.", exact: true }).click();
  await calendar.getByRole("button", { name: "Выбрать год: 2029", exact: true }).click();
  await page.keyboard.press("ArrowRight");
  await expect(calendar.getByRole("button", { name: "2030", exact: true })).toBeFocused();
  await page.keyboard.press("Escape");
  await expect(calendar.getByRole("grid", { name: "Месяцы 2030 года", exact: true })).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(calendar.getByRole("grid", { name: "июнь 2030 г.", exact: true })).toBeVisible();
  await expect(input).toHaveValue("29.02.2028");
  await page.keyboard.press("Escape");
  await expect(calendar).toHaveCount(0);
  await expect(form).toBeVisible();
  await expect(trigger).toBeFocused();
});

test("all calendar views stay inside the editor frame and viewport after resizing", async ({ page }) => {
  const { form, input, trigger, calendar } = await editor(page, "create");
  await input.fill("01.11.2026");
  await trigger.click();
  for (const width of [page.viewportSize()!.width, 320, 1100]) {
    await page.setViewportSize({ width, height: 740 });
    if (!await calendar.count()) await trigger.click();
    for (const view of ["days", "months", "years"]) {
      if (view !== "days") await calendar.locator(".date-calendar-period").click();
      const panelBounds = (await calendar.boundingBox())!;
      const formBounds = (await form.boundingBox())!;
      expect(panelBounds.x).toBeGreaterThanOrEqual(Math.max(8, formBounds.x) - 1);
      expect(panelBounds.x + panelBounds.width).toBeLessThanOrEqual(Math.min(width - 8, formBounds.x + formBounds.width) + 1);
      expect(panelBounds.y).toBeGreaterThanOrEqual(7);
      expect(panelBounds.y + panelBounds.height).toBeLessThanOrEqual(733);
      expect(await calendar.evaluate(element => element.scrollWidth <= element.clientWidth)).toBe(true);
      expect(await calendar.locator(".date-calendar-heading").evaluate(element => Array.from(element.querySelectorAll("button")).every(button => button.scrollWidth <= button.clientWidth))).toBe(true);
    }
    await page.keyboard.press("Escape");
    await page.keyboard.press("Escape");
  }
});
