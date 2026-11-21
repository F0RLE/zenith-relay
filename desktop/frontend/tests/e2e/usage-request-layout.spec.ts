import { expect, test, type Locator } from "../bun-playwright";
import { installTauriMock } from "./tauri-mock";
import { setUsageEventOverrides } from "./request-details-fixture";

async function expectAutomaticColumns(table: Locator) {
  await expect(table).toBeVisible();
  await expect(table.locator("thead th").first()).toBeVisible();
  await expect(table).toHaveCSS("table-layout", "auto");
  await expect(table.getByRole("separator")).toHaveCount(0);
  await expect(table.locator("tbody tr").first()).toBeVisible();
  const columns = await table.locator("thead th").evaluateAll((elements) => elements
    .map((element) => ({ id: element.getAttribute("data-column"), bounds: element.getBoundingClientRect() }))
    .filter(({ bounds }) => bounds.width > 0)
    .map(({ id, bounds }) => ({ id, width: bounds.width, center: bounds.left + bounds.width / 2 })));
  expect(columns.length).toBeGreaterThan(1);
  const width = (id: string) => columns.find((column) => column.id === id)!.width;
  expect(width("status")).toBeLessThanOrEqual(72);
  expect(width("model")).toBeGreaterThan(width("status"));
  expect(await table.evaluate((element) => element.parentElement!.scrollWidth <= element.parentElement!.clientWidth + 1)).toBe(true);
  expect(await table.evaluate((element) => Math.abs(element.getBoundingClientRect().width - element.parentElement!.clientWidth))).toBeLessThanOrEqual(1);
  expect(await table.locator("th .usage-column-heading > span").evaluateAll((elements) => elements
    .filter((element) => element.scrollWidth > element.clientWidth + 1)
    .map((element) => element.textContent))).toEqual([]);
  const cells = await table.locator("tbody tr").first().locator("td").evaluateAll((elements) => elements
    .filter((element) => element.getBoundingClientRect().width > 0)
    .map((element) => {
      const bounds = element.getBoundingClientRect();
      return { id: element.getAttribute("data-column"), width: bounds.width, center: bounds.left + bounds.width / 2 };
    }));
  expect(cells).toEqual(columns);
}

async function expectEvenColumnExpansion(table: Locator) {
  const expansion = await table.evaluate((element) => {
    const headings = Array.from(element.querySelectorAll("thead th"));
    const widths = () => headings.map((heading) => heading.getBoundingClientRect().width);
    const stretched = widths();
    const previous = element.style.justifyContent;
    element.style.justifyContent = "start";
    const natural = widths();
    element.style.justifyContent = previous;
    return headings.map((heading, index) => ({
      id: heading.getAttribute("data-column"), added: stretched[index] - natural[index],
    }));
  });
  expect(expansion.find((column) => column.id === "status")!.added).toBeCloseTo(0, 0);
  const added = expansion.filter((column) => column.id !== "status").map((column) => column.added);
  expect(Math.min(...added)).toBeGreaterThan(0);
  expect(Math.max(...added) - Math.min(...added)).toBeLessThanOrEqual(1);
}

