import { expect, test, type Locator } from "../bun-playwright";
import { installTauriMock } from "./tauri-mock";

const models = ["newer-model", "older-model", "provider-alias"];

for (const mode of ["local", "remote"] as const) {
  for (const kind of ["account", "source"] as const) {
    test(`${mode} ${kind} member policy retains inventory order across save and cancel`, async ({ page }) => {
      await installTauriMock(page, {
        mode, locale: "en", populated: true,
        serverModelOrder: models,
        accountModels: models,
        accountAllowedModels: ["provider-alias", "older-model"],
        accountExcludedModels: ["NEWER-MODEL"],
      });
      await page.goto("/");
      await page.getByRole("button", { name: "Pool", exact: true }).click();
      const open = page.getByRole("button", { name: `Pool member policy: ${kind === "account" ? "Personal Plus" : "Example compatible API"}`, exact: true });
      const dialog = page.getByRole("dialog", { name: "Pool member policy", exact: true });
      const order = () => dialog.locator("[data-member-model-id]").evaluateAll((rows) => rows.map((row) => row.getAttribute("data-member-model-id")));
      await open.click();
      expect(await order()).toEqual(models);
      const first = dialog.getByRole("switch", { name: "Allow newer-model", exact: true });
      await expect(first).toBeChecked({ checked: kind === "source" });
      await first.focus();
      await first.press("Space");
      await dialog.getByRole("tab", { name: "Settings", exact: true }).click();
      await dialog.getByRole("tab", { name: "Models", exact: true }).click();
      expect(await order()).toEqual(models);
      await expect(first).toBeChecked({ checked: kind === "account" });
      await dialog.getByRole("button", { name: "Save policy", exact: true }).click();
      await expect(dialog).toBeHidden();

      await open.click();
      expect(await order()).toEqual(models);
      await expect(first).toBeChecked({ checked: kind === "account" });
      await first.click();
      await dialog.getByRole("button", { name: "Cancel", exact: true }).click();
      await open.click();
      expect(await order()).toEqual(models);
      await expect(first).toBeChecked({ checked: kind === "account" });
    });
  }
}

test("member model search changes only the chosen model and preserves hidden selections", async ({ page }) => {
  const inventory = Array.from({ length: 12 }, (_, index) => `model-${index + 1}`);
  await installTauriMock(page, { mode: "local", populated: true, accountModels: inventory });
  await page.goto("/");
  await page.getByRole("button", { name: "Pool", exact: true }).click();
  const open = page.getByRole("button", { name: "Pool member policy: Personal Plus", exact: true });
  await open.click();
  const dialog = page.getByRole("dialog", { name: "Pool member policy", exact: true });
  const search = dialog.getByRole("searchbox", { name: "Find a model" });
  await search.fill("MODEL-12");
  await expect(dialog.getByRole("switch")).toHaveCount(1);
  await dialog.getByRole("switch", { name: "Allow model-12", exact: true }).uncheck();
  await search.fill("missing");
  await expect(dialog.getByText("No matching results", { exact: true })).toBeVisible();
  await search.clear();
  await expect(dialog.getByRole("switch", { checked: true })).toHaveCount(11);
  await dialog.getByRole("button", { name: "Save policy", exact: true }).click();
  await expect(dialog).toBeHidden();
  await open.click();
  await expect(dialog.getByRole("switch", { checked: true })).toHaveCount(11);
  await expect(dialog.getByRole("switch", { name: "Allow model-12", exact: true })).not.toBeChecked();
});

