import { expect, test } from "@playwright/test";
import type { Locator, Page } from "@playwright/test";

test.beforeEach(async ({ page }) => {
  await page.goto("/app/keys");
  await expect(page.getByRole("heading", { name: "API-ключи", exact: true })).toBeVisible();
});

async function simplifiedEditor(form: Locator) {
  await expect(form.getByRole("checkbox")).toHaveCount(0);
  await expect(form.locator("fieldset")).toHaveCount(0);
  await expect(form).not.toContainText("Разрешённые модели");
  await expect(form).not.toContainText("Пустые лимит и срок");
  await expect(form).not.toContainText("Дневной лимит сбрасывается");
}

async function inlineLayout(page: Page) {
  await expect(page.getByRole("dialog")).toHaveCount(0);
  expect(await page.evaluate(() => document.body.style.overflow)).not.toBe("hidden");
  expect(await page.locator(".api-keys-page").evaluate(element => {
    const parent = element.getBoundingClientRect();
    return element.scrollWidth <= element.clientWidth && Array.from(element.querySelectorAll(".key-row, .key-editor, input, .key-actions, .key-reveal")).every(item => {
      const bounds = item.getBoundingClientRect();
      // A long input value scrolls inside its field; its outer box must still fit.
      return (item instanceof HTMLInputElement || item.scrollWidth <= item.clientWidth + 1)
        && bounds.left >= parent.left - 1 && bounds.right <= parent.right + 1;
    });
  })).toBe(true);
}

async function sortKeys(page: Page, field: string, label: string, direction = "ascending") {
  const mobile = page.getByRole("combobox", { name: "Сортировка ключей" });
  if (await mobile.isVisible()) {
    await mobile.click();
    await page.getByRole("option", { name: `${label} · ${direction === "ascending" ? "по возрастанию" : "по убыванию"}`, exact: true }).click();
  } else {
    const column = page.getByRole("columnheader", { name: label, exact: true });
    if (await column.getAttribute("aria-sort") !== direction) await column.getByRole("button").click();
    if (await column.getAttribute("aria-sort") !== direction) await column.getByRole("button").click();
    await expect(column).toHaveAttribute("aria-sort", direction);
  }
}