for (const scenario of [
  { width: 2451, locale: "en", theme: "light" },
  { width: 2301, locale: "ru", theme: "dark" },
  { width: 1660, locale: "ru", theme: "dark" },
  { width: 1160, locale: "ru", theme: "dark" },
  { width: 1160, locale: "ru", theme: "dark", fallbackFont: true },
  { width: 1060, locale: "ru", theme: "dark" },
  { width: 980, locale: "ru", theme: "dark" },
  { width: 840, locale: "en", theme: "light" },
  { width: 840, locale: "ru", theme: "dark", fallbackFont: true },
  { width: 700, locale: "ru", theme: "dark" },
  { width: 700, locale: "ru", theme: "dark", fallbackFont: true },
  { width: 390, locale: "ru", theme: "dark" },
] as const) {
  const fallback = "fallbackFont" in scenario;
  test(`request table balances separate model and reasoning columns: ${scenario.width} ${scenario.locale}${fallback ? " fallback font" : ""}`, async ({ page }) => {
    const accountLabel = "long.synthetic.member.0123456789@example.test";
    await page.setViewportSize({ width: scenario.width, height: 900 });
    await installTauriMock(page, {
      ...scenario, mode: "local", populated: true, accountLabel,
      usageRequestedModel: "synthetic-model-with-a-long-name-2026",
      usageResolvedModel: "synthetic-model-with-a-long-name-2026",
      usageRepeatedRequests: true,
    });
    await page.goto("/");
    if (fallback) {
      await page.addStyleTag({ content: ".app { font-family: Verdana, sans-serif !important; }" });
    }
    await page.getByRole("button", { name: scenario.locale === "ru" ? "Использование" : "Usage", exact: true }).click();
    const table = page.locator(".usage-request-table");
    const row = table.locator("tbody tr").first();
    await expect(row).toBeVisible();
    const tableLayout = await table.locator("thead").isVisible();
    const speed = row.locator('[data-column="speed"]');
    await expect(speed).toBeVisible();
    await expect(speed).toHaveText(scenario.locale === "ru" ? "6,7 ток/с" : "6.7 tok/s");
    await expect(table.locator('thead th[data-column="timing"] + th')).toHaveAttribute("data-column", "speed");
    await expect(table.locator('thead th[data-column="speed"] + th')).toHaveAttribute("data-column", "tokens");
    const connection = row.locator(".usage-request-connection");
    await expect(connection).toHaveText(accountLabel);
    await expect(row.locator('[data-column="connection"]')).toHaveAttribute("data-relay-tooltip", accountLabel);
    await expect(table.locator('thead th[data-column="model"] + th')).toHaveAttribute("data-column", "reasoning");
    await expect(table.locator('thead th[data-column="reasoning"]')).toHaveText(scenario.locale === "ru" ? "Размышление" : "Reasoning");
    await expect(row.locator('[data-column="model"]')).toHaveText("synthetic-model-with-a-long-name-2026");
    await expect(row.locator('[data-column="model"] small')).toHaveCount(0);
    await expect(row.locator(".usage-request-reasoning")).toHaveAttribute("data-relay-tooltip", /.+/);
    await expect(row.locator('[data-column="reasoning"]')).not.toHaveText("");
    if (tableLayout) {
      await expectAutomaticColumns(table);
      expect(await table.locator("th, td").evaluateAll((elements) => elements
        .filter((element) => element.getBoundingClientRect().width > 0)
        .every((element) => getComputedStyle(element).textAlign === "center"))).toBe(true);
      const model = (await row.locator('[data-column="model"]').boundingBox())!;
      const reasoning = (await row.locator('[data-column="reasoning"]').boundingBox())!;
      expect(reasoning.x).toBeCloseTo(model.x + model.width, 0);
      expect(reasoning.y).toBeCloseTo(model.y, 0);
      expect(reasoning.height).toBeCloseTo(model.height, 0);
      await expect(connection).toHaveCSS("white-space", "normal");
      expect(await connection.evaluate((element) => element.scrollWidth <= element.clientWidth + 1)).toBe(true);
    } else {
      expect(await row.locator('[data-column="model"]').evaluate((element) => getComputedStyle(element).textAlign)).toBe("left");
      await expect(row.locator('[data-column="reasoning"]')).toHaveAttribute("data-label", "Размышление");
      await expect(row.locator(".usage-request-model")).toHaveCSS("white-space", "normal");
    }
    if (scenario.width >= 1160) {
      for (const column of ["speed", "tokens", "request"]) {
        const heading = table.locator(`th[data-column="${column}"] .usage-column-heading > span`);
        const bounds = await heading.evaluate((element) => ({
          width: element.getBoundingClientRect().width,
          height: element.getBoundingClientRect().height,
          cellWidth: element.closest("th")!.getBoundingClientRect().width,
        }));
        expect(bounds.height, `${column}: ${JSON.stringify(bounds)}`).toBeLessThanOrEqual(scenario.width >= 1660 ? 17 : 33);
      }
    }
    expect(await row.locator(".request-link").evaluate((element) => element.scrollWidth <= element.clientWidth + 1)).toBe(true);
    expect(await table.locator("td").evaluateAll((elements) => elements
      .filter((element) => element.getBoundingClientRect().width > 0 && element.scrollWidth > element.clientWidth + 1)
      .map((element) => ({ column: element.getAttribute("data-column"), width: element.clientWidth, content: element.scrollWidth })))).toEqual([]);
    await page.mouse.move(0, 0);
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= document.documentElement.clientWidth + 1)).toBe(true);
    if (tableLayout) {
      const wrapper = table.locator("..");
      expect(await wrapper.evaluate((element) => element.scrollWidth <= element.clientWidth + 1)).toBe(true);
      await expect(row.locator(".request-link")).toBeInViewport();
    }
    await (tableLayout ? table : row).screenshot({ path: `output/playwright/usage-requests-columns-${scenario.locale}-${scenario.width}${fallback ? "-fallback" : ""}.png` });
    await row.getByRole("button", { name: /req|relay-synthetic/ }).click();
    await expect(page.getByRole("dialog")).toBeVisible();
  });
}

