import { expect, test } from "../bun-playwright";
import { installTauriMock } from "./tauri-mock";

for (const width of [1160, 840, 390, 360]) {
  test(`Russian controls fit fallback fonts at ${width}px`, async ({ page }, testInfo) => {
    await page.setViewportSize({ width, height: 844 });
    await installTauriMock(page, { mode: "local", locale: "ru", populated: true, accountCount: 3, providerCredits: 125.5 });
    await page.goto("/");
    await page.addStyleTag({ content: ".app { font-family: Verdana, sans-serif !important; }" });

    for (const [view, label, selector, maxHeight] of [
      ["connections", "Подключения", ".connections-account-controls", 120],
      ["pool", "Пул", ".pool-controls", 170],
    ] as const) {
      await page.getByRole("button", { name: label, exact: true }).click();
      const panel = page.locator(selector);
      await expect(panel.locator('[data-summary="provider-credits"] strong')).toHaveText("376,5");
      if (width === 1160) expect((await panel.boundingBox())!.height).toBeLessThanOrEqual(maxHeight);
      await panel.screenshot({ path: testInfo.outputPath(`${view}.png`), animations: "disabled" });
    }

    const speed = page.locator(".pool-speed-control");
    const initial = (await speed.boundingBox())!;
    for (const name of ["Обычная", "Быстрая", "Сверхбыстрая"]) {
      const option = speed.getByRole("radio", { name, exact: true });
      await option.click();
      await expect(option).toBeEnabled();
      expect(await speed.locator("button.active span").evaluate((element) => {
        const bounds = element.parentElement!.getBoundingClientRect();
        const text = element.getBoundingClientRect();
        return element.scrollWidth <= element.clientWidth + 1
          && text.left >= bounds.left && text.right <= bounds.right;
      })).toBe(true);
      const current = (await speed.boundingBox())!;
      expect({ width: current.width, height: current.height }).toEqual({ width: initial.width, height: initial.height });
      expect(current.x).toBeGreaterThanOrEqual(0);
      expect(current.x + current.width).toBeLessThanOrEqual(width);
    }
    await speed.screenshot({ path: testInfo.outputPath("ultrafast.png"), animations: "disabled" });
  });
}
