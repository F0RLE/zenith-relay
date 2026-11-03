import { expect, test } from "@playwright/test";
import type { Locator } from "@playwright/test";

async function material(control: Locator) {
  return control.evaluate(element => {
    const style = getComputedStyle(element);
    return { font: style.fontFamily, color: style.color, background: style.backgroundColor, border: style.borderTopColor, radius: style.borderRadius, shadow: style.boxShadow, transition: style.transitionDuration };
  });
}

test("Hosted key actions align with utilities without crowding", async ({ page }) => {
  await page.goto("/app/keys");
  const create = page.getByRole("button", { name: "Создать ключ", exact: true });
  await expect(create).toBeVisible();
  const action = (await create.boundingBox())!;
  const tools = (await page.locator(".hosted-tools").boundingBox())!;
  const title = (await page.getByRole("heading", { name: "API-ключи", level: 1 }).boundingBox())!;
  expect(action.height).toBe(page.viewportSize()!.width >= 640 ? 40 : 44);
  for (const control of await page.locator(".hosted-tools > button:visible").all()) {
    const box = (await control.boundingBox())!;
    expect(box.height).toBe(action.height);
    expect(box.y).toBe(tools.y + (page.viewportSize()!.width >= 640 ? 0 : 12));
  }
  if (page.viewportSize()!.width >= 640) {
    expect(Math.abs(action.y - tools.y)).toBeLessThanOrEqual(1);
    expect(tools.x - action.x - action.width).toBeGreaterThanOrEqual(16);
  } else {
    expect(action.y - tools.y - tools.height).toBeGreaterThanOrEqual(16);
  }
  expect(action.x >= title.x + title.width + 16 || action.y >= title.y + title.height).toBe(true);
  expect(await page.locator(".relay-content").evaluate(element => element.scrollWidth <= element.clientWidth)).toBe(true);
  await create.click();
  await expect(page.getByRole("form", { name: "Создать ключ", exact: true })).toBeVisible();
  await expect(page.getByRole("dialog")).toHaveCount(0);
  await page.keyboard.press("Escape");
  await expect(create).toBeFocused();
});

test("Hosted shares the landing type and soft action material", async ({ page }, testInfo) => {
  await page.goto("/app/billing");
  const topUp = page.getByRole("button", { name: "Пополнить", exact: true });
  await expect(topUp).toBeVisible();
  const primary = await material(topUp);
  expect(primary.font).toContain("Golos Text");
  expect(primary.radius).toBe("8px");
  expect(primary.shadow).toBe("none");
  const dark = await page.locator(".hosted-app").getAttribute("data-theme") === "dark";
  expect(primary.background).toBe("rgb(240, 242, 220)");
  expect(primary.color).toBe("rgb(22, 76, 60)");
  await expect(page.locator(".balance-panel")).toHaveCSS("border-radius", "16px");
  await expect(page.locator(".balance-panel strong")).toHaveText("42,80 USD");
  await page.screenshot({ path: testInfo.outputPath("branded-billing.png") });
  await expect(page.getByRole("spinbutton", { name: "Сумма, USD", exact: true })).toHaveValue("25");
  await topUp.click();
  await expect(page.getByRole("dialog")).toHaveCount(0);
  const operation = page.getByRole("region", { name: "Платёжные операции", exact: true }).locator("tbody tr").first().getByRole("button", { name: "Пополнение", exact: true });
  await operation.click();
  const dialog = page.getByRole("dialog");
  const close = dialog.getByRole("button", { name: "Закрыть", exact: true });
  await expect(dialog).toBeVisible();
  const secondary = await material(close);
  expect(secondary.font).toBe(primary.font);
  expect(secondary.radius).toBe("8px");
  expect(secondary.shadow).toBe("none");
  expect(secondary.background).toBe("rgba(0, 0, 0, 0)");
  await expect(dialog).toHaveCSS("border-radius", "16px");
  await expect(dialog).toContainText("Банковская карта");
  await page.screenshot({ path: testInfo.outputPath("branded-payment-operation.png") });
  await page.keyboard.press("Escape");
  await expect(operation).toBeFocused();
});

test("Hosted actions support a quiet press, visible keyboard focus and reduced motion", async ({ page }) => {
  await page.goto("/app/billing");
  const topUp = page.getByRole("button", { name: "Пополнить", exact: true });
  await topUp.hover();
  await expect(topUp).toHaveCSS("transform", "none");
  await expect(topUp).toHaveCSS("box-shadow", "none");
  await page.mouse.down();
  await expect(topUp).toHaveCSS("transform", "matrix(1, 0, 0, 1, 0, 1)");
  await expect(topUp).toHaveCSS("box-shadow", "none");
  await page.mouse.up();
  await expect(page.getByRole("dialog")).toHaveCount(0);
  await expect(topUp).toBeDisabled();
  await page.getByRole("button", { name: "25 USD", exact: true }).click();
  await page.mouse.move(0, 0);
  await page.keyboard.press("Tab");
  await topUp.focus();
  await expect(topUp).toBeFocused();
  await expect(topUp).toHaveCSS("outline-style", "none");
  await expect(topUp).toHaveCSS("outline-width", "0px");
  await page.emulateMedia({ reducedMotion: "reduce" });
  await topUp.hover();
  await expect(topUp).toHaveCSS("transform", "none");
  await expect(topUp).toHaveCSS("transition-duration", "0s");
  await page.mouse.down();
  await expect(topUp).toHaveCSS("transform", "none");
  await expect(topUp).toHaveCSS("box-shadow", "none");
  await page.mouse.up();
  await expect(page.getByRole("dialog")).toHaveCount(0);
  await expect(topUp).toBeDisabled();
});

