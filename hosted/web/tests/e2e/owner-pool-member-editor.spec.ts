import { expect, test } from "@playwright/test";
import type { Locator, Page } from "@playwright/test";

async function openEditor(page: Page) {
  await page.goto("/admin/pools");
  const trigger = page.getByRole("button", { name: "Редактировать Code account", exact: true });
  await trigger.click();
  return { dialog: page.getByRole("dialog", { name: "Настройки участника", exact: true }), trigger };
}

async function checkBounds(dialog: Locator, page: Page) {
  const box = (await dialog.boundingBox())!;
  expect(box.x).toBeGreaterThanOrEqual(0);
  expect(box.y).toBeGreaterThanOrEqual(0);
  expect(box.x + box.width).toBeLessThanOrEqual(page.viewportSize()!.width);
  expect(box.y + box.height).toBeLessThanOrEqual(page.viewportSize()!.height);
  expect(await dialog.locator(".dialog-body, .owner-member-panel, .owner-member-setting, .owner-member-model-group").evaluateAll(nodes => nodes.every(node => node.scrollWidth <= node.clientWidth + 1))).toBe(true);
  expect(await dialog.locator("button, input").evaluateAll(nodes => nodes.every(node => {
    const box = node.getBoundingClientRect(), parent = node.closest("dialog")!.getBoundingClientRect();
    return box.left >= parent.left && box.right <= parent.right && box.top >= parent.top && box.bottom <= parent.bottom;
  }))).toBe(true);
}

test("pencil opens Desktop-style model switches and settings with bounded layout", async ({ page }, info) => {
  const { dialog, trigger } = await openEditor(page);
  await expect(trigger).toHaveAttribute("aria-haspopup", "dialog");
  await expect(trigger.locator('svg[data-icon="edit"]')).toHaveCount(1);
  await expect(page.getByRole("menu")).toHaveCount(0);
  await expect(dialog.getByRole("tab", { name: "Модели", exact: true })).toBeFocused();
  const model = dialog.getByRole("switch", { name: "Разрешить demo-gpt", exact: true });
  await expect(model).toBeChecked();
  const thumb = await model.evaluate(el => {
    const input = getComputedStyle(el), knob = getComputedStyle(el, "::after");
    return { before: getComputedStyle(el, "::before").content, left: parseFloat(knob.left), top: parseFloat(knob.top), width: parseFloat(knob.width), translate: new DOMMatrix(knob.transform).m41, innerWidth: el.clientWidth, display: input.display };
  });
  expect(thumb.before).toBe("none");
  expect(thumb.display).toBe("block");
  expect(thumb.left + thumb.width + thumb.translate).toBeLessThan(thumb.innerWidth);
  await checkBounds(dialog, page);
  await page.screenshot({ path: info.outputPath("member-models.png"), animations: "disabled" });
  await dialog.getByRole("tab", { name: "Настройки", exact: true }).click();
  await expect(dialog.getByRole("switch", { name: "Приостановить", exact: true })).not.toBeChecked();
  await expect(dialog.getByRole("textbox", { name: "Стоимость покупки", exact: true })).toHaveValue("5");
  await checkBounds(dialog, page);
  await page.screenshot({ path: info.outputPath("member-settings.png"), animations: "disabled" });
  await page.keyboard.press("Escape");
  await expect(dialog).toHaveCount(0);
  await expect(trigger).toBeFocused();
});

test("saved preview edits persist across navigation without modifying the catalog", async ({ page }) => {
  const { dialog, trigger } = await openEditor(page);
  const requests: string[] = [];
  page.on("request", request => { if (request.method() !== "GET") requests.push(request.method()); });
  await dialog.getByRole("switch", { name: "Разрешить demo-gpt", exact: true }).uncheck();
  await expect(dialog.locator(".owner-member-model-heading > span")).toHaveText("0 / 1");
  await dialog.getByRole("tab", { name: "Настройки", exact: true }).click();
  await dialog.getByRole("switch", { name: "Приостановить", exact: true }).check();
  await dialog.getByRole("textbox", { name: "Стоимость покупки", exact: true }).fill("12,5");
  await dialog.getByRole("button", { name: "Сохранить", exact: true }).click();
  const card = page.getByRole("article", { name: "Code account", exact: true });
  await expect(card).toHaveAttribute("data-status", "disabled");
  await expect(card.locator(".owner-account-values dd").last()).toHaveText("80 %");
  await trigger.click();
  await expect(dialog.getByRole("switch", { name: "Разрешить demo-gpt", exact: true })).not.toBeChecked();
  await dialog.getByRole("tab", { name: "Настройки", exact: true }).click();
  await expect(dialog.getByRole("textbox", { name: "Стоимость покупки", exact: true })).toHaveValue("12.5");
  await dialog.getByRole("button", { name: "Отмена", exact: true }).click();
  await page.getByRole("button", { name: "Настройки ротации пула", exact: true }).click();
  await page.keyboard.press("Escape");
  await trigger.click();
  await expect(dialog.getByRole("switch")).not.toBeChecked();
  await page.keyboard.press("Escape");
  expect(requests).toEqual([]);
  await page.reload();
  await trigger.click();
  await expect(dialog.getByRole("switch")).toBeChecked();
});

