import { expect, test } from "@playwright/test";
import type { Locator, Page } from "@playwright/test";

async function openRules(page: Page) {
  await page.goto("/admin/pools");
  await page.getByRole("button", { name: "Правила моделей", exact: true }).click();
  return page.getByRole("region", { name: "Правила моделей", exact: true });
}

async function navigateOwner(page: Page, name: string) {
  const button = page.locator(".relay-sidebar").getByRole("button", { name, exact: true });
  if (!await button.isVisible()) await page.getByRole("button", { name: "Меню", exact: true }).click();
  await button.click();
}

async function expectFits(rules: Locator) {
  expect(await rules.evaluate(root => {
    const box = root.getBoundingClientRect();
    return box.left >= 0 && box.right <= innerWidth && root.scrollWidth <= root.clientWidth + 1
      && [...root.querySelectorAll("button, input")].filter(el => el.getClientRects().length).every(el => {
        const rect = el.getBoundingClientRect();
        return rect.left >= box.left && rect.right <= box.right;
      });
  })).toBe(true);
}

test("model rules match the local list with only pricing and enablement", async ({ page }, info) => {
  const rules = await openRules(page);
  await expect(rules.locator(".owner-model-row")).toHaveCount(3);
  await expect(rules.getByRole("switch")).toHaveCount(3);
  await expect(rules.getByRole("textbox")).toHaveCount(12);
  for (const field of await rules.getByRole("textbox").all()) await expect(field).toBeVisible();
  await expect(rules.getByRole("textbox").first()).toHaveCSS("border-top-color", "rgba(0, 0, 0, 0)");
  await expect(rules.locator(".owner-model-offer .option-trigger")).toHaveCSS("background-color", "rgba(0, 0, 0, 0)");
  await expect(rules.locator(".owner-model-offer .option-trigger")).toHaveCSS("border-top-color", "rgba(0, 0, 0, 0)");
  await expect(rules.locator(".owner-model-price-button, .owner-model-price-editor")).toHaveCount(0);
  await expect(rules.locator(".owner-model-group")).toHaveCount(3);
  await expect(rules.locator('[data-model-id="demo-gpt"] .owner-model-identity')).toHaveText("GPT");
  await expect(rules.locator('[data-group-id="catalog-openai"] .owner-model-group')).toHaveText("OpenAI1");
  const groupHeading = rules.locator('[data-group-id="catalog-openai"] .owner-model-group-heading');
  expect(await groupHeading.evaluate(heading => {
    const [toggle, drag, label, count] = [...heading.children];
    return toggle?.classList.contains("owner-model-group-toggle") && drag?.classList.contains("owner-model-drag-handle")
      && label?.tagName === "STRONG" && count?.tagName === "SMALL";
  })).toBe(true);
  const collapse = groupHeading.locator(".owner-model-group-toggle");
  await collapse.hover();
  expect((await collapse.boundingBox())!.width).toBeLessThanOrEqual(44);
  await expect(collapse).toHaveCSS("background-color", await rules.evaluate(element => getComputedStyle(element).backgroundColor));
  await expect(groupHeading).toHaveCSS("background-color", "rgba(0, 0, 0, 0)");
  await page.mouse.move(0, 0);
  if (info.project.name === "desktop-light") {
    const heights = await rules.locator(".owner-model-row").evaluateAll(rows => rows.map(row => row.getBoundingClientRect().height));
    expect(Math.max(...heights)).toBeLessThanOrEqual(84);
    expect(Math.max(...heights) - Math.min(...heights)).toBeLessThanOrEqual(2);
  }
  await expect(page.getByRole("button", { name: "Настройки ротации пула", exact: true })).toBeHidden();
  await expect(rules.getByRole("radiogroup")).toHaveCount(0);
  await expect(page.getByRole("dialog")).toHaveCount(0);
  await expectFits(rules);
  await page.screenshot({ path: info.outputPath("model-rules.png"), animations: "disabled" });
  expect(await rules.locator(".owner-model-row").evaluateAll(rows => rows.every(row => {
    const name = row.querySelector("th")!;
    return getComputedStyle(name).textAlign === "center" && [...row.querySelectorAll("input")].every(input => {
      const cell = input.closest("td")!.getBoundingClientRect();
      const control = input.getBoundingClientRect();
      return input.type === "checkbox"
        ? getComputedStyle(row).display === "grid" || Math.abs(cell.left + cell.width / 2 - control.left - control.width / 2) < 2
        : getComputedStyle(input).textAlign === "center" && Math.abs(cell.left + cell.width / 2 - control.left - control.width / 2) < 2;
    });
  }))).toBe(true);
  const writes: string[] = [];
  page.on("request", request => { if (request.method() !== "GET") writes.push(request.method()); });
  await rules.getByRole("switch", { name: "Модель GPT", exact: true }).uncheck();
  await expect(rules.locator('[data-model-id="demo-gpt"]')).toHaveAttribute("data-enabled", "false");
  await page.getByRole("button", { name: "Участники", exact: true }).click();
  await expect(page.getByRole("article", { name: "Code account", exact: true })).toBeVisible();
  await page.getByRole("button", { name: "Правила моделей", exact: true }).click();
  await expect(rules.getByRole("switch", { name: "Модель GPT", exact: true })).not.toBeChecked();
  await navigateOwner(page, "Модели");
  await expect(page.locator('.catalog-row[data-model-id="demo-gpt"]')).toHaveCount(0);
  await navigateOwner(page, "Пул");
  await expect(rules.getByRole("switch", { name: "Модель GPT", exact: true })).not.toBeChecked();
  await rules.getByRole("switch", { name: "Модель GPT", exact: true }).focus();
  await page.keyboard.press("Space");
  await expect(rules.getByRole("switch", { name: "Модель GPT", exact: true })).toBeChecked();
  expect(writes).toEqual([]);
});