for (const width of [1060, 1160, 1660, 2301, 3840]) {
  test(`request columns automatically follow the available width: ${width}`, async ({ page }) => {
    await page.setViewportSize({ width, height: 900 });
    await installTauriMock(page, { locale: "en", mode: "local", populated: true });
    await page.goto("/");
    await page.getByRole("button", { name: "Usage", exact: true }).click();
    const table = page.locator(".usage-request-table");
    await expectAutomaticColumns(table);
    const initialWidth = (await table.boundingBox())!.width;
    await page.setViewportSize({ width: 840, height: 900 });
    await expectAutomaticColumns(table);
    expect((await table.boundingBox())!.width).toBeLessThan(initialWidth);
    await page.reload();
    await page.getByRole("button", { name: "Usage", exact: true }).click();
    await page.setViewportSize({ width, height: 900 });
    await expectAutomaticColumns(table);
    expect((await table.boundingBox())!.width).toBeCloseTo(initialWidth, 0);
  });

  test(`reasoning is readable and independently movable: ${width}`, async ({ page }) => {
    await page.setViewportSize({ width, height: 900 });
    await installTauriMock(page, { locale: "ru", theme: "dark", mode: "local", populated: true, accountLabel: "synthetic.member+372@example.test", usageRepeatedRequests: true });
    await page.goto("/");
    await setUsageEventOverrides(page, { requestedModel: "gpt-6.2-sol", resolvedModel: "gpt-6.2-sol", requestedReasoningEffort: "xhigh", effectiveReasoningEffort: "max", totalTokens: 124500, outputTokens: 33, latencyMs: 18300, ttftMs: 17800, generationMs: 500, apiEquivalent: { microUsd: 26800, pricedTokens: 124500, unpricedTokens: 0 } });
    await page.getByRole("button", { name: "Использование", exact: true }).click();
    const table = page.locator(".usage-request-table");
    const row = table.locator("tbody tr").first();
    await expect(row.locator('[data-column="model"]')).toHaveText("gpt-6.2-sol");
    expect((await row.locator(".usage-request-model").boundingBox())!.height).toBeLessThanOrEqual(18);
    await expect(row.locator(".usage-request-reasoning")).toHaveText("Максимальное");
    await expect(row.locator(".usage-request-reasoning")).toHaveAttribute("data-relay-tooltip", "Запрошено: Очень высокое, передано: Максимальное");
    await expect(row.locator('[data-column="tokens"]')).toHaveText("124,5 тыс.");
    for (const column of ["timing", "speed", "tokens", "equivalent"]) {
      const cell = row.locator(`[data-column="${column}"]`);
      expect(await cell.evaluate((element) => element.scrollWidth <= element.clientWidth + 1)).toBe(true);
    }
    for (const selector of [".usage-request-model", ".usage-request-reasoning"]) {
      expect(await row.locator(selector).evaluate((element) => element.scrollWidth <= element.clientWidth + 1)).toBe(true);
    }
    await page.mouse.move(0, 0);
    await expectAutomaticColumns(table);
    if (width >= 1660) await expectEvenColumnExpansion(table);
    await table.screenshot({ path: `output/playwright/usage-requests-columns-preview-${width}.png` });
    await table.locator("..").screenshot({ path: `output/playwright/usage-requests-viewport-${width}.png` });
    await table.locator('th[data-column="reasoning"] .usage-column-heading').focus();
    await page.keyboard.press("Alt+ArrowRight");
    await expect(table.locator('th[data-column="connection"] + th')).toHaveAttribute("data-column", "reasoning");
    await page.reload();
    await page.getByRole("button", { name: "Использование", exact: true }).click();
    await expect(table.locator('th[data-column="connection"] + th')).toHaveAttribute("data-column", "reasoning");
  });
}