test("cancel and Escape discard drafts and cost errors block saving", async ({ page }) => {
  const { dialog, trigger } = await openEditor(page);
  await dialog.getByRole("switch").uncheck();
  await dialog.getByRole("button", { name: "Отмена", exact: true }).click();
  await trigger.click();
  await expect(dialog.getByRole("switch")).toBeChecked();
  await page.keyboard.press("End");
  await expect(dialog.getByRole("tab", { name: "Настройки", exact: true })).toBeFocused();
  const cost = dialog.getByRole("textbox", { name: "Стоимость покупки", exact: true });
  await cost.fill("-1");
  await expect(cost).toHaveAttribute("aria-invalid", "true");
  await expect(dialog.getByRole("alert")).toBeVisible();
  await expect(dialog.getByRole("button", { name: "Сохранить", exact: true })).toBeDisabled();
  await cost.fill("");
  await expect(dialog.getByRole("button", { name: "Сохранить", exact: true })).toBeEnabled();
  await page.keyboard.press("Escape");
  await trigger.click();
  await dialog.getByRole("tab", { name: "Настройки", exact: true }).click();
  await expect(cost).toHaveValue("5");
});

for (const [locale, edit, settings, cost] of [["en", "Edit Code account", "Settings", "Purchase cost"], ["zh-CN", "编辑 Code account", "设置", "购买成本"]] as const) {
  test(`member editor localizes and fits in ${locale}`, async ({ page }) => {
    await page.addInitScript(value => localStorage.setItem("relay-hosted-locale", value), locale);
    await page.goto("/admin/pools");
    await page.getByRole("button", { name: edit, exact: true }).click();
    const dialog = page.getByRole("dialog");
    await dialog.getByRole("tab", { name: settings, exact: true }).click();
    await expect(dialog.getByRole("textbox", { name: cost, exact: true })).toHaveValue("5");
    await checkBounds(dialog, page);
    expect(await dialog.innerText()).not.toMatch(/pool\.|Настройки|Стоимость/);
  });
}

for (const inventory of [[], Array.from({ length: 40 }, (_, index) => `test-model-${String(index).padStart(2, "0")}-${"long-name-".repeat(8)}`)]) {
  test(`member inventory of ${inventory.length} models supports empty/search/scroll states`, async ({ page }, info) => {
    await page.route("**/src/preview/snapshot.ts", async route => {
      const response = await route.fetch();
      await route.fulfill({ response, body: `${await response.text()}\npreviewSnapshot.owner.connections.find(item => item.id === 'demo_account_1').models = ${JSON.stringify(inventory)};` });
    });
    const { dialog } = await openEditor(page);
    if (!inventory.length) {
      await expect(dialog.getByRole("status")).toHaveText("Модели пока не получены");
      await expect(dialog.getByRole("switch")).toHaveCount(0);
      await expect(dialog.getByRole("searchbox")).toHaveCount(0);
      await checkBounds(dialog, page);
      return;
    }
    const panel = dialog.getByRole("tabpanel");
    await expect(dialog.getByRole("switch")).toHaveCount(inventory.length);
    const dimensions = await panel.evaluate(node => ({ height: node.clientHeight, total: node.scrollHeight }));
    expect(dimensions.total).toBeGreaterThan(dimensions.height);
    const header = await dialog.locator(".dialog-header").boundingBox();
    const footer = await dialog.locator("footer").boundingBox();
    await dialog.getByRole("switch").last().scrollIntoViewIfNeeded();
    expect(await panel.evaluate(node => node.scrollTop)).toBeGreaterThan(0);
    expect(await dialog.locator(".dialog-header").boundingBox()).toEqual(header);
    expect(await dialog.locator("footer").boundingBox()).toEqual(footer);
    const search = dialog.getByRole("searchbox", { name: "Найти модель", exact: true });
    await search.fill("no-such-model");
    await expect(dialog.getByRole("status")).toHaveText("Модели не найдены");
    await search.fill("test-model-39-");
    await expect(dialog.getByRole("switch")).toHaveCount(1);
    await dialog.getByRole("switch").uncheck();
    await checkBounds(dialog, page);
    await search.fill("");
    await expect(dialog.locator(".owner-member-model-heading > span")).toHaveText("39 / 40");
    await page.screenshot({ path: info.outputPath("member-long-inventory.png"), animations: "disabled" });
    await dialog.getByRole("button", { name: "Сохранить", exact: true }).click();
    await page.getByRole("button", { name: "Редактировать Code account", exact: true }).click();
    await expect(dialog.getByRole("switch").last()).not.toBeChecked();
  });
}