test("member groups retain excluded models and search opens their group", async ({ page }) => {
  const inventory = ["gpt-5.4", "gpt-5.4-mini", "claude-opus-4-8", "gemini-3.1-pro-preview", "grok-4.5", "glm-5.2", "provider-alias"];
  await installTauriMock(page, { mode: "local", populated: true, serverModelOrder: inventory, omittedPoolModels: ["claude-opus-4-8"] });
  await page.goto("/");
  await page.getByRole("button", { name: "Pool", exact: true }).click();
  const open = page.getByRole("button", { name: "Pool member policy: Example compatible API", exact: true });
  await open.click();
  const dialog = page.getByRole("dialog", { name: "Pool member policy", exact: true });
  const group = dialog.locator('[data-model-provider="anthropic"]');
  const model = group.getByRole("switch", { name: "Allow claude-opus-4-8", exact: true });
  await expect(model).toBeVisible();
  await model.uncheck();
  await group.locator("summary").click();
  await dialog.getByRole("searchbox").fill("CLAUDE");
  await expect(model).toBeVisible();
  await expect(dialog.locator(".member-model-group")).toHaveCount(1);
  await dialog.getByRole("searchbox").clear();
  await dialog.getByRole("button", { name: "Save policy", exact: true }).click();
  await expect(dialog).toBeHidden();
  await open.click();
  await expect(model).not.toBeChecked();
  await expect(group.locator("summary")).toContainText("0 / 1");
  await expect(dialog.locator('[data-model-provider="openai"] code')).toHaveText(["gpt-5.4", "gpt-5.4-mini"]);
  await dialog.getByRole("tab", { name: "Pricing", exact: true }).click();
  await expect(dialog.locator(".source-price-group").filter({ hasText: "Anthropic" }).getByRole("textbox", { name: "Input token price for claude-opus-4-8", exact: true })).toBeVisible();
});

async function expectDialogFits(dialog: Locator) {
  const metrics = await dialog.evaluate((element) => {
    const rect = element.getBoundingClientRect();
    const scrolls = [...element.querySelectorAll<HTMLElement>("*")].filter((item) =>
      /auto|scroll/.test(getComputedStyle(item).overflowY) && item.scrollHeight > item.clientHeight + 1,
    );
    return {
      fits: rect.left >= 0 && rect.right <= innerWidth && rect.top >= 36 && rect.bottom <= innerHeight,
      horizontal: [...element.querySelectorAll<HTMLElement>(".relay-dialog-body, [role=tabpanel], .member-model-rules li, .source-price-row, .member-editor-setting")].every((item) => item.scrollWidth <= item.clientWidth + 1),
      scrollCount: scrolls.length,
    };
  });
  expect(metrics.fits).toBe(true);
  expect(metrics.horizontal).toBe(true);
  expect(metrics.scrollCount).toBeLessThanOrEqual(1);
}

async function expectPriceInputsFit(dialog: Locator) {
  const fields = await dialog.locator(".source-price-input").evaluateAll((items) => items.map((item) => {
    const field = item.getBoundingClientRect();
    const input = item.querySelector("input")!.getBoundingClientRect();
    const currency = item.querySelector("span")!.getBoundingClientRect();
    return {
      contained: input.top >= field.top && input.bottom <= field.bottom
        && input.left >= currency.right && input.right <= field.right,
      inputOffset: Math.abs((input.top + input.bottom - field.top - field.bottom) / 2),
      currencyOffset: Math.abs((currency.top + currency.bottom - field.top - field.bottom) / 2),
    };
  }));
  expect(fields.length).toBeGreaterThan(0);
  for (const field of fields) {
    expect(field.contained).toBe(true);
    expect(field.inputOffset).toBeLessThanOrEqual(1);
    expect(field.currencyOffset).toBeLessThanOrEqual(1);
  }
}

