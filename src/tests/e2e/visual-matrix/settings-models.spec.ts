import { expect, test } from "../../bun-playwright";
import { installTauriMock } from "../tauri-mock";

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
      expect(groupsBox!.width).toBeGreaterThan(pageBox!.width - 80);
      const topGap = groupsBox!.y - (headerBox!.y + headerBox!.height);
      expect(topGap).toBeGreaterThanOrEqual(-1);
      expect(topGap).toBeLessThanOrEqual(4);
      await expect(page.locator(".settings-page > .relay-page-header")).toHaveClass(/relay-workspace-header/);
      await expect(page.locator(".settings-page > .relay-page-header p")).toHaveCount(0);
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
  expect(await table.getByRole("columnheader").evaluateAll((cells) => cells.map((cell) => getComputedStyle(cell).textAlign))).toEqual(["left", "right"]);
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
  await expect(model.locator('[data-model-reasoning-edit="gpt-image-2"]')).toHaveCount(0);
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

test("source prices are grouped by provider and Messages models expose cache TTLs", async ({ page }) => {
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
  await expect(dialog.locator(".source-price-group").filter({ hasText: "OpenAI" }).locator(".source-price-model code")).toHaveText(["gpt-5.4"]);
  await expect(dialog.locator(".source-price-group").filter({ hasText: "Anthropic" }).locator(".source-price-model code")).toHaveText(["claude-opus-4-8"]);
  await expect(dialog.getByText("Claude Opus", { exact: true })).toHaveCount(0);
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
