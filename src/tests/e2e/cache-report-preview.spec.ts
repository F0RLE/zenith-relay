import { expect, test } from "../bun-playwright";
import { installTauriMock } from "./tauri-mock";

for (const theme of ["dark", "light"] as const) {
  test(`cache remaining preview ${theme}`, async ({ page }) => {
    await page.setViewportSize({ width: 1280, height: 980 });
    await installTauriMock(page, { mode: "local", locale: "ru", theme, populated: true, accountCount: 2, cachePreview: true });
    await page.goto("/");
    await page.getByRole("button", { name: "Использование", exact: true }).click();
    await expect(page.getByRole("tab", { name: "Кэш", exact: true })).toHaveCount(0);
    await page.getByRole("button", { name: "Сведения о запросе: req_synthetic_local" }).click();
    await page.getByRole("tab", { name: "Токены", exact: true }).click();
    const dialog = page.getByRole("dialog");
    await expect(dialog.getByText("ещё около 18 мин")).toBeVisible();
    await dialog.screenshot({ path: `output/cache-remaining-${theme}.png` });
  });
}
