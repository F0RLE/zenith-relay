import { expect, test } from "../../bun-playwright";
import { installTauriMock } from "../tauri-mock";
import { locales, themes, viewports, TITLE_BAR_HEIGHT } from "./fixtures";

const pickerViewports = [...viewports, { width: 390, height: 844 }, { width: 488, height: 348 }];

for (const locale of locales) {
  for (const theme of [...themes, "system"] as const) {
    for (const viewport of pickerViewports) {
      test(`application picker ${locale} ${theme} ${viewport.width}x${viewport.height}`, async ({ page }) => {
        await installTauriMock(page, { locale, theme, mode: "local", populated: true, gatewayRunning: true });
        await page.emulateMedia({ colorScheme: "dark" });
        await page.setViewportSize(viewport);
        await page.goto("/");
        await page.getByRole("button", { name: locale === "ru" ? "Пул" : "Pool", exact: true }).click();
        await page.getByRole("button", { name: locale === "ru" ? "Подключить" : "Connect", exact: true }).click();
        const picker = page.getByRole("dialog", { name: locale === "ru" ? "Подключить приложение" : "Connect an application" });
        await expect(picker).toBeVisible();
        await expect(picker.getByRole("checkbox")).toHaveCount(0);
        const options = picker.locator(".application-picker-option");
        await expect(options).toHaveText(["ChatGPT", "Claude Desktop", "OpenCode"]);
        const dialogBox = await picker.boundingBox();
        expect(dialogBox).not.toBeNull();
        expect(dialogBox!.x).toBeGreaterThanOrEqual(0);
        expect(dialogBox!.y).toBeGreaterThanOrEqual(TITLE_BAR_HEIGHT);
        expect(dialogBox!.x + dialogBox!.width).toBeLessThanOrEqual(viewport.width);
        expect(dialogBox!.y + dialogBox!.height).toBeLessThanOrEqual(viewport.height);
        expect(await options.evaluateAll((elements) => elements.every((element, index) => {
          const box = element.getBoundingClientRect();
          const previous = elements[index - 1]?.getBoundingClientRect();
          const logo = element.querySelector("img")!.getBoundingClientRect();
          const label = element.querySelector(".application-picker-label")!.getBoundingClientRect();
          return (!previous || previous.right < box.left && previous.top === box.top && Math.abs(previous.width - box.width) < 1)
            && logo.bottom < label.top
            && Math.abs(logo.left + logo.width / 2 - box.left - box.width / 2) < 1;
        }))).toBe(true);
        await expect.poll(() => picker.locator(".application-picker-logo").evaluateAll((images) =>
          images.length === 3 && images.every((image) => image instanceof HTMLImageElement && image.complete && image.naturalWidth > 0))).toBe(true);
        expect(await picker.locator(".relay-dialog-body, .application-picker-option").evaluateAll((elements) =>
          elements.every((element) => element.scrollWidth <= element.clientWidth && element.scrollHeight <= element.clientHeight))).toBe(true);
        const path = `output/playwright/application-picker-${locale}-${theme}-${viewport.width}x${viewport.height}`;
        await page.screenshot({ path: `${path}.png` });
        await picker.screenshot({ path: `${path}-dialog.png` });
      });
    }
  }
}