test("inline prices preserve exact values and isolate existing price variants", async ({ page }, info) => {
  const rules = await openRules(page);
  const editor = rules.locator('.owner-model-row[data-model-id="demo-gpt"]');
  const input = editor.getByRole("textbox", { name: "Цена Вход для GPT", exact: true });
  await expect(input).toHaveValue("0.4");
  await input.fill("0,123456");
  await input.press("Enter");
  await expect(input).toHaveValue("0.123456");
  await input.fill("-1");
  await input.press("Tab");
  await expect(input).toHaveAttribute("aria-invalid", "true");
  await expect(editor.getByRole("alert")).toBeVisible();
  await input.focus();
  await input.press("Escape");
  await expect(input).toHaveValue("0.123456");
  await expect(editor.getByRole("alert")).toHaveCount(0);
  await editor.locator(".option-trigger").click();
  await page.getByRole("option", { name: "Flex", exact: true }).click();
  await expect(input).toHaveValue("0.2");
  await input.fill("0");
  await input.press("Enter");
  await expect(input).toHaveValue("0");
  await editor.locator(".option-trigger").click();
  await page.getByRole("option", { name: /^≤/ }).click();
  await expect(input).toHaveValue("0.123456");
  const output = editor.getByRole("textbox", { name: "Цена Выход для GPT", exact: true });
  await output.fill("");
  await output.press("Tab");
  await expect(output).toHaveValue("");
  await expectFits(rules);
  await page.screenshot({ path: info.outputPath("model-prices.png"), animations: "disabled" });
  await navigateOwner(page, "Модели");
  const catalog = page.locator('.catalog-row[data-model-id="demo-gpt"]');
  await expect(catalog.locator('[data-component="input"] strong')).toHaveText("$0,123456");
  await expect(catalog.locator('[data-component="output"] strong')).toHaveText("—");
  await expect(catalog.locator(".catalog-saving")).toHaveCount(0);
});

test("context labels show supplied bounds and leave missing context blank", async ({ page }, info) => {
  await page.route("**/src/preview/snapshot.ts", async route => {
    const response = await route.fetch();
    await route.fulfill({ response, body: `${await response.text()}\nconst gpt = previewSnapshot.models.find(model => model.id === "demo-gpt"); const base = gpt.pricingVariants[0]; gpt.pricingVariants = [{ ...base, id: "base", contextBand: "base" }, { ...base, id: "middle", contextBand: "above100k" }, { ...base, id: "last", contextBand: "above200k" }]; gpt.defaultPricingVariantId = "middle"; const claude = previewSnapshot.models.find(model => model.id === "demo-claude"); claude.contextTokens = 1000000; claude.pricingVariants = [{ id: "single", tier: "standard", contextBand: "base", rates: claude.rates }];` });
  });
  const rules = await openRules(page);
  const context = rules.locator('[data-model-id="demo-gpt"] .option-trigger');
  await expect(context).toHaveText(/> 100\s000 – ≤ 200\s000/);
  for (const modelId of ["demo-claude", "demo-gemini"]) {
    const row = rules.locator(`[data-model-id="${modelId}"]`);
    await expect(row.locator(".owner-model-offer-value")).toHaveText("");
    await expect(row.getByRole("combobox")).toHaveCount(0);
  }
  await context.click();
  await expect(page.getByRole("option")).toHaveCount(3);
  await expect(page.getByRole("option", { name: /Standard/ })).toHaveCount(0);
  await page.getByRole("option", { name: /^> 200/ }).click();
  await expect(context).toHaveText(/^> 200\s000$/);
  await expectFits(rules);
  await page.screenshot({ path: info.outputPath("model-context-bounds.png"), animations: "disabled" });
});

