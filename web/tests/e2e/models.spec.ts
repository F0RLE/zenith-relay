import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { previewModels } from "../../src/preview/models";
import { formatModelPrice, pricePresentation, selectedModelVariant } from "../../src/features/models/model";

async function fits(page: Page) {
  await page.evaluate(() => document.fonts.ready);
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  expect(await page.locator(".relay-content").evaluate(element => element.scrollWidth <= element.clientWidth)).toBe(true);
  const surfaces = page.locator(".models-page:visible :is(.page-heading, .catalog-frame, .catalog-model-heading, .catalog-choice-buttons, .catalog-choice-buttons button, .catalog-context, .catalog-row, .catalog-price)");
  for (const element of await surfaces.all()) {
    const bounds = await element.evaluate(node => ({ className: node.className, width: node.clientWidth, scroll: node.scrollWidth }));
    expect(bounds.scroll, `${bounds.className} overflows its ${bounds.width}px width`).toBeLessThanOrEqual(bounds.width + 1);
  }
  for (const row of await page.locator(".models-page:visible .catalog-row").all()) {
    if (await row.evaluate(element => getComputedStyle(element).display === "grid")) {
      const name = await row.locator("th").boundingBox();
      const protocol = await row.locator(".catalog-protocol-cell").boundingBox();
      expect(Math.abs(name!.x - protocol!.x)).toBeLessThan(1);
    } else {
      for (const cell of await row.locator(":scope > th, :scope > td").all()) {
        await expect(cell).toHaveCSS("text-align", "center");
      }
      for (const group of await row.locator(".catalog-model-heading, .catalog-price-main, .catalog-official, .catalog-context-value").all()) {
        await expect(group).toHaveCSS("justify-content", "center");
      }
    }
  }
}

test("models routes show one framed catalog with component-specific comparisons", async ({ page }, testInfo) => {
  const errors: string[] = [];
  page.on("pageerror", error => errors.push(error.message));
  for (const [name, path] of [["customer", "/app/models"], ["owner", "/admin/models"], ["preview", "/preview.html?page=models"]] as const) {
    await page.goto(path);
    const models = page.locator(".models-page:visible");
    await expect(models.getByRole("heading", { name: "Модели и цены", level: 1, exact: true })).toBeVisible();
    await expect(models.locator(".catalog-row")).toHaveCount(previewModels.length);
    await expect(models.locator(".catalog-offer, .catalog-intro, .catalog-list-heading")).toHaveCount(0);
    await expect(models.locator(".catalog-frame")).toHaveCount(1);
    await expect(models.locator(".catalog-toolbar, .catalog-controls")).toHaveCount(0);
    await expect(models.locator(".catalog-meta, .catalog-demo-note, .catalog-unit")).toHaveCount(0);
    await expect(models.locator(".page-heading button, .catalog-start")).toHaveCount(0);
    await expect(models.getByRole("button", { name: "Получить API-ключ", exact: true })).toHaveCount(0);
    await expect(models.locator(".catalog-tier, th button, th [role=group]")).toHaveCount(0);
    await expect(models.locator("th").filter({ hasText: "Standard" })).toHaveCount(0);
    for (const model of previewModels) {
      const row = models.locator(`[data-model-id="${model.id}"]`);
      await expect(row.locator(".catalog-protocols")).toHaveText(model.protocols.join(" · "));
      await expect(row.locator("th .catalog-protocols")).toHaveCount(0);
      for (const component of ["input", "output", "cacheRead", "cacheWrite"] as const) {
        const price = pricePresentation(selectedModelVariant(model) ?? model, component);
        const cell = row.locator(`[data-component="${component}"]`);
        await expect(cell.locator("strong")).toHaveText(formatModelPrice(price.price, "ru"));
        await expect(cell.locator(".catalog-saving")).toHaveCount(price.discount === null ? 0 : 1);
        await expect(cell.locator("s")).toHaveCount(price.official === null ? 0 : 1);
      }
    }
    await expect(page.getByRole("dialog")).toHaveCount(0);
    await fits(page);
    await page.screenshot({ path: testInfo.outputPath(`models-${name}.png`) });
  }
  expect(errors).toEqual([]);
});