test("keys use a centered single-line table with a shared header and responsive rows", async ({ page }, testInfo) => {
  const rows = page.locator(".key-row");
  const table = page.getByRole("table", { name: "API-ключи", exact: true });
  await expect(table).toBeVisible();
  const frame = page.locator(".keys-list");
  await expect(frame).toHaveClass(/table-shell/);
  await expect(frame).toHaveCSS("border-radius", "12px");
  for (const side of ["top", "right", "bottom", "left"]) {
    await expect(frame).toHaveCSS(`border-${side}-width`, "1px");
    await expect(frame).toHaveCSS(`border-${side}-style`, "solid");
  }
  await expect(frame).toHaveCSS("overflow", "hidden");
  await expect(table.locator("tbody").last().locator("tr").last().locator("td").first()).toHaveCSS("border-bottom-width", "0px");
  await expect(rows).toHaveCount(3);
  await expect(page.locator(".key-card, .key-list")).toHaveCount(0);
  await expect(rows.first().locator(".key-policy-cell .key-cell-value").first()).toHaveText("GPT");
  await expect(rows.first()).toHaveCSS("background-color", "rgba(0, 0, 0, 0)");
  await expect(rows.first()).toHaveCSS("border-radius", "0px");
  const bounds = (await rows.first().boundingBox())!;
  if (await table.locator("thead").isVisible()) {
    await expect(table.getByRole("columnheader")).toHaveText(["Имя", "API-ключ", "Статус", "Модели", "Дневной лимит", "Срок действия", "Действия"]);
    expect(bounds.height).toBeLessThanOrEqual(72);
    expect(await table.evaluate(element => {
      const headers = Array.from(element.querySelectorAll("th"));
      const cells = Array.from(element.querySelectorAll(".key-row:first-child > td")).slice(0, 7);
      return cells.every((cell, index) => {
        const header = headers[index]!.getBoundingClientRect(), value = cell.getBoundingClientRect();
        return Math.abs(header.x + header.width / 2 - value.x - value.width / 2) < 1
          && getComputedStyle(cell).textAlign === "center"
          && value.y === cells[0]!.getBoundingClientRect().y;
      });
    })).toBe(true);
    await expect(rows.first().locator(".key-cell-label").first()).toBeHidden();
  } else {
    expect(bounds.height).toBeLessThan(280);
    await expect(page.getByRole("combobox", { name: "Сортировка ключей" })).toBeVisible();
    for (const label of await rows.first().locator(".key-cell-label").all()) await expect(label).toBeVisible();
  }
  await inlineLayout(page);
  await page.screenshot({ path: testInfo.outputPath("keys-list.png") });
  if (testInfo.project.name.startsWith("desktop")) {
    for (const width of [1307, 1366, 1440, 1920]) {
      await page.setViewportSize({ width, height: 1000 });
      await expect(table.locator("thead")).toBeVisible();
      await inlineLayout(page);
      expect(await table.locator("th, .key-row > td").evaluateAll(cells => cells.every(cell => cell.scrollWidth <= cell.clientWidth + 1))).toBe(true);
      const sortSpacing = await table.locator(".key-sort").evaluateAll(buttons => buttons.map(button => {
        const control = button.getBoundingClientRect(), icon = button.querySelector("svg")!.getBoundingClientRect();
        const range = document.createRange();
        range.selectNode(button.firstChild!);
        const text = range.getBoundingClientRect();
        return { inset: control.right - icon.right, gap: icon.left - text.right, center: text.x + text.width / 2 - control.x - control.width / 2 };
      }));
      for (const spacing of sortSpacing) {
        expect(spacing.inset).toBeCloseTo(12, 1);
        expect(spacing.gap).toBeGreaterThanOrEqual(0);
        expect(Math.abs(spacing.center)).toBeLessThan(1);
      }
      if (width === 1440) await page.screenshot({ path: testInfo.outputPath("keys-wide-table.png") });
    }
    const byName = table.getByRole("columnheader", { name: "Имя", exact: true });
    await byName.getByRole("button").focus();
    await page.keyboard.press("Enter");
    await expect(byName).toHaveAttribute("aria-sort", "ascending");
    await expect(byName.getByRole("button")).toHaveCSS("outline-style", "none");
    await expect(byName.getByRole("button")).toHaveCSS("outline-width", "0px");
    await page.keyboard.press("Space");
    await expect(byName).toHaveAttribute("aria-sort", "descending");
  }
});

test("compact key rows group identity and align policies without stranded fields", async ({ page }, testInfo) => {
  test.skip(testInfo.project.name !== "desktop-light", "Explicit viewport coverage runs once; interaction tests cover all projects.");
  for (const width of [1280, 1152, 1024, 768, 390, 320]) {
    await page.setViewportSize({ width, height: 1000 });
    const row = page.locator(".key-row").first();
    const name = (await row.locator(".key-identity").boundingBox())!;
    const token = (await row.locator(".key-token").boundingBox())!;
    expect(Math.abs(name.x - token.x)).toBeLessThan(1);
    expect(token.y - name.y - name.height).toBeCloseTo(4, 0);
    const policies = await row.locator(".key-policy-cell").evaluateAll(cells => cells.map(cell => cell.getBoundingClientRect().top));
    if (width >= 1024) expect(Math.max(...policies) - Math.min(...policies)).toBeLessThan(1);
    expect((await row.boundingBox())!.height).toBeLessThan(width === 1280 ? 145 : 260);
    await inlineLayout(page);
    await page.screenshot({ path: testInfo.outputPath(`keys-compact-${width}.png`) });
  }
  await page.setViewportSize({ width: 1280, height: 1000 });
  const sort = page.getByRole("combobox", { name: "Сортировка ключей", exact: true });
  await expect(sort).not.toHaveJSProperty("tagName", "SELECT");
  const row = page.locator(".key-row").first();
  const before = await row.boundingBox();
  await sort.click();
  await expect(page.getByRole("listbox")).toBeVisible();
  expect(await row.boundingBox()).toEqual(before);
  const menu = (await page.getByRole("listbox").boundingBox())!;
  expect(menu.x).toBeGreaterThanOrEqual(8);
  expect(menu.x + menu.width).toBeLessThanOrEqual(1272);
  expect(menu.y + menu.height).toBeLessThanOrEqual(992);
  await page.keyboard.press("Escape");
  await expect(sort).toBeFocused();
});

