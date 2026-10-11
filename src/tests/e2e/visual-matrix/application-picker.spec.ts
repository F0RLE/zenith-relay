import { expect, test } from "../../bun-playwright";
import { installTauriMock } from "../tauri-mock";
import { locales, themes, viewports, TITLE_BAR_HEIGHT } from "./fixtures";

for (const locale of locales) {
  for (const theme of [...themes, "system"] as const) {
    for (const viewport of viewports) {
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
        const chatgpt = picker.getByRole("button", { name: "ChatGPT", exact: true });
        const opencode = picker.getByRole("button", { name: "OpenCode", exact: true });
        await expect(chatgpt).toBeVisible();
        await expect(opencode).toBeVisible();
        const [dialogBox, chatgptBox, opencodeBox] = await Promise.all([
          picker.boundingBox(), chatgpt.boundingBox(), opencode.boundingBox(),
        ]);
        expect(dialogBox).not.toBeNull();
        expect(dialogBox!.x).toBeGreaterThanOrEqual(0);
        expect(dialogBox!.y).toBeGreaterThanOrEqual(TITLE_BAR_HEIGHT);
        expect(dialogBox!.x + dialogBox!.width).toBeLessThanOrEqual(viewport.width);
        expect(dialogBox!.y + dialogBox!.height).toBeLessThanOrEqual(viewport.height);
        expect(chatgptBox!.x).toBe(opencodeBox!.x);
        expect(chatgptBox!.width).toBe(opencodeBox!.width);
        expect(chatgptBox!.y + chatgptBox!.height).toBeLessThan(opencodeBox!.y);
        expect(await picker.locator(".application-picker-logo").evaluateAll((images) =>
          images.every((image) => image instanceof HTMLImageElement && image.complete && image.naturalWidth > 0))).toBe(true);
        expect(await picker.locator(".relay-dialog-body, .application-picker-option").evaluateAll((elements) =>
          elements.every((element) => element.scrollWidth <= element.clientWidth && element.scrollHeight <= element.clientHeight))).toBe(true);
        const path = `output/playwright/application-picker-${locale}-${theme}-${viewport.width}x${viewport.height}`;
        await page.screenshot({ path: `${path}.png` });
        await picker.screenshot({ path: `${path}-dialog.png` });
      });
    }
  }
}
