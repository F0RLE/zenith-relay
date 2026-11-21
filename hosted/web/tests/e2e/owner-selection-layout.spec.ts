import { expect, test } from "@playwright/test";

for (const path of ["connections", "pools"]) {
  test(`${path}: selecting cards swaps toolbar controls without moving the summary or grid`, async ({ page }, testInfo) => {
    await page.goto(`/admin/${path}`);
    const controls = page.locator(".owner-resource-controls:visible");
    const grid = page.locator(".owner-resource-grid:visible");
    const first = grid.getByRole("checkbox").first();
    const selectAll = controls.getByRole("checkbox", { name: "Выбрать все показанные подключения", exact: true });
    const bulk = controls.getByRole("group", { name: "Действия с выбранными подключениями" });
    await expect(first).toBeVisible();
    const layout = async () => ({
      controls: await controls.evaluate(el => {
        const toolbar = el.querySelector<HTMLElement>(".owner-resource-toolbar")!;
        return { height: el.clientHeight, toolbarTop: toolbar.offsetTop, toolbarHeight: toolbar.clientHeight };
      }),
      grid: await grid.evaluate(el => ({ top: (el as HTMLElement).offsetTop, width: el.clientWidth })),
    });
    const before = await layout();
    await first.check();
    await expect(controls.getByRole("status")).toHaveText("Выбрано: 1");
    await expect(selectAll).toHaveJSProperty("indeterminate", true);
    await expect(controls.locator('.owner-resource-select')).toHaveAttribute("data-state", "mixed");
    await expect(grid.locator('.owner-resource-select[data-state="checked"] .owner-selection-mark')).toHaveCount(1);
    expect(await layout()).toEqual(before);
    await expect(controls.locator(path === "pools" ? ".owner-pool-actions" : ".owner-resource-filters")).toHaveAttribute("inert", "");
    await expect(controls.locator(".owner-resource-summary")).toBeVisible();
    expect(await bulk.evaluate(el => {
      const bounds = el.getBoundingClientRect();
      return [...el.querySelectorAll("button")].every(button => {
        const rect = button.getBoundingClientRect();
        return rect.left >= bounds.left && rect.right <= bounds.right && rect.top >= bounds.top && rect.bottom <= bounds.bottom;
      });
    })).toBe(true);
    await selectAll.check();
    await expect(selectAll).toHaveJSProperty("indeterminate", false);
    await expect(controls.locator('.owner-resource-select')).toHaveAttribute("data-state", "checked");
    await expect(async () => {
      const selectedMarks = await page.locator('.owner-resource-select[data-state="checked"]:visible .owner-selection-mark').evaluateAll(marks => marks.map(mark => {
        const style = getComputedStyle(mark);
        return { width: style.width, height: style.height, background: style.backgroundColor, color: style.color, radius: style.borderRadius };
      }));
      expect(selectedMarks.length).toBeGreaterThan(1);
      for (const mark of selectedMarks) {
        expect(mark).toEqual(selectedMarks[0]);
        expect(mark.width).toBe("32px");
        expect(mark.height).toBe("32px");
      }
    }).toPass({ timeout: 3_000 });
    expect(await layout()).toEqual(before);
    await controls.scrollIntoViewIfNeeded();
    await page.screenshot({ path: testInfo.outputPath(`${path}-selection-stable.png`) });
    await selectAll.uncheck();
    await expect(bulk).toHaveCount(0);
    if (path === "pools") {
      await expect(controls.getByRole("radiogroup", { name: "Скорость запросов" })).toBeVisible();
      await expect(controls.getByRole("button", { name: "Настройки ротации пула" })).toBeVisible();
    } else {
      await expect(controls.getByRole("searchbox")).toBeVisible();
      await expect(controls.getByRole("combobox")).toBeVisible();
    }
    await expect(controls.locator(".owner-resource-summary")).toBeVisible();
    expect(await layout()).toEqual(before);
    expect(await page.locator(".relay-content").evaluate(el => el.scrollWidth <= el.clientWidth)).toBe(true);
  });
}