test("desktop catalog columns keep equal spacing and prices share a row baseline", async ({ page }, testInfo) => {
  test.skip(testInfo.project.name !== "desktop-light", "Explicit desktop widths run once; other tests cover responsive projects.");
  await page.goto("/app/models");
  const rows = page.locator(".models-page:visible .catalog-row");
  await expect(rows).toHaveCount(previewModels.length);
  await page.evaluate(() => document.fonts.ready);
  for (const width of [1410, 1440, 1480, 1920]) {
    await page.setViewportSize({ width, height: 1000 });
    const grid = await page.locator(".catalog-table thead th").evaluateAll(headers => headers.slice(0, -1).map(header => {
      const bounds = header.getBoundingClientRect();
      return { width: bounds.width, center: bounds.x + bounds.width / 2 };
    }));
    expect(Math.max(...grid.map(column => column.width)) - Math.min(...grid.map(column => column.width))).toBeLessThan(0.2);
    const steps = grid.slice(1).map((column, index) => column.center - grid[index]!.center);
    expect(Math.max(...steps) - Math.min(...steps)).toBeLessThan(0.2);
    const heights: number[] = [];
    for (const row of await rows.all()) {
      heights.push((await row.boundingBox())!.height);
      const prices = await row.locator(".catalog-price-main").evaluateAll(values => values.map(value => value.getBoundingClientRect().top));
      expect(Math.max(...prices) - Math.min(...prices)).toBeLessThan(1);
    }
    expect(Math.max(...heights) - Math.min(...heights)).toBeLessThan(1);
    await expect(rows.locator("th button, .catalog-tier")).toHaveCount(0);
    for (const button of await rows.first().locator(".catalog-context .catalog-choice-buttons button").all()) {
      expect((await button.boundingBox())!.height).toBeLessThan(35);
    }
    await fits(page);
    if (width === 1440) await page.locator(".catalog-frame").screenshot({ path: testInfo.outputPath("models-even-spacing.png"), animations: "disabled" });
  }
});

test("context bands use published prices while tier selection stays out of the catalog", async ({ page }, testInfo) => {
  await page.goto("/app/models");
  const models = page.locator(".models-page:visible");
  const row = models.locator('[data-model-id="demo-gpt"]');
  const context = row.getByRole("group", { name: "Тарифный контекст модели GPT", exact: true });
  await expect(models.getByRole("searchbox")).toHaveCount(0);
  await expect(row).toHaveAttribute("data-variant-id", "standard-short");
  await expect(row.locator("th button, .catalog-tier")).toHaveCount(0);
  await expect(row.getByRole("combobox", { name: "Тариф модели GPT", exact: true })).toHaveCount(0);
  await expect(context.getByRole("combobox")).toHaveCount(0);
  await expect(context.getByRole("button")).toHaveText(["≤ 100 тыс.", "> 100 тыс."]);
  await expect(context).toHaveAccessibleDescription("Максимальное окно контекста: 128 тыс.");
  await expect(row.locator(".catalog-context-value")).not.toHaveText(/^128 тыс\./);
  const short = context.getByRole("button", { name: "Короткий · ≤ 100 тыс.", exact: true });
  const long = context.getByRole("button", { name: "Длинный · > 100 тыс.", exact: true });
  await expect(short).toHaveAttribute("aria-pressed", "true");
  const shortBox = (await short.boundingBox())!;
  const longBox = (await long.boundingBox())!;
  expect(shortBox.y).toBe(longBox.y);
  expect(longBox.x).toBeGreaterThan(shortBox.x);
  if (testInfo.project.name.startsWith("mobile")) expect(shortBox.height).toBeGreaterThanOrEqual(44);
  await long.focus();
  await page.keyboard.press("Space");
  await expect(long).toHaveAttribute("aria-pressed", "true");
  await expect(short).toHaveAttribute("aria-pressed", "false");
  await expect(row).toHaveAttribute("data-variant-id", "standard-long");
  await expect(row.locator('[data-component="input"] strong')).toHaveText("$0,8");
  await expect(page.getByRole("listbox")).toHaveCount(0);
  await fits(page);
  await page.screenshot({ path: testInfo.outputPath("models-context-inline.png"), animations: "disabled" });
  await short.focus();
  await page.keyboard.press("Enter");
  await expect(row).toHaveAttribute("data-variant-id", "standard-short");
  await expect(row.locator('[data-component="input"] strong')).toHaveText("$0,4");
  await expect(models.locator('[data-model-id="demo-claude"] [role="combobox"]')).toHaveCount(0);
  await expect(models.locator(".catalog-model-name")).toHaveText(["GPT", "Claude", "Gemini"]);
  await expect(page.getByRole("dialog")).toHaveCount(0);
  await fits(page);
  await models.locator(".catalog-table").scrollIntoViewIfNeeded();
  await page.screenshot({ path: testInfo.outputPath("models-catalog.png") });
});

