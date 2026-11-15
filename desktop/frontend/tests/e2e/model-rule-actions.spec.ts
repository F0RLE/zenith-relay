import { expect, test } from "../bun-playwright";
import { installTauriMock } from "./tauri-mock";

for (const width of [1160, 840, 720, 600, 390, 360]) {
  for (const theme of ["light", "dark"] as const) {
    test(`model actions form one aligned group in ${theme} at ${width}px`, async ({ page }, testInfo) => {
      await page.setViewportSize({ width, height: 844 });
      await installTauriMock(page, { locale: "ru", mode: "local", theme, populated: true, mixedModels: true, modelSpeed: { "gpt-5.4": "ultrafast" }, modelReasoning: { "gpt-5.4": ["low", "medium", "high"] }, modelContext: { "gpt-5.4": { configurable: true, contextLimit: 1_050_000 } } });
      await page.goto("/");
      await page.getByRole("button", { name: "Пул", exact: true }).click();
      await page.getByRole("tab", { name: "Правила моделей", exact: true }).click();
      const row = page.locator('[data-model-id="gpt-5.4"]');
      const actions = row.locator('[data-column="actions"]');
      await expect(actions.getByRole("button")).toHaveCount(1);
      await expect(actions.getByRole("radio")).toHaveCount(6);
      await expect(actions.getByRole("checkbox")).toBeChecked();
      await expect(row.locator(".model-rule-identity")).toBeInViewport();
      await expect(row.locator(".model-rule-identity strong")).toBeVisible();
      await expect(row.locator(".model-rule-identity code")).toHaveCount(0);
      if (width === 1160) {
        expect(await row.evaluate((element) => element.getBoundingClientRect().height)).toBeLessThanOrEqual(48);
      }
      await expect(actions.getByRole("button")).toBeInViewport();
      await expect(actions.getByRole("checkbox")).toBeInViewport();
      await expect(actions.locator(".pool-speed-control.icons-only button.active .sr-only")).toHaveCount(1);
      expect(await page.locator(".model-rules > .relay-table-wrap").evaluate((wrapper) => wrapper.scrollWidth <= wrapper.clientWidth + 1)).toBe(true);
      expect(await page.locator(".model-rules > .relay-table-wrap").evaluate((wrapper) => {
        const style = getComputedStyle(wrapper);
        return style.backgroundColor !== "rgba(0, 0, 0, 0)" && style.borderTopWidth === "1px";
      })).toBe(true);
      expect(await actions.locator(".pool-speed-control").evaluate((element) => {
        const control = element.getBoundingClientRect();
        const cell = element.closest("[data-column='actions']")!.getBoundingClientRect();
        const widths = [...element.querySelectorAll("button")].map((button) => button.getBoundingClientRect().width);
        const equalIcons = widths.length === 3 && widths.every((width) => Math.abs(width - widths[0]!) < 1);
        return equalIcons && control.left >= cell.left - 1 && control.right <= cell.right + 1;
      })).toBe(true);
      for (const selector of [".pool-speed-control", ".pool-context-options"]) {
        expect(await actions.locator(selector).evaluate((element) => {
          const strip = element.getBoundingClientRect();
          const control = element.closest(".pool-model-control")!.getBoundingClientRect();
          const buttons = [...element.querySelectorAll("button")];
          const rects = buttons.map((button) => button.getBoundingClientRect());
          return buttons.length === 3
            && Math.abs(rects[0]!.left - strip.left) < 1
            && Math.abs(rects[rects.length - 1]!.right - strip.right) < 1
            && buttons.every((button, index) => {
              const style = getComputedStyle(button);
              const rect = rects[index]!;
              return [style.borderTopLeftRadius, style.borderTopRightRadius, style.borderBottomLeftRadius, style.borderBottomRightRadius].every((radius) => radius === "0px")
                && Math.abs(rect.top - control.top) < 1
                && Math.abs(rect.bottom - control.bottom) < 1
                && (index === 0 || Math.abs(rect.left - rects[index - 1]!.right) < 1);
            });
        }), `${selector} should fill its strip without inset pills or gaps`).toBe(true);
      }
      const layout = await actions.evaluate((cell) => {
        const rect = cell.getBoundingClientRect();
        const controls = [...cell.querySelectorAll(".model-rule-actions > *")].map((control) => control.getBoundingClientRect());
        if (controls.length !== 4 || !controls.every((control) => control.left >= rect.left - 1 && control.right <= rect.right + 1)) return false;
        const speed = cell.querySelector(".pool-speed-control")!.getBoundingClientRect();
        const context = cell.querySelector(".pool-context-control")!.getBoundingClientRect();
        const reasoning = controls[2];
        const toggle = controls[3];
        const speedCell = controls[0];
        const contextCell = controls[1];
        const sameRow = Math.abs(speedCell.top - contextCell.top) < 1;
        const contextBorder = getComputedStyle(cell.querySelector('[data-setting="context"]')!);
        return { ok: Math.abs(reasoning.top + reasoning.height / 2 - toggle.top - toggle.height / 2) < 1
          && (sameRow ? Math.abs(contextCell.left - speedCell.right) < 1 && contextBorder.borderInlineStartWidth === "1px" && context.left - speed.right >= 12
            : Math.abs(contextCell.left - speedCell.left) < 1 && Math.abs(contextCell.top - speedCell.bottom) < 1 && contextBorder.borderTopWidth === "1px"), speed: speed.toJSON(), context: context.toJSON(), reasoning: reasoning.toJSON(), toggle: toggle.toJSON() };
      });
      expect(layout).toMatchObject({ ok: true });
      expect(await row.locator(".model-rule-identity").evaluate((identity) => identity.getBoundingClientRect().right <= identity.parentElement!.getBoundingClientRect().right)).toBe(true);
      await page.screenshot({ path: testInfo.outputPath("model-rules.png"), animations: "disabled" });
      await page.locator(".model-rules > .relay-table-wrap").screenshot({ path: testInfo.outputPath("model-actions.png"), animations: "disabled" });
      await actions.locator("[data-model-reasoning-edit]").click();
      await expect(page.getByRole("dialog")).toBeVisible();
      await page.getByRole("dialog").screenshot({ path: testInfo.outputPath("reasoning.png"), animations: "disabled" });
      await page.keyboard.press("Escape");
      const speed = actions.getByRole("radiogroup", { name: "Скорость запроса" });
      await expect(speed.getByRole("radio", { name: "Сверхбыстрая", exact: true })).toBeChecked();
      await expect(speed.getByRole("radio")).toHaveCount(3);
      await speed.screenshot({ path: testInfo.outputPath("speed-menu.png"), animations: "disabled" });
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    });
  }
}
