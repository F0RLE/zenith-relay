import { expect, test, type Page } from "../bun-playwright";
import { installTauriMock } from "./tauri-mock";

async function openRules(page: Page, locale: "en" | "ru" = "en") {
  await installTauriMock(page, { mode: "local", locale, populated: true, mixedModels: true });
  await page.goto("/");
  await page.getByRole("button", { name: locale === "ru" ? "Пул" : "Pool", exact: true }).click();
  await page.getByRole("tab", { name: locale === "ru" ? "Правила моделей" : "Model Rules" }).click();
}

test("model hints use one themed overlay and dismiss on Escape, click and navigation", async ({ page }) => {
  await openRules(page, "ru");
  const drag = page.locator(".model-rule-drag-handle").first();
  const hint = await drag.getAttribute("data-relay-tooltip");
  await drag.hover();
  await expect(page.getByRole("tooltip")).toHaveText(hint!);
  await expect(page.getByRole("tooltip")).toHaveCSS("opacity", "1");
  await expect(drag).toHaveAttribute("aria-describedby", await page.getByRole("tooltip").getAttribute("id") as string);
  await page.screenshot({ path: "output/playwright/tooltips-rules-ru.png" });
  await page.keyboard.press("Escape");
  await expect(page.getByRole("tooltip")).toHaveCount(0);
  await expect(drag).not.toHaveAttribute("aria-describedby");

  const toggle = page.locator(".model-toggle").first();
  await toggle.hover();
  await expect(page.getByRole("tooltip")).toHaveCount(1);
  await drag.hover();
  await expect(page.getByRole("tooltip")).toHaveText(hint!);
  await drag.click();
  await expect(page.getByRole("tooltip")).toHaveCount(0);

  await page.keyboard.press("Tab");
  await drag.focus();
  await expect(page.getByRole("tooltip")).toHaveText(hint!);
  await expect(page.getByRole("tooltip")).toHaveAttribute("data-instant", "true");
  await page.getByRole("button", { name: "Подключения", exact: true }).click();
  await expect(page.getByRole("tooltip")).toHaveCount(0);
  await expect(page.locator("[title]")).toHaveCount(0);
});

test("visible identifiers have no redundant hint but clipped identifiers retain the full value", async ({ page }) => {
  await openRules(page);
  const name = page.locator(".model-rule-identity strong").first();
  await name.hover();
  await expect(page.getByRole("tooltip")).toHaveCount(0);
  await page.mouse.move(0, 0);
  await name.evaluate((node) => { node.style.width = "20px"; node.style.display = "block"; });
  await name.hover();
  await expect(page.getByRole("tooltip")).toHaveText(await name.innerText());
  await page.evaluate(() => window.dispatchEvent(new Event("resize")));
  await expect(page.getByRole("tooltip")).toHaveCount(0);
});

test("source route disabled reasons support both hover and keyboard", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await page.getByRole("tab", { name: "Sources" }).click();
  await page.getByRole("row").filter({ hasText: "Example compatible API" }).getByRole("button", { name: "Edit" }).click();
  const dialog = page.getByRole("dialog", { name: "Edit source" });
  await dialog.getByRole("tab", { name: "Models and formats" }).click();
  const disabled = dialog.locator("label.source-route-cell[tabindex='0']").first();
  await disabled.hover();
  await expect(page.getByRole("tooltip")).toHaveText(await disabled.getAttribute("data-relay-tooltip") as string);
  await page.keyboard.press("Escape");
  await expect(dialog).toBeVisible();
  await expect(page.getByRole("tooltip")).toHaveCount(0);
  await page.keyboard.press("Tab");
  await disabled.focus();
  await expect(page.getByRole("tooltip")).toHaveAttribute("data-instant", "true");
  await expect(page.locator("[title]")).toHaveCount(0);
});