test("new catalog models appear without hardcoding and disabled rows stay editable", async ({ page }) => {
  await page.route("**/src/preview/snapshot.ts", async route => {
    const response = await route.fetch();
    await route.fulfill({ response, body: `${await response.text()}\nconst added = { id: "new-model", name: "New discovered model with a long descriptive name", publisher: "New provider", protocols: [], description: "", revision: "test", rates: { input: 9223372036854775807n, output: null } }; previewSnapshot.models.push(added); previewSnapshot.owner.connections[0].models.push(added.id); previewSnapshot.owner.connections[0].paused = true; previewSnapshot.owner.modelRules = { "new-model": { enabled: false } };` });
  });
  const rules = await openRules(page);
  const row = rules.locator('[data-model-id="new-model"]');
  await expect(row).toBeVisible();
  await expect(row.getByRole("switch")).not.toBeChecked();
  await expect(rules.getByRole("textbox", { name: /Цена Вход для New discovered/ })).toHaveValue("9223372036854.775807");
  await expectFits(rules);
});

test("an empty pool does not manufacture model rows", async ({ page }) => {
  await page.route("**/src/preview/snapshot.ts", async route => {
    const response = await route.fetch();
    await route.fulfill({ response, body: `${await response.text()}\npreviewSnapshot.owner.connections.forEach(member => member.inPool = false);` });
  });
  const rules = await openRules(page);
  await expect(rules.locator(".owner-model-row")).toHaveCount(0);
  await expect(rules.getByRole("status")).toHaveText("В пуле пока нет моделей");
});

test("cache write durations remain directly editable without expanding a row", async ({ page }, info) => {
  await page.route("**/src/preview/snapshot.ts", async route => {
    const response = await route.fetch();
    await route.fulfill({ response, body: `${await response.text()}\nconst ttl = previewSnapshot.models.find(model => model.id === "demo-claude"); ttl.pricingVariants = undefined; ttl.rates = { input: 600000n, output: 2000000n, cacheRead: 120000n, cacheWrite5m: 750000n, cacheWrite1h: 1500000n };` });
  });
  const rules = await openRules(page);
  const row = rules.locator('.owner-model-row[data-model-id="demo-claude"]');
  const short = row.getByRole("textbox", { name: /5 мин/ });
  const long = row.getByRole("textbox", { name: /1 час/ });
  await expect(short).toBeVisible();
  await expect(long).toHaveValue("1.5");
  await short.fill("0.8");
  await short.press("Enter");
  await expect(short).toHaveValue("0.8");
  await expect(long).toHaveValue("1.5");
  await expectFits(rules);
  await page.screenshot({ path: info.outputPath("model-cache-durations.png"), fullPage: true, animations: "disabled" });
});

test("model rules support English and Chinese without extra runtime controls", async ({ page }) => {
  for (const [locale, title] of [["en", "Model rules"], ["zh-CN", "模型规则"]]) {
    await page.goto("/admin/pools");
    await page.evaluate(value => localStorage.setItem("relay-hosted-locale", value!), locale);
    await page.reload();
    await page.getByRole("button", { name: title!, exact: true }).click();
    const rules = page.getByRole("region", { name: title!, exact: true });
    await expect(rules.getByRole("textbox")).toHaveCount(12);
    await expect(rules.locator('.owner-model-row[data-model-id="demo-gpt"]').getByRole("textbox")).toHaveCount(4);
    await expect(rules.getByRole("radiogroup")).toHaveCount(0);
    await expectFits(rules);
  }
});

async function addSibling(page: Page) {
  await page.context().route("**/src/preview/snapshot.ts", async route => {
    const response = await route.fetch();
    await route.fulfill({ response, body: `${await response.text()}\nconst sibling = { ...previewSnapshot.models[0], id: "demo-gpt-small", name: "GPT Small" }; previewSnapshot.models.push(sibling); previewSnapshot.owner.connections[0].models.push(sibling.id);` });
  });
}

async function modelOrder(root: Locator) {
  return root.locator(".owner-model-row, .catalog-row").evaluateAll(rows => rows.map(row => row.getAttribute("data-model-id")));
}