test("server default tiers remain implicit and multiple context bands keep an anchored menu", async ({ page }, testInfo) => {
  // Isolated module fixture; never add synthetic combinations to the actual catalog.
  await page.route(/\/src\/preview\/models\.ts(?:\?.*)?$/, async route => {
    const response = await route.fetch();
    await route.fulfill({ response, body: `${await response.text()}\n{
      const model = previewModels[0];
      const base = model.pricingVariants[0];
      previewModels[0] = { ...model, contextTokens: 1000000, defaultPricingVariantId: "middle", pricingVariants: [
        { ...base, id: "last", contextBand: "above272k", rates: { input: 1200000n } },
        { ...base, id: "base", contextBand: "base" },
        { ...base, id: "middle", contextBand: "above128k", rates: { input: 800000n } },
        model.pricingVariants.find(variant => variant.tier === "flex")
      ] };
      previewModels.push({ ...previewModels[0], id: "test-bounded-pair", name: "Bounded pair", defaultPricingVariantId: "middle",
        pricingVariants: previewModels[0].pricingVariants.filter(variant => variant.id === "middle" || variant.id === "last") });
      previewModels.push({ ...model, id: "test-default-flex", name: "Published default", defaultPricingVariantId: "flex-short" });
    }` });
  });
  await page.goto("/app/models");
  const row = page.locator('.models-page:visible [data-model-id="demo-gpt"]');
  await expect(page.locator(".catalog-row th button, .catalog-tier")).toHaveCount(0);
  const context = row.getByRole("combobox", { name: "Тарифный контекст модели GPT", exact: true });
  await expect(row).toHaveAttribute("data-variant-id", "middle");
  await expect(context).toHaveText("> 128 тыс. · ≤ 272 тыс.");
  await context.scrollIntoViewIfNeeded();
  const before = await row.boundingBox();
  await context.click();
  expect(await row.boundingBox()).toEqual(before);
  await expect(page.getByRole("option")).toHaveText(["Короткий · ≤ 128 тыс.", "> 128 тыс. · ≤ 272 тыс.", "Длинный · > 272 тыс."]);
  const menu = (await page.getByRole("listbox").boundingBox())!;
  expect(menu.x).toBeGreaterThanOrEqual(0);
  expect(menu.x + menu.width).toBeLessThanOrEqual(page.viewportSize()!.width);
  expect(menu.y).toBeGreaterThanOrEqual(0);
  expect(menu.y + menu.height).toBeLessThanOrEqual(page.viewportSize()!.height);
  await fits(page);
  await page.screenshot({ path: testInfo.outputPath("models-three-contexts.png"), animations: "disabled" });
  await page.keyboard.press("End");
  await page.keyboard.press("Enter");
  await expect(row).toHaveAttribute("data-variant-id", "last");
  await expect(row.locator('[data-component="input"] strong')).toHaveText("$1,2");
  await expect(context).toBeFocused();
  await context.click();
  await page.keyboard.press("Home");
  await page.keyboard.press("Enter");
  await expect(row).toHaveAttribute("data-variant-id", "base");
  await expect(context).toBeVisible();
  const flex = page.locator('[data-model-id="test-default-flex"]');
  await expect(flex).toHaveAttribute("data-variant-id", "flex-short");
  await expect(flex.locator('[data-component="input"] strong')).toHaveText("$0,2");
  await expect(flex.locator('[data-component="cacheWrite"] strong')).toHaveText("—");
  await expect(flex.locator("s, .catalog-saving, th button")).toHaveCount(0);
  await expect(flex.locator(".catalog-context-value")).toHaveText("128 тыс.");
  const bounded = page.locator('[data-model-id="test-bounded-pair"]');
  await expect(bounded.getByRole("combobox")).toHaveCount(0);
  const boundedChoices = bounded.locator(".catalog-context .catalog-choice-buttons button");
  await expect(boundedChoices).toHaveCount(2);
  const pair = await boundedChoices.evaluateAll(buttons => buttons.map(button => button.getBoundingClientRect().y));
  expect(pair[0]).toBe(pair[1]);
  await boundedChoices.last().click();
  await expect(bounded).toHaveAttribute("data-variant-id", "last");
  await fits(page);
});

