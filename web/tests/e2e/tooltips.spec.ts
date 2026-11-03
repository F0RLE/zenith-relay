import { expect, test } from "@playwright/test";

test.beforeEach(async ({ page }) => {
  await page.goto("/preview.html?page=usage");
  await expect(page.locator("html")).toHaveAttribute("lang", "ru");
});

test("Hosted hints wait on hover and use Relay styling instead of browser titles", async ({ page }, testInfo) => {
  const metric = page.locator(".usage-metric").first();
  const text = await metric.getAttribute("data-relay-tooltip");
  await expect(metric).not.toHaveAttribute("title");
  await metric.hover();
  await expect(page.getByRole("tooltip")).toHaveCount(0);
  const hint = page.getByRole("tooltip");
  await expect(hint).toHaveText(text!);
  await expect(hint).toHaveCSS("opacity", "1");
  await expect(hint).toHaveClass(/relay-tooltip/);
  await expect(hint).toHaveCSS("pointer-events", "none");
  await expect(metric).toHaveAttribute("aria-describedby", (await hint.getAttribute("id"))!);
  await page.screenshot({ path: testInfo.outputPath("hosted-tooltip.png") });
  await page.mouse.move(0, 0);
  await expect(hint).toHaveCount(0);
  await expect(metric).not.toHaveAttribute("aria-describedby");
});

test("brief hover, scrolling and clicks do not leave stale hints", async ({ page }) => {
  await page.setViewportSize({ width: page.viewportSize()!.width, height: 600 });
  const metric = page.locator(".usage-metric").first();
  await metric.hover();
  await page.mouse.move(0, 0);
  // Longer than the hover delay: leaving a trigger must cancel its timer.
  await page.waitForTimeout(700);
  await expect(page.getByRole("tooltip")).toHaveCount(0);
  await metric.hover();
  await expect(page.getByRole("tooltip")).toHaveCSS("opacity", "1");
  await page.mouse.wheel(0, 200);
  await expect(page.getByRole("tooltip")).toHaveCount(0);
  const theme = page.getByRole("button", { name: /^(Тёмная|Светлая) тема$/ });
  // Finish returning to the header before hovering: an automatic scroll after
  // mouseover correctly cancels the tooltip's pending timer.
  await theme.scrollIntoViewIfNeeded();
  await theme.hover();
  await expect(page.getByRole("tooltip")).toHaveCSS("opacity", "1");
  await theme.click();
  await expect(page.getByRole("tooltip")).toHaveCount(0);
});

test("keyboard focus exposes the hint immediately and Escape dismisses it", async ({ page }) => {
  const metric = page.locator(".usage-metric").first();
  await metric.focus();
  await expect(page.getByRole("tooltip")).toHaveText((await metric.getAttribute("data-relay-tooltip"))!);
  await expect(page.getByRole("tooltip")).toHaveCSS("opacity", "1");
  await page.keyboard.press("Escape");
  await expect(page.getByRole("tooltip")).toHaveCount(0);
  await expect(metric).toBeFocused();
});

test("request hints stay above the native dialog without intercepting Escape", async ({ page }, testInfo) => {
  await page.getByRole("button", { name: "Детали запроса demo_request_0003", exact: true }).click();
  const dialog = page.getByRole("dialog");
  const download = dialog.getByRole("button", { name: "Скачать диагностику запроса", exact: true });
  await download.hover();
  const hint = page.getByRole("tooltip");
  await expect(hint).toHaveText("Скачать сведения о запросе");
  await expect(hint).toHaveCSS("opacity", "1");
  expect(await hint.evaluate(element => element.matches(":popover-open"))).toBe(true);
  const bounds = await hint.boundingBox();
  expect(bounds!.x).toBeGreaterThanOrEqual(8);
  expect(bounds!.x + bounds!.width).toBeLessThanOrEqual(testInfo.project.use.viewport!.width - 8);
  await expect(dialog.locator("[title]")).toHaveCount(0);
  await page.screenshot({ path: testInfo.outputPath("request-tooltip.png") });
  await page.keyboard.press("Escape");
  await expect(dialog).toHaveCount(0);
  await expect(hint).toHaveCount(0);
});

test("collapsed navigation keeps its custom label hints", async ({ page }, testInfo) => {
  test.skip(testInfo.project.use.viewport!.width < 640, "Mobile navigation has visible labels.");
  await expect(page.locator(".relay-sidebar")).toBeVisible();
  const collapse = page.getByRole("button", { name: "Свернуть меню", exact: true });
  if (await collapse.isVisible()) await collapse.click();
  await expect(page.locator(".relay-sidebar")).toHaveCSS("width", "56px");
  await page.mouse.move(0, 0);
  await page.locator(".relay-sidebar").getByRole("button", { name: "Использование", exact: true }).hover();
  await expect(page.getByRole("tooltip")).toHaveText("Использование");
  await expect(page.getByRole("tooltip")).toHaveCSS("opacity", "1");
  await expect(page.getByRole("tooltip")).toHaveAttribute("data-placement", "right");
});