test("company and model keyboard ordering is shared with customer tabs, prices and enablement", async ({ page }) => {
  await addSibling(page);
  const rules = await openRules(page);
  const company = rules.getByRole("button", { name: "Переместить компанию OpenAI", exact: true });
  await company.focus();
  await company.press("ArrowDown");
  expect(await modelOrder(rules)).toEqual(["demo-claude", "demo-gpt", "demo-gpt-small", "demo-gemini"]);
  await company.press("End");
  expect(await modelOrder(rules)).toEqual(["demo-claude", "demo-gemini", "demo-gpt", "demo-gpt-small"]);
  await company.press("Home");
  const model = rules.getByRole("button", { name: "Переместить модель GPT", exact: true });
  await model.focus();
  await model.press("End");
  await model.press("ArrowDown");
  expect(await modelOrder(rules)).toEqual(["demo-gpt-small", "demo-gpt", "demo-claude", "demo-gemini"]);
  await expect(model).toBeFocused();
  const customer = await page.context().newPage();
  await customer.goto("/app/models");
  const catalog = customer.locator(".models-page");
  await expect.poll(() => modelOrder(catalog)).toEqual(await modelOrder(rules));
  await expect(catalog.locator(".catalog-company")).toHaveText(["OpenAI", "Anthropic", "Google"]);
  const input = rules.getByRole("textbox", { name: "Цена Вход для GPT", exact: true });
  await input.fill("0.345678");
  await input.press("Enter");
  await expect(catalog.locator('[data-model-id="demo-gpt"] [data-component="input"] strong')).toHaveText("$0,345678");
  const enabled = rules.getByRole("switch", { name: "Модель GPT", exact: true });
  await enabled.uncheck();
  await expect(catalog.locator('[data-model-id="demo-gpt"]')).toHaveCount(0);
  expect(await modelOrder(rules)).toEqual(["demo-gpt-small", "demo-gpt", "demo-claude", "demo-gemini"]);
  await enabled.check();
  await expect.poll(() => modelOrder(catalog)).toEqual(await modelOrder(rules));
  await customer.reload();
  await expect.poll(() => modelOrder(catalog)).toEqual(await modelOrder(rules));
  await expect(catalog.locator('[data-model-id="demo-gpt"] [data-component="input"] strong')).toHaveText("$0,345678");
  await customer.close();
});

async function dragTo(page: Page, source: Locator, target: Locator, cancel = false) {
  // A geometrically visible handle may be covered by the sticky mobile header.
  // Require real pointer actionability before using raw mouse coordinates.
  await source.click({ trial: true });
  const from = (await source.boundingBox())!;
  await page.mouse.move(from.x + from.width / 2, from.y + from.height / 2);
  await page.mouse.down();
  await target.evaluate(element => element.scrollIntoView({ block: "center" }));
  const to = (await target.boundingBox())!;
  await page.mouse.move(to.x + to.width / 2, to.y + to.height / 2, { steps: 5 });
  if (cancel) await page.keyboard.press("Escape");
  await page.mouse.up();
}

test("drag handles move full company blocks and reject cross-company model drops", async ({ page }, info) => {
  await addSibling(page);
  const rules = await openRules(page);
  const openai = rules.getByRole("button", { name: "Переместить компанию OpenAI", exact: true });
  const google = rules.getByRole("button", { name: "Переместить компанию Google", exact: true });
  await dragTo(page, openai, google);
  expect(await modelOrder(rules)).toEqual(["demo-claude", "demo-gemini", "demo-gpt", "demo-gpt-small"]);
  const first = rules.getByRole("button", { name: "Переместить модель GPT", exact: true });
  const sibling = rules.getByRole("button", { name: "Переместить модель GPT Small", exact: true });
  const input = rules.getByRole("textbox", { name: "Цена Вход для GPT", exact: true });
  await input.fill("0.987654");
  await input.press("Enter");
  await dragTo(page, first, sibling);
  expect(await modelOrder(rules)).toEqual(["demo-claude", "demo-gemini", "demo-gpt-small", "demo-gpt"]);
  await expect(input).toHaveValue("0.987654");
  await dragTo(page, first, rules.locator('[data-model-id="demo-claude"] .owner-model-identity'));
  expect(await modelOrder(rules)).toEqual(["demo-claude", "demo-gemini", "demo-gpt-small", "demo-gpt"]);
  await dragTo(page, first, sibling, true);
  expect(await modelOrder(rules)).toEqual(["demo-claude", "demo-gemini", "demo-gpt-small", "demo-gpt"]);
  await expect(rules.locator('[data-dragging="true"], [data-drop-target="true"]')).toHaveCount(0);
  await expectFits(rules);
  await openai.scrollIntoViewIfNeeded();
  await page.screenshot({ path: info.outputPath("company-model-order.png"), animations: "disabled" });
});