test("sort header hover fills its cell and follows the frame corners", async ({ page }, testInfo) => {
  test.skip(!testInfo.project.name.startsWith("desktop"), "Narrow layouts do not show the table header.");
  await page.setViewportSize({ width: 1440, height: 1000 });
  const frame = page.locator(".keys-list");
  await expect(frame).toHaveCSS("border-radius", "12px");
  await expect(frame).toHaveCSS("overflow", "hidden");
  for (const label of ["Имя", "Срок действия"]) {
    const cell = page.getByRole("columnheader", { name: label, exact: true });
    const button = cell.getByRole("button");
    await button.hover();
    await expect(button).toHaveCSS("border-radius", "0px");
    await expect(button).not.toHaveCSS("background-color", "rgba(0, 0, 0, 0)");
    const parent = (await cell.boundingBox())!;
    const control = (await button.boundingBox())!;
    expect(Math.abs(parent.x - control.x)).toBeLessThan(1);
    expect(Math.abs(parent.y - control.y)).toBeLessThan(1);
    expect(Math.abs(parent.width - control.width)).toBeLessThan(1);
    expect(parent.height - control.height).toBeLessThanOrEqual(1);
    await expect(page.getByRole("tooltip")).toBeVisible();
    if (label === "Имя") await frame.screenshot({ path: testInfo.outputPath("keys-header-flush-corners.png"), animations: "disabled" });
  }
});

test("sort hints use Relay styling above the header and follow the next sorting direction", async ({ page }, testInfo) => {
  test.skip(!testInfo.project.name.startsWith("desktop"), "Narrow layouts use the sort selector instead of column headers.");
  await page.setViewportSize({ width: 1440, height: 1000 });
  const header = page.getByRole("columnheader", { name: "Дневной лимит", exact: true });
  const trigger = header.getByRole("button");
  const hint = page.getByRole("tooltip");
  await expect(page.locator(".key-sort[title]")).toHaveCount(0);
  await trigger.hover();
  await expect(hint).toHaveText("Сортировать по возрастанию");
  await expect(hint).toHaveClass(/hosted-tooltip/);
  await expect(hint).toHaveAttribute("data-placement", "top");
  await expect(hint).toHaveCSS("opacity", "1");
  await expect(hint).toHaveCSS("pointer-events", "none");
  expect(await hint.evaluate(element => element.matches(":popover-open"))).toBe(true);
  await expect(trigger).toHaveAttribute("aria-describedby", (await hint.getAttribute("id"))!);
  const anchor = (await trigger.boundingBox())!, bounds = (await hint.boundingBox())!;
  expect(Math.abs(bounds.x + bounds.width / 2 - anchor.x - anchor.width / 2)).toBeLessThan(1);
  expect(anchor.y - bounds.y - bounds.height).toBeCloseTo(8, 0);
  expect(bounds.y).toBeGreaterThanOrEqual(8);
  expect(bounds.x).toBeGreaterThanOrEqual(8);
  expect(bounds.x + bounds.width).toBeLessThanOrEqual(1432);
  await page.screenshot({ path: testInfo.outputPath("keys-sort-tooltip.png") });

  await trigger.click();
  await expect(header).toHaveAttribute("aria-sort", "ascending");
  await expect(hint).toHaveCount(0);
  await page.mouse.move(0, 0);
  await trigger.hover();
  await expect(hint).toHaveText("Сортировать по убыванию");
  await page.keyboard.press("Escape");
  await expect(hint).toHaveCount(0);
  await page.keyboard.press("Tab");
  await page.keyboard.press("Shift+Tab");
  await expect(trigger).toBeFocused();
  await expect(hint).toHaveText("Сортировать по убыванию");
  await page.keyboard.press("Enter");
  await expect(header).toHaveAttribute("aria-sort", "descending");
  await expect(hint).toHaveText("Сортировать по возрастанию");
  await expect(hint).toHaveAttribute("data-placement", "top");
  await page.keyboard.press("Escape");
  await expect(hint).toHaveCount(0);
  await expect(trigger).not.toHaveAttribute("aria-describedby");
});