test("delegated hints preserve descriptions and reposition identical text on different anchors", async ({ page }) => {
  await openRules(page);
  // Exercise shared DOM behavior with synthetic values, without provider/account data.
  await page.evaluate(() => {
    const fixture = document.createElement("div");
    fixture.id = "tooltip-fixture";
    fixture.style.cssText = "position:fixed;bottom:10px;left:10px;right:10px;z-index:999;display:flex;justify-content:space-between";
    for (const id of ["first", "second"]) {
      const button = document.createElement("button");
      button.id = `tooltip-${id}`;
      button.textContent = id;
      button.setAttribute("data-relay-tooltip", "Synthetic explanation with the same text");
      button.setAttribute("aria-describedby", "existing-description");
      fixture.append(button);
    }
    document.body.append(fixture);
  });
  const first = page.locator("#tooltip-first");
  const second = page.locator("#tooltip-second");
  await first.hover();
  await expect(page.getByRole("tooltip")).toHaveAttribute("data-positioned", "true");
  const before = await page.getByRole("tooltip").boundingBox();
  await second.hover();
  await expect(first).toHaveAttribute("aria-describedby", "existing-description");
  await expect(second).toHaveAttribute("aria-describedby", /^existing-description .+/);
  await expect(page.getByRole("tooltip")).toHaveAttribute("data-placement", "top");
  await expect.poll(async () => (await page.getByRole("tooltip").boundingBox())!.x).toBeGreaterThan(before!.x);
  const after = await page.getByRole("tooltip").boundingBox();
  expect(after!.x + after!.width).toBeLessThanOrEqual(page.viewportSize()!.width);
  await second.evaluate((node) => node.setAttribute("data-relay-tooltip", "Updated explanation"));
  await expect(page.getByRole("tooltip")).toHaveText("Updated explanation");
  await second.evaluate((node) => node.remove());
  await expect(page.getByRole("tooltip")).toHaveCount(0);
});

test("disabled Relay buttons expose their reason on keyboard focus", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, gatewayRunning: false, theme: "dark" });
  await page.goto("/");
  await page.getByRole("button", { name: "Pool", exact: true }).click();
  const anchor = page.locator(".relay-disabled-tooltip-anchor").filter({ has: page.locator('[data-action="pool-connect"]') });
  await anchor.hover();
  const hint = page.getByRole("tooltip");
  await expect(hint).toHaveText("Start pool");
  await expect(hint).toHaveCSS("opacity", "1");
  await page.screenshot({ path: "output/playwright/tooltips-disabled-dark.png" });
  await page.keyboard.press("Escape");
  await page.keyboard.press("Tab");
  await anchor.focus();
  await expect(hint).toHaveAttribute("data-instant", "true");
  await page.keyboard.press("Escape");
  await expect(hint).toHaveCount(0);
});

test("a nested custom control wins over a delegated parent hint", async ({ page }) => {
  await openRules(page);
  const row = page.locator(".model-rules tr[data-model-id]").first();
  await row.evaluate((node) => node.setAttribute("data-relay-tooltip", "Synthetic row explanation"));
  const toggle = row.locator(".model-toggle");
  await toggle.hover();
  await expect(page.getByRole("tooltip")).toHaveText(await toggle.getAttribute("aria-label") as string);
  await row.locator('[data-column="actions"]').hover();
  await expect(page.getByRole("tooltip")).toHaveText("Synthetic row explanation");
  await toggle.hover();
  await expect(page.getByRole("tooltip")).toHaveText(await toggle.getAttribute("aria-label") as string);
  await row.evaluate((node) => node.removeAttribute("data-relay-tooltip"));
});

test("main pages and model rules contain no browser tooltip attributes", async ({ page }) => {
  await openRules(page);
  await expect(page.locator("[title]")).toHaveCount(0);
  for (const name of ["Overview", "Connections", "Pool", "API", "Usage", "Settings"]) {
    await page.getByRole("button", { name, exact: true }).click();
    await expect(page.locator("[title]")).toHaveCount(0);
  }
});

test("keyboard focus stays visible on shared navigation controls", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, theme: "dark" });
  await page.goto("/");

  const usage = page.getByRole("button", { name: "Usage", exact: true });
  await usage.focus();
  await expect(usage).toBeFocused();
  const focusStyle = await usage.evaluate((element) => {
    const style = getComputedStyle(element);
    return { borderColor: style.borderColor, boxShadow: style.boxShadow };
  });
  expect(focusStyle.borderColor).not.toBe("rgb(48, 58, 64)");
  expect(focusStyle.boxShadow).not.toBe("none");
});