test("shared Hosted design preserves route layouts and semantic states", async ({ page }, testInfo) => {
  for (const [path, title, surface, radius] of [
    ["/app/overview", "Обзор", ".overview-chart.tokens", "16px"],
    ["/app/usage", "Использование", ".usage-overview", "16px"],
    ["/app/keys", "API-ключи", ".key-row", "0px"],
    ["/admin/connections", "Подключения", ".owner-resource-card", "12px"],
    ["/admin/pools", "Пул", ".owner-resource-card", "12px"],
  ]) {
    await page.goto(path!);
    await expect(page.getByRole("heading", { name: title!, level: 1, exact: true })).toBeVisible();
    await expect(page.locator(surface!).first()).toHaveCSS("border-radius", radius!);
    expect(await page.locator(".relay-content").evaluate(element => element.scrollWidth <= element.clientWidth)).toBe(true);
    if (path === "/app/usage") {
      await expect(page.locator('.usage-filters [data-icon="close"]')).toHaveCount(0);
      await expect(page.locator(".pagination .icon-button:disabled").first()).toHaveCSS("box-shadow", "none");
      await page.screenshot({ path: testInfo.outputPath("branded-usage.png") });
    }
    if (path === "/app/keys") {
      const dark = await page.locator(".hosted-app").getAttribute("data-theme") === "dark";
      await expect(page.locator(".danger-button").first()).toHaveCSS("color", dark ? "rgb(255, 168, 164)" : "rgb(165, 45, 42)");
    }
    if (path!.startsWith("/admin/")) {
      const header = page.locator(".relay-workspace-header:visible");
      const width = (await header.boundingBox())!.width;
      if (width > 560) {
        const tabs = (await header.locator(".relay-page-navigation").boundingBox())!;
        const actions = (await header.locator(".relay-page-actions").boundingBox())!;
        expect(Math.abs(tabs.y - actions.y)).toBeLessThanOrEqual(2);
      }
    }
  }
});

test("sidebar retains pine navigation and cream selection in both themes", async ({ page }) => {
  await page.goto("/app/overview");
  await expect(page.getByRole("heading", { name: "Обзор", exact: true })).toBeVisible();
  const sidebar = page.locator(".relay-sidebar");
  const selected = sidebar.locator('button[aria-current="page"]');
  for (let pass = 0; pass < 2; pass++) {
    const dark = await page.locator(".hosted-app").getAttribute("data-theme") === "dark";
    await expect(sidebar).toHaveCSS("background-color", dark ? "rgb(23, 63, 50)" : "rgb(22, 76, 60)");
    await expect(selected).toHaveCSS("background-color", "rgb(240, 242, 220)");
    await expect(selected).toHaveCSS("color", "rgb(22, 76, 60)");
    if (pass === 0) await page.getByRole("button", { name: dark ? "Светлая тема" : "Тёмная тема", exact: true }).click();
  }
});

test("chart and usage summaries group data without oversized or detached metrics", async ({ page }) => {
  for (const [path, selector, valueSelector] of [
    ["/app/overview", ".overview-chart.requests:visible", ".overview-chart.requests:visible .overview-chart-summary"],
    ["/app/usage", ".usage-overview:visible", ".usage-overview:visible .usage-metrics strong"],
  ]) {
    await page.goto(path!);
    const summary = page.locator(selector!);
    await expect(summary).toBeVisible();
    await expect(summary).toHaveCSS("border-radius", "16px");
    expect(await page.locator(valueSelector!).evaluateAll(values => values.every(value => {
      const bounds = value.getBoundingClientRect();
      const cell = value.closest(".usage-metric") ?? value.parentElement!;
      return value.scrollWidth <= value.clientWidth
        && bounds.left >= cell.getBoundingClientRect().left
        && bounds.right <= cell.getBoundingClientRect().right;
    }))).toBe(true);
    if (path === "/app/usage") {
      await expect(summary.locator(".usage-metrics strong").last()).toHaveText("0,041544 USD");
      await expect(summary.locator(".usage-secondary-metrics")).toHaveCSS("border-radius", "0px");
      const bounds = (await summary.boundingBox())!;
      const details = (await summary.locator(".usage-overview-details").boundingBox())!;
      expect(details.y + details.height).toBeLessThanOrEqual(bounds.y + bounds.height);
      if (bounds.width >= 1000) expect(bounds.height).toBeLessThanOrEqual(180);
    }
  }
});
