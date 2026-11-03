import { expect, test } from "../../bun-playwright";
import { installTauriMock } from "../tauri-mock";
import { locales, themes } from "./fixtures";

for (const locale of locales) {
  for (const theme of themes) {
    for (const viewport of [{ width: 1240, height: 900 }, { width: 840, height: 560 }, { width: 1160, height: 420 }]) {
      test(`sidebar layout ${locale} ${theme} ${viewport.width}x${viewport.height}`, async ({ page }) => {
        await installTauriMock(page, { locale, theme, mode: "local", populated: true });
        await page.setViewportSize(viewport);
        await page.goto("/");
        const sidebar = page.locator(".relay-sidebar");
        const mode = page.locator(".mode-picker-trigger");
        const help = sidebar.getByRole("button", { name: locale === "ru" ? "Помощь" : "Help", exact: true });
        const overview = sidebar.getByRole("button", { name: locale === "ru" ? "Обзор" : "Overview", exact: true });
        await expect(overview).toHaveAttribute("aria-current", "page");
        await expect(mode.locator("svg").first()).toBeVisible();
        expect((await mode.boundingBox())!.height).toBeLessThanOrEqual(40);
        await expect(mode.locator(".mode-picker-label")).toHaveText(locale === "ru" ? "На устройстве" : "Computer");
        await mode.click();
        expect(await mode.locator("svg").first().evaluate((element) => getComputedStyle(element).transform)).toBe("none");
        const menu = page.getByRole("menu");
        await expect(menu.getByRole("menuitemradio")).toHaveCount(2);
        expect(await menu.evaluate((element) => {
          const rect = element.getBoundingClientRect();
          return rect.left >= 0 && rect.right <= innerWidth && rect.top >= 36 && rect.bottom <= innerHeight;
        })).toBe(true);
        await page.keyboard.press("Escape");
        await expect(mode).toBeFocused();
        await page.keyboard.press("Tab");
        await expect(overview).toBeFocused();

        for (const button of await sidebar.locator("nav button").all()) {
          await button.scrollIntoViewIfNeeded();
          await expect(button).toBeInViewport({ ratio: 1 });
        }
        await help.scrollIntoViewIfNeeded();
        await expect(help).toBeInViewport({ ratio: 1 });
        await help.click();
        await expect(help).toHaveAttribute("aria-current", "page");
        await expect(sidebar.locator('[aria-current="page"]')).toHaveCount(1);
        await overview.scrollIntoViewIfNeeded();
        await overview.click();
        await expect(overview).toHaveAttribute("aria-current", "page");
        await expect(sidebar.locator('[aria-current="page"]')).toHaveCount(1);
        await expect(sidebar.locator(".sidebar-bottom-actions button")).toHaveCount(3);
        await expect(page.locator(".sidebar-footer .relay-icon-button")).toBeInViewport({ ratio: 1 });
        const geometry = await sidebar.evaluate((element) => {
          const bounds = element.getBoundingClientRect();
          const titlebar = document.querySelector(".titlebar")?.getBoundingClientRect();
          const actions = element.querySelector(".sidebar-bottom-actions")!.getBoundingClientRect();
          const footer = element.querySelector(".sidebar-footer")!.getBoundingClientRect();
          return {
            top: bounds.top, bottom: bounds.bottom, titlebarBottom: titlebar?.bottom, height: innerHeight,
            bottomActionsAnchored: actions.top > bounds.top + bounds.height / 2 && actions.bottom === footer.top && footer.bottom === bounds.bottom,
            clipped: [...element.querySelectorAll<HTMLElement>(".sidebar-navigation, .sidebar-bottom-actions, .sidebar-footer, .mode-picker-trigger")].filter((child) => {
              const rect = child.getBoundingClientRect();
              return child.scrollWidth > child.clientWidth + 1 || rect.left < bounds.left || rect.right > bounds.right;
            }).map((child) => child.className),
          };
        });
        expect(geometry.titlebarBottom).toBeDefined();
        expect(geometry.top).toBeGreaterThanOrEqual(geometry.titlebarBottom!);
        expect(geometry.bottom).toBeLessThanOrEqual(geometry.height);
        expect(geometry.bottomActionsAnchored).toBe(true);
        expect(geometry.clipped).toEqual([]);
        await overview.blur();
        await page.mouse.move(500, 1);
        await page.screenshot({ path: `output/playwright/sidebar-${locale}-${theme}-${viewport.width}x${viewport.height}.png` });
        if (viewport.width === 1240) {
          await sidebar.screenshot({ path: `output/playwright/sidebar-panel-${locale}-${theme}.png` });
        }
      });
    }
  }
}

test("feedback does not move sidebar navigation or bottom actions", async ({ page }) => {
  await installTauriMock(page, { locale: "en", theme: "dark", mode: "local", populated: true, gatewayRunning: false });
  await page.goto("/");
  await page.getByRole("button", { name: "Access", exact: true }).click();
  const positions = () => page.locator(".sidebar-navigation button, .sidebar-bottom-actions button, .sidebar-footer button").evaluateAll((buttons) => buttons.map((button) => {
    const { x, y } = button.getBoundingClientRect();
    return { x, y };
  }));
  const before = await positions();
  await page.locator(".gateway-api-controls").getByRole("button", { name: "Start API", exact: true }).click();
  await expect(page.locator(".global-feedback.success")).toBeVisible();
  expect(await positions()).toEqual(before);
  await page.locator(".global-feedback.success").hover();
  await page.locator(".global-feedback.success .relay-icon-button").click();
  await expect(page.locator(".global-feedback.success")).toHaveCount(0);
  expect(await positions()).toEqual(before);
});