for (const [width, height] of [[1160, 760], [840, 560], [740, 760], [640, 720], [390, 844]]) {
  for (const theme of ["light", "dark"] as const) {
    test(`member policy dialogs fit ${theme} ${width}x${height}`, async ({ page }) => {
      const inventory = ["gpt-5.4", "gpt-5.4-mini", "claude-opus-4-8", "provider/model-with-a-long-identifier-that-must-wrap-without-hiding-its-switch", ...models];
      await page.setViewportSize({ width, height });
      await installTauriMock(page, {
        mode: "local", locale: "ru", theme, populated: true,
        accountModels: inventory, serverModelOrder: inventory,
        sourceProtocolBindings: [
          { wireApi: "responses", adapter: "native", reasoningMode: "disabled", modelIds: inventory.filter((model) => !model.startsWith("claude-")) },
          { wireApi: "messages", adapter: "native", reasoningMode: "disabled", modelIds: ["claude-opus-4-8"] },
        ],
      });
      await page.goto("/");
      await page.getByRole("button", { name: "Пул", exact: true }).click();
      const dialog = page.getByRole("dialog", { name: "Правила участника пула", exact: true });
      for (const [kind, name] of [["account", "Personal Plus"], ["source", "Example compatible API"]]) {
        await page.getByRole("button", { name: `Правила участника пула: ${name}`, exact: true }).click();
        await expect(dialog.getByRole("switch")).toHaveCount(inventory.length);
        await expectDialogFits(dialog);
        await dialog.screenshot({ path: `output/playwright/member-${kind}-${theme}-${width}.png` });
        if (kind === "source") {
          await dialog.getByRole("tab", { name: "Цены", exact: true }).click();
          const cache = dialog.getByRole("textbox", { name: "Цена записи кэша на 1 час для claude-opus-4-8", exact: true });
          await expect(cache).toHaveCount(1);
          await cache.scrollIntoViewIfNeeded();
          await expectDialogFits(dialog);
          await expectPriceInputsFit(dialog);
          const rows = await dialog.locator(".source-price-row").evaluateAll((items) => items.map((item) => {
            const name = item.querySelector(".source-price-model")!.getBoundingClientRect();
            const fields = item.querySelector(".source-price-fields")!.getBoundingClientRect();
            const inputs = [...item.querySelectorAll(".source-price-input")].map((input) => input.getBoundingClientRect());
            const headings = [...item.closest(".source-price-table")!.querySelectorAll(".member-price-grid-head > div > span")].map((heading) => heading.getBoundingClientRect());
            return {
              nameRight: name.right, fieldsLeft: fields.left,
              centerDelta: Math.abs((name.top + name.bottom - fields.top - fields.bottom) / 2),
              inputTops: inputs.map((input) => input.top),
              inputLefts: inputs.map((input) => input.left),
              headingDeltas: inputs.map((input, index) => Math.abs(input.left - headings[index].left)),
            };
          }));
          for (const row of rows) {
            expect(row.nameRight).toBeLessThanOrEqual(row.fieldsLeft);
            expect(row.centerDelta).toBeLessThanOrEqual(1);
            if (width > 720) {
              expect(Math.max(...row.inputTops) - Math.min(...row.inputTops)).toBeLessThanOrEqual(1);
              expect(Math.max(...row.headingDeltas)).toBeLessThanOrEqual(1);
              row.inputLefts.slice(0, 3).forEach((left, index) => expect(Math.abs(left - rows[0].inputLefts[index])).toBeLessThanOrEqual(1));
            }
          }
          await dialog.locator(".member-editor-identity").scrollIntoViewIfNeeded();
          await dialog.screenshot({ path: `output/playwright/member-prices-${theme}-${width}.png` });
          const input = dialog.locator(".source-price-input input").first();
          await input.fill("12.75");
          await expect(input).toBeFocused();
          await expect(input).toHaveValue("12.75");
          await expectPriceInputsFit(dialog);
          await expect(input).toHaveCSS("outline-style", "none");
          await expect(dialog.locator(".source-price-input").first()).not.toHaveCSS("box-shadow", "none");
          await dialog.screenshot({ path: `output/playwright/member-prices-focused-${theme}-${width}.png` });
        }
        await dialog.getByRole("tab", { name: "Настройки", exact: true }).click();
        await expectDialogFits(dialog);
        await dialog.screenshot({ path: `output/playwright/member-settings-${kind}-${theme}-${width}.png` });
        await dialog.getByRole("button", { name: "Отмена", exact: true }).click();
      }
    });
  }
}