test("name and model sorting keep controls attached to the right key", async ({ page }, testInfo) => {
  const names = page.locator(".key-row h2");
  await sortKeys(page, "name", "Имя");
  await expect(names).toHaveText(["Bot", "Editor", "My project"]);
  await sortKeys(page, "name", "Имя", "descending");
  await expect(names).toHaveText(["My project", "Editor", "Bot"]);
  await sortKeys(page, "models", "Модели");
  await expect(names).toHaveText(["My project", "Bot", "Editor"]);
  const editor = page.getByRole("row", { name: "Editor", exact: true });
  await editor.getByRole("button", { name: "Отозвать", exact: true }).click();
  await sortKeys(page, "name", "Имя");
  await expect(page.getByRole("button", { name: "Да, отозвать", exact: true })).toHaveCount(0);
  await expect(editor).toHaveAttribute("data-revoked", "false");
  await editor.getByRole("button", { name: "Изменить", exact: true }).click();
  await expect(page.getByRole("form", { name: "Настройки ключа" }).getByLabel("Название", { exact: true })).toHaveValue("Editor");
  await page.keyboard.press("Escape");
  await expect(editor.getByRole("button", { name: "Изменить", exact: true })).toBeFocused();
  await inlineLayout(page);
  await page.screenshot({ path: testInfo.outputPath("keys-sorted.png") });
});

test("limit and expiry sorting is numeric and keeps inherited policies last", async ({ page }) => {
  for (const [name, limit, expires] of [["Editor", "10", "2030-02-01"], ["Bot", "2", "2029-01-01"]]) {
    await page.getByRole("row", { name, exact: true }).getByRole("button", { name: "Изменить", exact: true }).click();
    const form = page.getByRole("form", { name: "Настройки ключа", exact: true });
    await form.getByLabel("Дневной лимит, USD", { exact: true }).fill(limit!);
    await form.getByLabel("Действует до", { exact: true }).fill(expires!);
    await form.getByRole("button", { name: "Сохранить", exact: true }).click();
  }
  const names = page.locator(".key-row h2");
  for (const [field, label] of [["dailyLimit", "Дневной лимит"], ["expires", "Срок действия"]]) {
    await sortKeys(page, field!, label!);
    await expect(names).toHaveText(["Bot", "Editor", "My project"]);
    await sortKeys(page, field!, label!, "descending");
    await expect(names).toHaveText(["Editor", "Bot", "My project"]);
  }
  const editor = page.getByRole("row", { name: "Editor", exact: true });
  await editor.getByRole("button", { name: "Отозвать", exact: true }).click();
  await editor.getByRole("button", { name: "Да, отозвать", exact: true }).click();
  await sortKeys(page, "revoked", "Статус", "descending");
  await expect(names.first()).toHaveText("Editor");
  await expect(editor.getByRole("button")).toHaveCount(0);
  await inlineLayout(page);
});