test("models copy exact IDs without text or layout shifts and keep failure recoverable", async ({ page }, testInfo) => {
  await page.goto("/app/models");
  await page.evaluate(() => Object.defineProperty(navigator, "clipboard", { configurable: true, value: { writeText: async (text: string) => { (window as unknown as { copiedModel: string }).copiedModel = text; } } }));
  const row = page.locator('.models-page:visible [data-model-id="demo-gpt"]');
  await row.scrollIntoViewIfNeeded();
  const copy = row.getByRole("button", { name: "Скопировать ID модели GPT", exact: true });
  const before = await row.boundingBox();
  const buttonBefore = await copy.boundingBox();
  await copy.click();
  expect(await page.evaluate(() => (window as unknown as { copiedModel: string }).copiedModel)).toBe("demo-gpt");
  await expect(row.getByRole("status")).toHaveClass("sr-only");
  await expect(row.locator(".copy-control .form-note")).toHaveCount(0);
  expect(await row.boundingBox()).toEqual(before);
  expect(await copy.boundingBox()).toEqual(buttonBefore);
  if (await row.evaluate(element => getComputedStyle(element).display === "table-row")) {
    const cell = await row.locator(".catalog-actions").boundingBox();
    const button = await copy.boundingBox();
    expect(cell).not.toBeNull();
    expect(button).not.toBeNull();
    expect(Math.abs(button!.x + button!.width / 2 - cell!.x - cell!.width / 2)).toBeLessThan(1);
    expect(Math.abs(button!.y + button!.height / 2 - cell!.y - cell!.height / 2)).toBeLessThan(1);
  }
  await page.mouse.move(0, 0);
  await page.locator(".models-page:visible .catalog-table").screenshot({ path: testInfo.outputPath("models-copy-centered.png") });
  await expect(row.getByRole("status")).toHaveCount(0);
  await page.evaluate(() => Object.defineProperty(navigator, "clipboard", { configurable: true, value: { writeText: async () => { throw new Error("Test clipboard refusal"); } } }));
  await row.getByRole("button", { name: "Скопировать ID модели GPT", exact: true }).click();
  await expect(row.getByRole("textbox")).toHaveValue("demo-gpt");
  await fits(page);
  await row.screenshot({ path: testInfo.outputPath("models-copy-fallback.png") });
});

for (const [locale, title] of [["en", "Models and pricing"], ["ru", "Модели и цены"], ["zh-CN", "模型与价格"]] as const) {
  test(`models ${locale} pricing stays localized and within the viewport`, async ({ page }, testInfo) => {
    await page.addInitScript(value => localStorage.setItem("relay-hosted-locale", value), locale);
    await page.goto("/app/models");
    const models = page.locator(".models-page:visible");
    await expect(models.getByRole("heading", { name: title, exact: true })).toBeVisible();
    const text = await models.innerText();
    if (locale !== "ru") expect(text).not.toMatch(/\p{Script=Cyrillic}/u);
    if (locale !== "zh-CN") expect(text).not.toMatch(/\p{Script=Han}/u);
    expect(text).not.toMatch(/modelCatalog\.|\{(?:price|percent|name)\}/);
    await fits(page);
    await models.locator(".catalog-table").scrollIntoViewIfNeeded();
    await page.screenshot({ path: testInfo.outputPath(`models-${locale}.png`) });
  });
}