test("saved narrow columns stay readable without stretching status", async ({ page }) => {
  await page.setViewportSize({ width: 1060, height: 900 });
  await installTauriMock(page, { locale: "ru", theme: "dark", mode: "local", populated: true });
  await page.addInitScript(() => {
    const order = ["time", "status", "model", "reasoning", "connection", "timing", "speed", "tokens", "equivalent", "request"];
    if (!localStorage.getItem("relay.usageRequestTableLayout")) localStorage.setItem("relay.usageRequestTableLayout", JSON.stringify({ order, widths: { time: 130, status: 240, model: 82, reasoning: 64, connection: 80, timing: 72, speed: 48, tokens: 40, equivalent: 48, request: 56 } }));
  });
  await page.goto("/");
  await setUsageEventOverrides(page, { requestedReasoningEffort: "max", effectiveReasoningEffort: "max", totalTokens: 124500, latencyMs: 66800, ttftMs: 27600 });
  await page.getByRole("button", { name: "Использование", exact: true }).click();
  const table = page.locator(".usage-request-table");
  await expectAutomaticColumns(table);
  await expect(table.locator('th[data-column="status"] .usage-column-resizer')).toHaveCount(0);
  expect(await table.locator(".usage-request-reasoning").first().evaluate((element) => element.scrollWidth <= element.clientWidth + 1)).toBe(true);
  const saved = await page.evaluate(() => localStorage.getItem("relay.usageRequestTableLayout"));
  await page.reload();
  await page.getByRole("button", { name: "Использование", exact: true }).click();
  expect(await page.evaluate(() => localStorage.getItem("relay.usageRequestTableLayout"))).toBe(saved);
  await expectAutomaticColumns(table);
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= document.documentElement.clientWidth + 1)).toBe(true);
});

for (const scenario of [
  { name: "failed request", generationMs: 300, failure: true },
  { name: "missing generation time", generationMs: null, failure: false },
  { name: "buffered response", generationMs: 1, failure: false },
] as const) {
  test(`request speed stays unknown for ${scenario.name}`, async ({ page }) => {
    await installTauriMock(page, {
      locale: "en", mode: "local", populated: true, usageFailure: scenario.failure,
      usageTiming: { latencyMs: 428, ttftMs: 128, generationMs: scenario.generationMs },
    });
    await page.goto("/");
    await page.getByRole("button", { name: "Usage", exact: true }).click();
    await expect(page.locator('.usage-request-table tbody [data-column="speed"]')).toHaveText("-");
  });
}