test("revoke confirms in its own row, cancels safely and ignores accidental double clicks", async ({ page }, testInfo) => {
  const row = page.getByRole("row", { name: "Editor", exact: true });
  const other = page.getByRole("row", { name: "My project", exact: true });
  const revoke = row.getByRole("button", { name: "Отозвать", exact: true });
  await revoke.click();
  await expect(row).toHaveAttribute("data-revoked", "false");
  await expect(row.getByRole("button", { name: "Да, отозвать", exact: true })).toBeFocused();
  await expect(row.locator(".key-revoke-warning")).toHaveCount(0);
  await expect(row).not.toContainText("Новые запросы с этим ключом будут запрещены");
  await expect(row.getByRole("button", { name: "Да, отозвать", exact: true })).not.toHaveAttribute("aria-describedby");
  await inlineLayout(page);
  await page.screenshot({ path: testInfo.outputPath("keys-revoke-inline.png") });
  await row.getByRole("button", { name: "Отмена", exact: true }).click();
  await expect(revoke).toBeFocused();
  await revoke.click();
  await page.keyboard.press("Escape");
  await expect(revoke).toBeFocused();
  await revoke.click();
  await page.getByRole("heading", { name: "API-ключи", exact: true }).click();
  await expect(row.getByRole("button", { name: "Да, отозвать", exact: true })).toHaveCount(0);
  await revoke.click();
  await other.getByRole("button", { name: "Отозвать", exact: true }).click();
  await expect(page.getByRole("button", { name: "Да, отозвать", exact: true })).toHaveCount(1);
  await expect(other.getByRole("button", { name: "Да, отозвать", exact: true })).toBeVisible();
  await page.keyboard.press("Escape");
  await revoke.dblclick();
  await expect(row).toHaveAttribute("data-revoked", "false");
  await row.getByRole("button", { name: "Да, отозвать", exact: true }).press("Enter");
  await expect(row).toHaveAttribute("data-revoked", "true");
  await expect(row.locator(".status-badge")).toHaveText("Отозван");
  await expect(row.getByRole("heading", { name: "Editor", exact: true })).toBeFocused();
  await expect(row.getByRole("button")).toHaveCount(0);
  await expect(other).toHaveAttribute("data-revoked", "false");
  await inlineLayout(page);
});

test("inline create validates fields without a model picker, supports copying and clears the one-time secret", async ({ page }, testInfo) => {
  await page.evaluate(() => Object.defineProperty(navigator, "clipboard", { configurable: true, value: { writeText: async () => {} } }));
  const create = page.locator(".page-heading").getByRole("button", { name: "Создать ключ", exact: true });
  await create.click();
  const form = page.getByRole("form", { name: "Создать ключ", exact: true });
  await simplifiedEditor(form);
  await expect(form.getByLabel("Название", { exact: true })).toBeFocused();
  const submit = form.getByRole("button", { name: "Создать ключ", exact: true });
  await expect(submit).toBeDisabled();
  await form.getByLabel("Название", { exact: true }).fill("Проверка inline");
  await expect(submit).toBeEnabled();
  await form.getByLabel("Дневной лимит, USD", { exact: true }).fill("0");
  await submit.click();
  await expect(form).toBeVisible();
  await expect(page.locator(".key-row")).toHaveCount(3);
  await form.getByLabel("Дневной лимит, USD", { exact: true }).fill("12");
  await form.getByLabel("Действует до", { exact: true }).fill("2030-01-01");
  await inlineLayout(page);
  await page.screenshot({ path: testInfo.outputPath("keys-create-inline.png") });
  await submit.click();
  const result = page.getByRole("region", { name: "Ключ создан", exact: true });
  await expect(result).toBeVisible();
  await expect(result.locator("code")).toContainText("demo_not_a_real_key_");
  await expect(result.getByRole("button", { name: "Скопировать ключ", exact: true })).toBeFocused();
  await expect(create).toBeDisabled();
  await result.getByRole("button", { name: "Скопировать ключ", exact: true }).click();
  await expect(result.getByRole("status")).toHaveText("Скопировано.");
  await inlineLayout(page);
  await result.getByRole("button", { name: "Готово", exact: true }).click();
  await expect(result).toHaveCount(0);
  const row = page.getByRole("row", { name: "Проверка inline", exact: true });
  await expect(row.getByRole("heading", { name: "Проверка inline", exact: true })).toBeFocused();
  await expect(row.locator(".key-policy-cell .key-cell-value")).toHaveText(["GPT, Claude, Gemini", "12 USD", "01.01.2030"]);
  await expect(page.getByText(/demo_not_a_real_key_/)).toHaveCount(0);
  expect(await page.evaluate(() => JSON.stringify({ ...localStorage, ...sessionStorage }))).not.toMatch(/Проверка inline|demo_not_a_real_key_/);
});