test("clearing selection restores filters and bulk actions return focus without movement", async ({ page }) => {
  await page.goto("/admin/connections");
  const controls = page.locator(".owner-resource-controls:visible");
  const grid = page.locator(".owner-resource-grid:visible");
  const search = controls.getByRole("searchbox");
  await expect(search).toBeVisible();
  const height = await controls.evaluate(el => el.clientHeight);
  await grid.getByRole("checkbox").first().check();
  await grid.getByRole("checkbox").first().uncheck();
  await search.fill("Text");
  await expect(controls.locator(".owner-resource-bulk")).toHaveCount(0);
  await expect(grid.getByRole("article")).toHaveCount(1);
  await grid.getByRole("checkbox").check();
  await controls.getByRole("button", { name: "Убрать из пула", exact: true }).click();
  await expect(search).toBeFocused();
  await expect(grid.getByRole("article")).toHaveAttribute("data-status", "idle");
  expect(await controls.evaluate(el => el.clientHeight)).toBe(height);
});

test("selection uses the Desktop fill regardless of membership and preserves status identity", async ({ page }) => {
  await page.goto("/admin/connections");
  const cards = page.locator(".owner-resource-grid:visible article");
  const outside = cards.nth(1);
  await outside.getByRole("button", { name: "Убрать из пула", exact: true }).click();
  await expect(outside).toHaveAttribute("data-status", "idle");
  const identity = await cards.locator(".owner-resource-icon").evaluateAll(icons => icons.map(icon => ({ background: getComputedStyle(icon).backgroundColor, color: getComputedStyle(icon).color })));
  await page.locator(".owner-resource-controls:visible").getByRole("checkbox").check();
  const selected = await cards.evaluateAll(items => items.map(card => ({ background: getComputedStyle(card).backgroundColor, border: getComputedStyle(card).borderColor })));
  expect(selected[0]).toEqual(selected[1]);
  expect(selected[0]!.border).toBe("rgba(0, 0, 0, 0)");
  expect(await cards.locator(".owner-resource-icon").evaluateAll(icons => icons.map(icon => ({ background: getComputedStyle(icon).backgroundColor, color: getComputedStyle(icon).color })))).toEqual(identity);
});

test("shared selection retains keyboard, partial, disabled and high-contrast states", async ({ page }) => {
  await page.goto("/admin/connections");
  const controls = page.locator(".owner-resource-controls:visible");
  const first = page.locator(".owner-resource-grid:visible article").first();
  const checkbox = first.getByRole("checkbox");
  const mark = first.locator(".owner-selection-mark");
  await checkbox.focus();
  await page.keyboard.press("Space");
  await expect(checkbox).toBeChecked();
  await expect(checkbox).toBeFocused();
  await expect(mark).toHaveCSS("outline-style", "none");
  await expect(mark).toHaveCSS("outline-width", "0px");
  const target = (await checkbox.boundingBox())!;
  expect(target.width).toBeGreaterThanOrEqual(page.viewportSize()!.width <= 639 ? 44 : 40);
  expect(target.height).toBeGreaterThanOrEqual(page.viewportSize()!.width <= 639 ? 44 : 40);
  await page.emulateMedia({ forcedColors: "active", reducedMotion: "reduce" });
  await expect(mark).toHaveCSS("transition-duration", "0s");
  expect(await mark.evaluate(el => getComputedStyle(el).backgroundColor !== getComputedStyle(el).color)).toBe(true);
  await page.keyboard.press("Space");
  await expect(checkbox).not.toBeChecked();
  await controls.getByRole("searchbox").fill("НетТакогоПодключения");
  await expect(controls.getByRole("checkbox")).toBeDisabled();
  await expect(controls.locator(".owner-resource-select")).toHaveAttribute("data-state", "unchecked");
});
