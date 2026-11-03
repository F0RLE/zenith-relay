import { expect, test } from "@playwright/test";
import type { Locator } from "@playwright/test";

test("trailing chevrons share a twelve-pixel inset across selectors and disclosures", async ({ page }, testInfo) => {
  async function checkInset(control: Locator) {
    await control.scrollIntoViewIfNeeded();
    const icon = control.locator(":scope > .relay-icon").last();
    await expect(icon).toHaveCSS("width", "14px");
    await expect(icon).toHaveCSS("height", "14px");
    await expect.poll(() => control.evaluate(element => {
      const arrow = element.querySelector(":scope > .relay-icon:last-child")!.getBoundingClientRect();
      return element.getBoundingClientRect().right - parseFloat(getComputedStyle(element).borderRightWidth) - arrow.right;
    })).toBeCloseTo(12, 1);
    expect(await control.evaluate(element => element.scrollWidth <= element.clientWidth + 1)).toBe(true);
  }

  for (const path of ["/app/overview", "/app/usage"]) {
    await page.goto(path);
    const controls = page.locator(".option-trigger:not(.language-trigger):visible");
    await expect(controls.first()).toBeVisible();
    for (const control of await controls.all()) await checkInset(control);
  }
  const pageSize = page.locator(".pagination .option-trigger").first();
  await checkInset(pageSize);
  await pageSize.click();
  await page.getByRole("option", { name: "16", exact: true }).click();
  await expect(pageSize).toContainText("16");
  await checkInset(pageSize);

  await page.getByRole("button", { name: "Детали запроса demo_request_0003", exact: true }).click();
  const disclosure = page.getByRole("dialog").locator(".request-technical-details > summary");
  await checkInset(disclosure);
  await disclosure.click();
  await expect(page.getByRole("dialog").locator(".request-technical-details")).toHaveAttribute("open");
  await checkInset(disclosure);
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  await page.screenshot({ path: testInfo.outputPath("shared-chevron-insets.png") });
});

test("customer and owner actions share flat controls and reachable sizes", async ({ page }) => {
  const height = page.viewportSize()!.width <= 639 ? "44px" : "40px";
  await page.goto("/admin/connections");
  const primary = page.getByRole("button", { name: "Добавить подключение", exact: true });
  const open = page.getByRole("button", { name: /^Действия:/ }).first();
  for (const control of [primary, open]) {
    await expect(control).toHaveCSS("height", height);
    await expect(control).toHaveCSS("line-height", "20px");
    await expect(control).toHaveCSS("border-radius", "8px");
    await expect(control).toHaveCSS("box-shadow", "none");
  }
  await page.goto("/app/keys");
  const edit = page.getByRole("button", { name: "Изменить", exact: true }).first();
  await expect(edit).toHaveCSS("height", height);
  await expect(edit).toHaveCSS("background-color", "rgba(0, 0, 0, 0)");
  await expect(edit).toHaveCSS("box-shadow", "none");
  const revoke = page.getByRole("button", { name: "Отозвать", exact: true }).first();
  await expect(revoke).toHaveCSS("background-color", "rgba(0, 0, 0, 0)");
  await expect(revoke).not.toHaveCSS("color", await edit.evaluate(element => getComputedStyle(element).color));
  await edit.click();
  const editor = page.getByRole("form", { name: "Настройки ключа", exact: true });
  await expect(editor.getByRole("button", { name: "Сохранить", exact: true })).toHaveCSS("height", height);
  await expect(page.getByRole("dialog")).toHaveCount(0);
  await page.keyboard.press("Escape");
  await expect(edit).toBeFocused();
  await expect(edit).toHaveCSS("outline-style", "none");
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
});

test("key row actions use background-only hover and retain keyboard focus", async ({ page }, testInfo) => {
  await page.goto("/app/keys");
  const row = page.locator('.key-row[data-revoked="false"]').first();
  await expect(row.locator(".key-actions button")).toHaveCount(3);
  const edit = row.getByRole("button", { name: "Изменить", exact: true });
  for (const action of await row.locator(".key-actions button").all()) {
    await action.hover();
    await expect(action).toHaveCSS("border-color", "rgba(0, 0, 0, 0)");
    await expect(action).not.toHaveCSS("background-color", "rgba(0, 0, 0, 0)");
    await expect(action).toHaveCSS("box-shadow", "none");
    await expect(action).toHaveCSS("outline-style", "none");
  }
  await edit.hover();
  await expect(edit).toHaveCSS("border-color", "rgba(0, 0, 0, 0)");
  await page.screenshot({ path: testInfo.outputPath("keys-hover-no-border.png") });
  await edit.click();
  await expect(page.getByRole("form", { name: "Настройки ключа", exact: true })).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(edit).toBeFocused();
  await expect(edit).toHaveCSS("outline-style", "none");
  await expect(edit).toHaveCSS("outline-width", "0px");
  await expect(edit).toHaveCSS("border-color", "rgba(0, 0, 0, 0)");
  await row.getByRole("button", { name: "Перевыпустить", exact: true }).click();
  const confirm = row.getByRole("button", { name: "Да, перевыпустить", exact: true });
  await confirm.hover();
  await expect(confirm).toHaveCSS("border-color", "rgba(0, 0, 0, 0)");
  await row.getByRole("button", { name: "Отмена", exact: true }).click();
  await expect(confirm).toHaveCount(0);
});