test("inline editing preserves cancel, filter scope and long key names", async ({ page }, testInfo) => {
  const row = page.getByRole("row", { name: "Editor", exact: true });
  const edit = row.getByRole("button", { name: "Изменить", exact: true });
  await edit.click();
  const form = page.getByRole("form", { name: "Настройки ключа", exact: true });
  await simplifiedEditor(form);
  await expect(form.getByRole("heading")).toHaveCount(0);
  await expect(form).toHaveCSS("background-color", "rgba(0, 0, 0, 0)");
  await expect(form).toHaveCSS("border-radius", "0px");
  if (testInfo.project.name.startsWith("desktop")) {
    for (const width of [1307, 1440]) {
      await page.setViewportSize({ width, height: 1000 });
      await inlineLayout(page);
      expect(await form.evaluate(element => {
        const save = element.querySelector<HTMLButtonElement>('button[type="submit"]')!.getBoundingClientRect();
        return Array.from(element.querySelectorAll("input")).every(input => Math.abs(input.getBoundingClientRect().bottom - save.bottom) < 1);
      })).toBe(true);
      expect((await form.locator("xpath=..").boundingBox())!.height).toBeLessThanOrEqual(112);
    }
  }
  await page.screenshot({ path: testInfo.outputPath("keys-edit-compact.png") });
  await expect(form.getByLabel("Название", { exact: true })).toHaveValue("Editor");
  await form.getByLabel("Название", { exact: true }).fill("Не сохранять");
  await form.getByRole("button", { name: "Отмена", exact: true }).click();
  await expect(edit).toBeFocused();
  await expect(row.getByRole("heading", { name: "Editor", exact: true })).toBeVisible();
  await edit.click();
  const longName = "ДлинноеИмяКлюча".repeat(5);
  await form.getByLabel("Название", { exact: true }).fill(longName);
  await form.getByLabel("Дневной лимит, USD", { exact: true }).fill("7");
  await inlineLayout(page);
  await page.screenshot({ path: testInfo.outputPath("keys-edit-inline.png") });
  await form.getByRole("button", { name: "Сохранить", exact: true }).click();
  await expect(form).toHaveCount(0);
  const updated = page.getByRole("row", { name: longName, exact: true });
  await expect(updated.locator(".key-policy-cell .key-cell-value")).toHaveText(["GPT", "7 USD", "По правилам кабинета"]);
  await expect(updated.getByRole("button", { name: "Изменить", exact: true })).toBeFocused();
  await inlineLayout(page);
});

test("reissue confirms in its own row, cancels safely and ignores repeated activation", async ({ page }, testInfo) => {
  const row = page.getByRole("row", { name: "Editor", exact: true });
  const other = page.getByRole("row", { name: "My project", exact: true });
  const reissue = row.getByRole("button", { name: "Перевыпустить", exact: true });
  const confirm = row.getByRole("button", { name: "Да, перевыпустить", exact: true });
  await reissue.click();
  await expect(confirm).toBeFocused();
  await expect(page.locator(".key-row")).toHaveCount(3);
  await expect(page.getByRole("form")).toHaveCount(0);
  await expect(row).not.toContainText("Старый ключ останется активным до отзыва.");
  await expect(row.locator(".key-reissue-note")).toHaveCount(0);
  await expect(confirm).not.toHaveAttribute("aria-describedby");
  await inlineLayout(page);
  await page.screenshot({ path: testInfo.outputPath("keys-reissue-confirm.png") });
  await row.getByRole("button", { name: "Отмена", exact: true }).click();
  await expect(reissue).toBeFocused();
  await reissue.click();
  await page.keyboard.press("Escape");
  await expect(reissue).toBeFocused();
  await reissue.click();
  await page.getByRole("heading", { name: "API-ключи", exact: true }).click();
  await expect(confirm).toHaveCount(0);
  await reissue.click();
  await other.getByRole("button", { name: "Перевыпустить", exact: true }).click();
  await expect(confirm).toHaveCount(0);
  await expect(other.getByRole("button", { name: "Да, перевыпустить", exact: true })).toBeVisible();
  await page.keyboard.press("Escape");
  await reissue.click();
  await other.getByRole("button", { name: "Отозвать", exact: true }).click();
  await expect(confirm).toHaveCount(0);
  await expect(other.getByRole("button", { name: "Да, отозвать", exact: true })).toBeVisible();
  await page.keyboard.press("Escape");
  await reissue.focus();
  await page.keyboard.down("Enter");
  await expect(confirm).toBeFocused();
  await page.keyboard.down("Enter");
  await page.keyboard.up("Enter");
  await expect(page.locator(".key-row")).toHaveCount(3);
  await page.keyboard.press("Escape");
  await reissue.dblclick();
  await expect(page.locator(".key-row")).toHaveCount(3);
  await expect(confirm).toBeVisible();
  await confirm.press("Enter");
  await expect(page.locator(".key-row")).toHaveCount(4);
  await expect(page.getByRole("region", { name: "Ключ создан", exact: true })).toBeVisible();
  await expect(row).toHaveAttribute("data-revoked", "false");
  await expect(other).toHaveAttribute("data-revoked", "false");
  await inlineLayout(page);
});

