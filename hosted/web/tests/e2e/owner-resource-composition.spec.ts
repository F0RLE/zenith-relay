import { expect, test } from "@playwright/test";

test("Pool matches Desktop summary, centered metrics and three equal footer actions", async ({ page }, testInfo) => {
  await page.goto("/admin/pools");
  const controls = page.locator(".owner-resource-controls:visible");
  const card = page.getByRole("article", { name: "Code account", exact: true });
  await expect(card).toBeVisible();
  await expect(controls.locator(".owner-resource-summary dd")).toHaveText(["3", "3", "0", "1"]);
  await expect(controls.locator(".owner-resource-economics dd")).toHaveText(["100 $", "≈11,6 $", "≈200 %"]);
  await expect(card.locator("header .owner-card-menu-trigger")).toHaveCount(0);
  await expect(card.locator("footer").getByRole("button")).toHaveCount(3);
  const widths = await card.locator("footer > *").evaluateAll(items => items.map(item => item.getBoundingClientRect().width));
  expect(Math.max(...widths) - Math.min(...widths)).toBeLessThan(1);
  const centers = await controls.locator(".owner-resource-summary > div, .owner-resource-economics > div").evaluateAll(cells => cells.map(cell => {
    const box = cell.getBoundingClientRect(), label = cell.querySelector("dt")!.getBoundingClientRect(), value = cell.querySelector("dd")!.getBoundingClientRect();
    return Math.abs((label.left + value.right) / 2 - (box.left + box.right) / 2);
  }));
  expect(Math.max(...centers)).toBeLessThan(1);
  const toggle = controls.getByRole("button", { name: "Показать расчёты аккаунтов", exact: true });
  await toggle.click();
  await expect(controls.locator(".owner-resource-economics")).toHaveCount(0);
  await expect(card.locator(".owner-account-values")).toHaveCount(0);
  await expect(card.getByRole("meter")).toHaveCount(2);
  await toggle.click();
  await expect(card.locator(".owner-account-values")).toBeVisible();
  const edit = card.getByRole("button", { name: "Редактировать Code account", exact: true });
  await expect(edit).toHaveAttribute("aria-haspopup", "dialog");
  await edit.click();
  await expect(page.getByRole("dialog", { name: "Настройки участника", exact: true })).toBeVisible();
  await expect(page.getByRole("tab", { name: "Модели", exact: true })).toBeFocused();
  await page.keyboard.press("Escape");
  await expect(edit).toBeFocused();
  await page.screenshot({ path: testInfo.outputPath("pool-desktop-composition.png"), fullPage: true });
});

test("totals describe saved membership, not the search result or API-provider usage", async ({ page }) => {
  await page.goto("/admin/pools");
  const controls = page.locator(".owner-resource-controls:visible");
  const total = controls.locator(".owner-resource-economics dd").first();
  await expect(total).toHaveText("100 $");
  await controls.getByRole("button", { name: "Поиск сохранённых подключений", exact: true }).click();
  await controls.getByRole("searchbox").fill("Text");
  await expect(page.locator(".owner-resource-grid:visible article")).toHaveCount(1);
  await expect(total).toHaveText("100 $");
  await page.getByRole("article", { name: "Text account", exact: true }).getByRole("button", { name: "Убрать из пула", exact: true }).click();
  await expect(total).toHaveText("10 $");
  await expect(controls.locator(".owner-resource-summary dd").first()).toHaveText("2");
});