test("header utilities stay quiet while preserving theme and notification states", async ({ page }) => {
  await page.goto("/app/overview");
  const theme = page.getByRole("button", { name: /^(Тёмная|Светлая) тема$/ });
  const bell = page.getByRole("button", { name: /^Уведомления:/ });
  for (const control of [theme, bell]) {
    await expect(control).toHaveCSS("background-color", "rgba(0, 0, 0, 0)");
    await expect(control).toHaveCSS("border-top-color", "rgba(0, 0, 0, 0)");
    await expect(control).toHaveCSS("box-shadow", "none");
  }
  const previousTheme = await page.locator(".hosted-app").getAttribute("data-theme");
  await theme.click();
  await expect(page.locator(".hosted-app")).toHaveAttribute("data-theme", previousTheme === "light" ? "dark" : "light");
  await bell.click();
  await expect(bell).toHaveAttribute("aria-expanded", "true");
  await expect(bell).not.toHaveCSS("background-color", "rgba(0, 0, 0, 0)");
  await expect(page.locator("#notifications-panel")).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(bell).toHaveAttribute("aria-expanded", "false");
  await expect(bell).toBeFocused();
});

test("disabled pool actions stay inert and unraised on press", async ({ page }) => {
  await page.goto("/admin/pools");
  await page.getByRole("button", { name: "Настройки ротации пула", exact: true }).click();
  const panel = page.getByRole("region", { name: "Ротация пула", exact: true });
  await panel.getByRole("radio", { name: "По кругу", exact: true }).click();
  const up = panel.getByRole("button", { name: "Переместить Code account вверх", exact: true });
  await expect(up).toBeDisabled();
  const order = panel.getByRole("listitem");
  const before = await order.allTextContents();
  await up.scrollIntoViewIfNeeded();
  const box = (await up.boundingBox())!;
  await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
  await page.mouse.down();
  await expect(up).toHaveCSS("transform", "none");
  await expect(up).toHaveCSS("box-shadow", "none");
  await page.mouse.up();
  expect(await order.allTextContents()).toEqual(before);
});

for (const locale of ["en", "ru", "zh-CN"] as const) {
  test(`key creation stays grouped with the page title in ${locale}`, async ({ page }, testInfo) => {
    await page.addInitScript(value => localStorage.setItem("relay-hosted-locale", value), locale);
    await page.goto("/app/keys");
    const header = page.locator(".api-keys-page > .page-heading");
    const title = header.getByRole("heading", { level: 1 });
    const create = header.getByRole("button");
    await expect(create).toBeVisible();
    await expect(create).toBeEnabled();
    await page.evaluate(() => document.fonts.ready);
    const heading = (await title.boundingBox())!;
    const action = (await create.boundingBox())!;
    const tools = (await page.locator(".hosted-tools:visible").boundingBox())!;
    const centerDifference = Math.abs(heading.y + heading.height / 2 - action.y - action.height / 2);
    if (page.viewportSize()!.width >= 1024) expect(centerDifference).toBeLessThan(1);
    if (centerDifference < 1) {
      expect(action.x - heading.x - heading.width).toBeCloseTo(24, 1);
    } else {
      expect(action.y).toBeGreaterThanOrEqual(heading.y + heading.height);
      expect(action.x).toBeCloseTo(heading.x, 1);
    }
    for (const box of [heading, action]) {
      expect(box.x + box.width <= tools.x || box.x >= tools.x + tools.width
        || box.y + box.height <= tools.y || box.y >= tools.y + tools.height).toBe(true);
    }
    expect(await header.evaluate(element => element.scrollWidth <= element.clientWidth)).toBe(true);
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    await page.screenshot({ path: testInfo.outputPath(`keys-heading-${locale}.png`) });
  });
}

test("narrow desktop keeps headings clear of the extended utility header", async ({ page }) => {
  await page.setViewportSize({ width: 640, height: 900 });
  for (const path of ["/app/keys"]) {
    await page.goto(path);
    const heading = page.getByRole("heading", { name: "API-ключи", exact: true });
    await expect(heading).toHaveCSS("height", "36px");
    const title = (await heading.boundingBox())!;
    const action = (await page.getByRole("button", { name: "Создать ключ", exact: true }).boundingBox())!;
    const tools = (await page.locator(".hosted-tools").boundingBox())!;
    expect(action.y).toBeGreaterThanOrEqual(title.y + title.height);
    expect(title.x + title.width).toBeLessThanOrEqual(tools.x);
    expect(await page.locator(".relay-content").evaluate(element => element.scrollWidth <= element.clientWidth)).toBe(true);
  }
});

test("tablet touch controls keep forty-four pixel targets", async ({ browser }, testInfo) => {
  test.skip(testInfo.project.name !== "desktop-light", "One dedicated coarse-pointer viewport");
  const context = await browser.newContext({ viewport: { width: 1024, height: 1000 }, hasTouch: true, baseURL: testInfo.project.use.baseURL! });
  try {
    const page = await context.newPage();
    await page.goto("/admin/connections");
    for (const control of [page.getByRole("button", { name: "Добавить подключение", exact: true }), page.getByRole("button", { name: /^Действия:/ }).first(), page.getByRole("button", { name: /^Уведомления:/ })]) {
      await expect(control).toHaveCSS("height", "44px");
    }
    await page.goto("/app/keys");
    for (const control of [page.getByRole("button", { name: /^(Тёмная|Светлая) тема$/ }), page.locator(".language-switcher").getByRole("combobox")]) {
      await expect(control).toHaveCSS("height", "44px");
      await expect(control).toHaveCSS("width", "44px");
    }
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  } finally {
    await context.close();
  }
});
