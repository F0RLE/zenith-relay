import { expect, test } from "@playwright/test";

test("inline key editor keeps keyboard navigation without focus rings or trapped Tab", async ({ page }, testInfo) => {
  await page.goto("/app/keys");
  const edit = page.getByRole("button", { name: "Изменить", exact: true }).first();
  await edit.click();
  const editor = page.getByRole("form", { name: "Настройки ключа", exact: true });
  const name = editor.getByRole("textbox", { name: "Название", exact: true });
  await expect(name).toBeFocused();
  await expect(name).toHaveCSS("outline-style", "none");
  await expect(name).toHaveCSS("outline-width", "0px");
  await expect(name).toHaveCSS("box-shadow", "none");
  await name.fill("Учебное название");
  await page.keyboard.press("Tab");
  await expect(editor.getByRole("spinbutton", { name: "Дневной лимит, USD", exact: true })).toBeFocused();
  await expect(editor.getByRole("spinbutton", { name: "Дневной лимит, USD", exact: true })).toHaveCSS("outline-style", "none");
  await page.keyboard.press("Shift+Tab");
  await expect(name).toBeFocused();
  await editor.getByLabel("Действует до", { exact: true }).focus();
  await expect(editor.getByLabel("Действует до", { exact: true })).toHaveCSS("outline-style", "none");
  await editor.getByRole("button", { name: "Сохранить", exact: true }).focus();
  await page.keyboard.press("Tab");
  await expect(page.getByRole("row", { name: "My project", exact: true }).getByRole("button", { name: "Изменить", exact: true })).toBeFocused();
  await name.focus();
  await page.screenshot({ path: testInfo.outputPath("key-input-keyboard-focus.png") });
  await page.keyboard.press("Escape");
  await expect(editor).toHaveCount(0);
  await expect(page.getByRole("dialog")).toHaveCount(0);
  await expect(edit).toBeFocused();
  await expect(edit).toHaveCSS("outline-style", "none");
  await expect(edit).not.toHaveCSS("box-shadow", /inset/);
});

test("usage metrics and chart keyboard tooltips work without focus rings", async ({ page }) => {
  await page.goto("/app/usage");
  const metric = page.locator(".usage-metric").first();
  await expect(metric).toBeVisible();
  await page.keyboard.press("Tab");
  await metric.focus();
  await expect(metric).toBeFocused();
  await expect(metric).toHaveCSS("outline-style", "none");
  await page.goto("/app/overview");
  const point = page.locator(".tokens .hosted-chart-canvas");
  await expect(point).toBeVisible();
  await page.keyboard.press("Tab");
  await point.focus();
  await expect(point).toBeFocused();
  await expect(point).toHaveCSS("outline-style", "none");
  await expect(point.locator('[data-active="true"]')).toHaveCount(1);
  await expect(page.locator(".tokens").getByRole("tooltip")).toBeVisible();
});

test("Hosted search has no outer focus ring in either workspace or theme", async ({ page }, testInfo) => {
  test.skip(testInfo.project.name !== "desktop-light", "Explicit workspace, theme and viewport coverage runs once.");
  for (const [path, label] of [["/app/status", "Поиск моделей"], ["/admin/connections", "Поиск подключений"]] as const) {
    for (const theme of ["light", "dark"]) {
      await page.goto(path);
      await page.evaluate(value => localStorage.setItem("relay-hosted-theme", value), theme);
      await page.reload();
      const search = page.getByRole("searchbox", { name: label, exact: true });
      await expect(search).toBeVisible();
      const before = await search.boundingBox();
      await search.click();
      await expect(search).toBeFocused();
      await expect(search).toHaveCSS("outline-style", "none");
      await expect(search).toHaveCSS("box-shadow", "none");
      await page.keyboard.press("Tab");
      await page.keyboard.press("Shift+Tab");
      await expect(search).toBeFocused();
      await expect(search).toHaveCSS("outline-style", "none");
      expect(await search.boundingBox()).toEqual(before);
      await page.screenshot({ path: testInfo.outputPath(`status-no-focus-${path.includes("admin") ? "owner" : "customer"}-${theme}.png`) });
    }
  }
});

test("system high contrast retains accessible keyboard focus", async ({ page }) => {
  await page.emulateMedia({ forcedColors: "active" });
  await page.goto("/app/keys");
  await page.getByRole("button", { name: "Изменить", exact: true }).first().click();
  const name = page.getByRole("textbox", { name: "Название", exact: true });
  await expect(name).toBeFocused();
  await expect(name).toHaveCSS("outline-style", "solid");
  await expect(name).toHaveCSS("outline-width", "1px");
});
