import { expect, test } from "../bun-playwright";
import { installTauriMock } from "./tauri-mock";

for (const width of [1160, 840, 390]) {
  for (const theme of ["light", "dark"] as const) {
    test(`model actions form one aligned group in ${theme} at ${width}px`, async ({ page }, testInfo) => {
      await page.setViewportSize({ width, height: 844 });
      await installTauriMock(page, { locale: "ru", mode: "local", theme, populated: true, mixedModels: true, modelSpeed: { "gpt-5.4": "ultrafast" }, modelReasoning: { "gpt-5.4": ["low", "medium", "high"] } });
      await page.goto("/");
      await page.getByRole("button", { name: "Пул", exact: true }).click();
      await page.getByRole("tab", { name: "Правила моделей", exact: true }).click();
      const row = page.locator('[data-model-id="gpt-5.4"]');
      const actions = row.locator('[data-column="actions"]');
      await expect(actions.locator("button")).toHaveCount(2);
      await expect(actions.getByRole("checkbox")).toBeChecked();
      if (width <= 600) {
        await expect(row.locator(".model-rule-identity")).toBeInViewport();
        await expect(actions.locator("button").first()).toBeInViewport();
        await expect(actions.getByRole("checkbox")).toBeInViewport();
        expect(await page.locator(".model-rules > .relay-table-wrap").evaluate((wrapper) => wrapper.scrollWidth <= wrapper.clientWidth)).toBe(true);
      }
      expect(await actions.locator(".relay-option-trigger").evaluate((button) => [...button.querySelectorAll("span")].every((span) => span.scrollWidth <= span.clientWidth + 1))).toBe(true);
      expect(await actions.evaluate((cell) => {
        const rect = cell.getBoundingClientRect();
        const buttons = [...cell.querySelectorAll("button, input")].map((control) => control.getBoundingClientRect());
        return buttons.every((button, i) => Math.abs(button.top + button.height / 2 - buttons[0].top - buttons[0].height / 2) < 1
          && button.left >= rect.left && button.right <= rect.right
          && (i === 0 || Math.abs(button.left - buttons[i - 1].right - 6) < 1));
      })).toBe(true);
      expect(await row.locator(".model-rule-identity").evaluate((identity) => identity.getBoundingClientRect().right <= identity.parentElement!.getBoundingClientRect().right)).toBe(true);
      await actions.locator("[data-model-reasoning-edit]").click();
      await expect(page.getByRole("dialog")).toBeVisible();
      await page.keyboard.press("Escape");
      await actions.locator(".model-speed-select button").click();
      await expect(page.getByRole("listbox")).toBeInViewport();
      await expect(page.getByRole("option")).toHaveCount(3);
      await page.keyboard.press("Escape");
      await page.locator(".model-rules > .relay-table-wrap").screenshot({ path: testInfo.outputPath("model-actions.png"), animations: "disabled" });
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    });
  }
}