test("touch dragging reorders companies without changing prices", async ({ page }) => {
  const rules = await openRules(page);
  const source = rules.getByRole("button", { name: "Переместить компанию OpenAI", exact: true });
  const target = rules.getByRole("button", { name: "Переместить компанию Anthropic", exact: true });
  const cdp = await page.context().newCDPSession(page);
  await source.scrollIntoViewIfNeeded();
  const from = (await source.boundingBox())!;
  await cdp.send("Input.dispatchTouchEvent", { type: "touchStart", touchPoints: [{ x: from.x + from.width / 2, y: from.y + from.height / 2 }] });
  await target.scrollIntoViewIfNeeded();
  const to = (await target.boundingBox())!;
  await cdp.send("Input.dispatchTouchEvent", { type: "touchMove", touchPoints: [{ x: to.x + to.width / 2, y: to.y + to.height / 2 }] });
  await cdp.send("Input.dispatchTouchEvent", { type: "touchEnd", touchPoints: [] });
  await expect.poll(() => modelOrder(rules)).toEqual(["demo-claude", "demo-gpt", "demo-gemini"]);
  await expect(rules.getByRole("textbox", { name: "Цена Вход для GPT", exact: true })).toHaveValue("0.4");
  await cdp.detach();
});

test("company collapse is visual only, preserves edits and supports reordering while collapsed", async ({ page }, info) => {
  await addSibling(page);
  const rules = await openRules(page);
  const group = rules.locator('[data-group-id="catalog-openai"]');
  const input = group.getByRole("textbox", { name: "Цена Вход для GPT", exact: true });
  await input.fill("0.654321");
  await input.press("Enter");
  const expandedHeight = (await group.boundingBox())!.height;
  const toggle = group.locator(".owner-model-group-toggle");
  await toggle.click();
  await expect(toggle).toHaveAttribute("aria-expanded", "false");
  await expect(toggle).toHaveAccessibleName("Развернуть модели OpenAI");
  await expect(group.locator(".owner-model-row:visible")).toHaveCount(0);
  await expect(rules.locator(".owner-model-row:visible")).toHaveCount(2);
  expect((await group.boundingBox())!.height).toBeLessThan(expandedHeight);
  expect(await toggle.evaluate(button => button.getAttribute("aria-controls")!.split(" ").every(id => document.getElementById(id)?.hidden))).toBe(true);
  const customer = await page.context().newPage();
  await customer.goto("/app/models");
  await expect(customer.locator('.catalog-row[data-model-id="demo-gpt"]')).toBeVisible();
  await expect(customer.locator('.catalog-row[data-model-id="demo-gpt-small"]')).toBeVisible();
  await expect(customer.locator('[data-model-id="demo-gpt"] [data-component="input"] strong')).toHaveText("$0,654321");
  await customer.close();
  await page.getByRole("button", { name: "Участники", exact: true }).click();
  await page.getByRole("button", { name: "Правила моделей", exact: true }).click();
  await expect(toggle).toHaveAttribute("aria-expanded", "false");
  await dragTo(page, group.getByRole("button", { name: "Переместить компанию OpenAI", exact: true }), rules.getByRole("button", { name: "Переместить компанию Google", exact: true }));
  expect(await modelOrder(rules)).toEqual(["demo-claude", "demo-gemini", "demo-gpt", "demo-gpt-small"]);
  await expect(toggle).toHaveAttribute("aria-expanded", "false");
  await toggle.focus();
  await page.keyboard.press("Enter");
  await expect(toggle).toHaveAttribute("aria-expanded", "true");
  await expect(input).toHaveValue("0.654321");
  await expect(group.getByRole("switch", { name: "Модель GPT", exact: true })).toBeChecked();
  await page.keyboard.press("Space");
  await expect(toggle).toHaveAttribute("aria-expanded", "false");
  for (const company of ["Anthropic", "Google"]) await rules.getByRole("button", { name: `Свернуть модели ${company}`, exact: true }).click();
  await expect(rules.locator(".owner-model-row:visible")).toHaveCount(0);
  await expectFits(rules);
  await rules.scrollIntoViewIfNeeded();
  await page.screenshot({ path: info.outputPath("collapsed-company-groups.png"), animations: "disabled" });
});
