import { expect, test } from "../bun-playwright";
import { installTauriMock } from "./tauri-mock";

for (const width of [1160, 840, 720, 600, 390, 360]) {
  for (const theme of ["light", "dark"] as const) {
    test(`model actions form one aligned group in ${theme} at ${width}px`, async ({ page }, testInfo) => {
      await page.setViewportSize({ width, height: 844 });
      await installTauriMock(page, { locale: "ru", mode: "local", theme, populated: true, mixedModels: true, modelSpeed: { "gpt-5.4": "ultrafast" }, modelReasoning: { "gpt-5.4": ["low", "medium", "high"] } });
      await page.goto("/");
      await page.getByRole("button", { name: "Пул", exact: true }).click();
      await page.getByRole("tab", { name: "Правила моделей", exact: true }).click();
      const row = page.locator('[data-model-id="gpt-5.4"]');
      const actions = row.locator('[data-column="actions"]');
      await expect(actions.getByRole("button")).toHaveCount(1);
      await expect(actions.getByRole("radio")).toHaveCount(3);
      await expect(actions.getByRole("checkbox")).toBeChecked();
      await expect(row.locator(".model-rule-identity")).toBeInViewport();
      await expect(actions.getByRole("button")).toBeInViewport();
      await expect(actions.getByRole("checkbox")).toBeInViewport();
      await expect(actions.locator(".pool-speed-control.icons-only button.active .sr-only")).toHaveCount(1);
      expect(await page.locator(".model-rules > .relay-table-wrap").evaluate((wrapper) => wrapper.scrollWidth <= wrapper.clientWidth + 1)).toBe(true);
      expect(await actions.locator(".pool-speed-control").evaluate((element) => {
        const control = element.getBoundingClientRect();
        const cell = element.closest("[data-column='actions']")!.getBoundingClientRect();
        const widths = [...element.querySelectorAll("button")].map((button) => button.getBoundingClientRect().width);
        const equalIcons = widths.length === 3 && widths.every((width) => Math.abs(width - widths[0]!) < 1);
        return equalIcons && control.left >= cell.left - 1 && control.right <= cell.right + 1;
      })).toBe(true);
      expect(await actions.evaluate((cell) => {
        const rect = cell.getBoundingClientRect();
        const controls = [...cell.querySelectorAll(".model-rule-actions > *")].map((control) => control.getBoundingClientRect());
        return controls.length === 3 && controls.every((control, i) => Math.abs(control.top + control.height / 2 - controls[0].top - controls[0].height / 2) < 1
          && control.left >= rect.left - 1 && control.right <= rect.right + 1
          && (i === 0 || Math.abs(control.left - controls[i - 1].right - 6) < 1));
      })).toBe(true);
      expect(await row.locator(".model-rule-identity").evaluate((identity) => identity.getBoundingClientRect().right <= identity.parentElement!.getBoundingClientRect().right)).toBe(true);
      await page.screenshot({ path: testInfo.outputPath("model-rules.png"), animations: "disabled" });
      await actions.locator("[data-model-reasoning-edit]").click();
      await expect(page.getByRole("dialog")).toBeVisible();
      await page.getByRole("dialog").screenshot({ path: testInfo.outputPath("reasoning.png"), animations: "disabled" });
      await page.keyboard.press("Escape");
      const speed = actions.getByRole("radiogroup", { name: "Скорость запроса" });
      await expect(speed.getByRole("radio", { name: "Сверхбыстрая", exact: true })).toBeChecked();
      await expect(speed.getByRole("radio")).toHaveCount(3);
      await speed.screenshot({ path: testInfo.outputPath("speed-menu.png"), animations: "disabled" });
      await page.locator(".model-rules > .relay-table-wrap").screenshot({ path: testInfo.outputPath("model-actions.png"), animations: "disabled" });
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    });
  }
}
