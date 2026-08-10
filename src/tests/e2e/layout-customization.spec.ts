import { expect, test, type Locator, type Page } from "../bun-playwright";
import { installTauriMock } from "./tauri-mock";

async function columnOrder(table: Locator) {
  return table.locator("thead th[data-column]").evaluateAll((headers) => headers.map((header) => header.getAttribute("data-column")));
}

async function dragColumn(page: Page, table: Locator, column: string, target: string) {
  const sourceBox = await table.locator(`th[data-column="${column}"] .usage-column-heading`).boundingBox();
  const targetBox = await table.locator(`th[data-column="${target}"]`).boundingBox();
  if (!sourceBox || !targetBox) throw new Error("Column header is not visible");
  await page.mouse.move(sourceBox.x + sourceBox.width / 2, sourceBox.y + sourceBox.height / 2);
  await page.mouse.down();
  await page.mouse.move(targetBox.x + targetBox.width - 2, targetBox.y + targetBox.height / 2, { steps: 5 });
  await page.mouse.up();
}

async function expectCentered(table: Locator) {
  expect(await table.locator("th, td").evaluateAll((cells) => cells.every((cell) => getComputedStyle(cell).textAlign === "center"))).toBe(true);
}

test("usage report columns move with pointer input and persist independently", async ({ page }) => {
  await installTauriMock(page, { locale: "en", mode: "local", populated: true, usageFailure: true, accountCount: 4 });
  await page.goto("/");
  await page.getByRole("button", { name: "Usage", exact: true }).click();

  let table = page.locator(".usage-request-table");
  await dragColumn(page, table, "time", "status");
  await expect.poll(() => columnOrder(table)).toEqual(["status", "time", "model", "tier", "connection", "timing", "speed", "tokens", "equivalent", "request"]);
  await expectCentered(table);

  await page.getByRole("tab", { name: "Models" }).click();
  table = page.locator(".usage-aggregate-table");
  await dragColumn(page, table, "name", "requests");
  await expect.poll(() => columnOrder(table)).toEqual(["requests", "name", "input", "output", "cache", "equivalent"]);
  await expectCentered(table);

  await page.getByRole("tab", { name: "Pool members" }).click();
  table = page.locator(".usage-aggregate-table");
  await dragColumn(page, table, "name", "success");
  await expect.poll(() => columnOrder(table)).toEqual(["requests", "success", "name", "breakdown", "total", "equivalent", "speed", "timing"]);
  await expectCentered(table);

  await page.getByRole("tab", { name: "Errors" }).click();
  table = page.locator(".usage-error-table");
  await dragColumn(page, table, "time", "model");
  await expect.poll(() => columnOrder(table)).toEqual(["model", "time", "connection", "origin", "error", "request"]);
  await expectCentered(table);

  await page.reload();
  await page.getByRole("button", { name: "Usage", exact: true }).click();
  await expect.poll(() => columnOrder(page.locator(".usage-request-table"))).toEqual(["status", "time", "model", "tier", "connection", "timing", "speed", "tokens", "equivalent", "request"]);
  await page.getByRole("tab", { name: "Models" }).click();
  await expect.poll(() => columnOrder(page.locator(".usage-aggregate-table"))).toEqual(["requests", "name", "input", "output", "cache", "equivalent"]);
  await page.getByRole("tab", { name: "Pool members" }).click();
  await expect.poll(() => columnOrder(page.locator(".usage-aggregate-table"))).toEqual(["requests", "success", "name", "breakdown", "total", "equivalent", "speed", "timing"]);
  await page.getByRole("tab", { name: "Errors" }).click();
  await expect.poll(() => columnOrder(page.locator(".usage-error-table"))).toEqual(["model", "time", "connection", "origin", "error", "request"]);

  await page.getByRole("button", { name: "Overview", exact: true }).click();
  const activity = page.locator(".activity-section li").first();
  await expect(activity).toBeVisible();
  await expect(activity.locator("[data-column]")).toHaveCount(0);
  await expect(activity.locator(":scope > *")).toHaveCount(3);
});

test("account grouping is explicit and saved on connections", async ({ page }) => {
  await installTauriMock(page, { locale: "en", mode: "local", populated: true, accountCount: 4 });
  await page.setViewportSize({ width: 1160, height: 760 });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();

  const grouping = page.getByRole("button", { name: "Group by plan" });
  await expect(grouping).toHaveAttribute("aria-pressed", "false");
  await grouping.click();
  await expect(grouping).toHaveAttribute("aria-pressed", "true");
  await expect(page.locator(".account-list .account-plan-group-heading")).toHaveCount(4);
  expect(await page.locator(".account-plan-group-heading + .account-card").evaluateAll((cards) => cards.every((card) => {
    const grid = card.parentElement!.getBoundingClientRect();
    const rect = card.getBoundingClientRect();
    return rect.width >= grid.width * 0.64;
  }))).toBe(true);
  await page.setViewportSize({ width: 600, height: 760 });
  expect(await page.locator(".account-plan-group-heading + .account-card").evaluateAll((cards) => cards.every((card) => {
    const style = getComputedStyle(card);
    return style.gridColumnStart === "auto" && style.gridColumnEnd === "auto";
  }))).toBe(true);
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= document.documentElement.clientWidth)).toBe(true);
  await page.mouse.move(1, 1);
  await page.screenshot({ path: "output/playwright/account-groups-en-1160x760.png" });

  await page.reload();
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await expect(page.getByRole("button", { name: "Group by plan" })).toHaveAttribute("aria-pressed", "true");
  await page.getByRole("tab", { name: "Sources" }).click();
  const sourceTable = page.locator(".source-table");
  expect(await sourceTable.locator("th:not(:last-child), td:not(:last-child)").evaluateAll((cells) => cells.every((cell) => getComputedStyle(cell).textAlign === "center"))).toBe(true);
  await page.screenshot({ path: "output/playwright/source-table-centered-en-1160x760.png" });
  await page.getByRole("button", { name: "Pool", exact: true }).click();
  await expect(page.getByRole("button", { name: "Group by plan" })).toHaveCount(0);
  await expect(page.locator(".pool-member-list .account-plan-group-heading")).toHaveCount(0);
});