test("confirmed reissue preserves settings and the old key until a separate revoke", async ({ page }) => {
  const original = page.getByRole("row", { name: "Editor", exact: true });
  const oldMask = await original.locator(".key-token").textContent();
  await original.getByRole("button", { name: "Изменить", exact: true }).click();
  const form = page.getByRole("form", { name: "Настройки ключа", exact: true });
  await form.getByLabel("Дневной лимит, USD", { exact: true }).fill("17");
  await form.getByLabel("Действует до", { exact: true }).fill("2031-02-03");
  await form.getByRole("button", { name: "Сохранить", exact: true }).click();
  await original.getByRole("button", { name: "Перевыпустить", exact: true }).click();
  await expect(page.locator(".key-row")).toHaveCount(3);
  await expect(page.getByRole("form")).toHaveCount(0);
  await inlineLayout(page);
  await original.getByRole("button", { name: "Да, перевыпустить", exact: true }).click();
  await expect(page.locator(".key-row")).toHaveCount(4);
  await expect(original).toHaveAttribute("data-revoked", "false");
  await expect(original.locator(".key-token")).toHaveText(oldMask!);
  const replacement = page.getByRole("row", { name: "Editor · замена", exact: true });
  await expect(replacement).toHaveAttribute("data-revoked", "false");
  await expect(replacement.locator(".key-token")).not.toHaveText(oldMask!);
  await expect(replacement.locator(".key-policy-cell .key-cell-value")).toHaveText(["GPT", "17 USD", "03.02.2031"]);
  for (const control of await page.getByRole("button", { name: "Перевыпустить", exact: true }).all()) await expect(control).toBeDisabled();
  await expect(page.getByRole("button", { name: "Создать ключ", exact: true })).toBeDisabled();
  const result = page.getByRole("region", { name: "Ключ создан", exact: true });
  await expect(result.locator("code")).toContainText("demo_not_a_real_key_");
  await expect(result.getByRole("button", { name: "Скопировать ключ", exact: true })).toBeFocused();
  await result.getByRole("button", { name: "Готово", exact: true }).click();
  await expect(result).toHaveCount(0);
  await expect(replacement.getByRole("heading")).toBeFocused();
  await expect(page.getByText(/demo_not_a_real_key_/)).toHaveCount(0);
  expect(await page.evaluate(() => JSON.stringify({ ...localStorage, ...sessionStorage }))).not.toMatch(/demo_not_a_real_key_/);
  await original.getByRole("button", { name: "Отозвать", exact: true }).click();
  await original.getByRole("button", { name: "Да, отозвать", exact: true }).click();
  await expect(original).toHaveAttribute("data-revoked", "true");
  await expect(replacement).toHaveAttribute("data-revoked", "false");
  await expect(page.getByRole("dialog")).toHaveCount(0);
});
