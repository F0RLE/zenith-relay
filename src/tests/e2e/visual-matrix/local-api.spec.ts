import { expect, test } from "../../bun-playwright";
import { installTauriMock } from "../tauri-mock";
import { locales, themes } from "./fixtures";

for (const locale of locales) {
  for (const theme of themes) {
    for (const width of [1240, 840, 390]) {
      test(`local API connection layout ${locale} ${theme} ${width}`, async ({ page }) => {
        await installTauriMock(page, { mode: "local", locale, theme, populated: true, gatewayRunning: true });
        await page.setViewportSize({ width, height: 900 });
        await page.goto("/");
        await page.getByRole("button", { name: locale === "ru" ? "Доступ" : "Access", exact: true }).click();
        const panel = page.locator(".gateway-api-connection-panel");
        await expect(panel).toBeVisible();
        await expect(panel.getByRole("heading", { name: locale === "ru" ? "На этом устройстве" : "On this device", exact: true })).toBeVisible();
        await expect(page.locator(".gateway-api-secret")).toHaveCount(1);
        await expect(page.locator(".gateway-api-controls").getByRole("button", { name: locale === "ru" ? "Остановить API" : "Stop API", exact: true })).toBeVisible();

        const sharingName = locale === "ru" ? "Доступ по локальной сети" : "Allow local network access";
        const sharing = page.getByRole("checkbox", { name: sharingName, exact: true });
        await expect(sharing).toBeEnabled();
        await expect(sharing).not.toBeChecked();
        await expect(page.locator(".gateway-network-access")).toContainText(locale === "ru"
          ? "Для других устройств в вашей локальной сети."
          : "For other devices on your local network.");
        await expect(page.locator(".gateway-network-details")).toHaveCount(0);
        if (locale === "ru" && theme === "dark" && width === 1240) {
          await page.screenshot({ path: "output/playwright/local-api-network-off-ru-dark.png" });
        }
        await sharing.click();
        await page.getByRole("dialog", { name: sharingName, exact: true }).getByRole("button", { name: locale === "ru" ? "Включить" : "Enable", exact: true }).click();
        await expect(sharing).toBeChecked();
        await expect(page.locator(".gateway-network-access")).toContainText(locale === "ru"
          ? "Используйте адрес нужной сети и сетевой ключ."
          : "Use the LAN address and network key.");
        await expect(page.locator(".gateway-network-details code")).toHaveText("http://192.168.1.10:14998/v1");
        await expect(page.locator(".gateway-api-endpoint code")).toHaveText("http://127.0.0.1:14998/v1");
        await expect(page.locator(".gateway-api-secret")).toHaveCount(2);
        expect(await page.evaluate(() => (window as unknown as {
          __TAURI_TEST_INVOKES__: Array<{ command: string }>;
        }).__TAURI_TEST_INVOKES__.some(({ command }) => command === "reveal_local_gateway_api_key"))).toBe(false);

        const portBox = await page.locator(".gateway-api-settings").boundingBox();
        const networkBox = await page.locator(".gateway-network-access").boundingBox();
        expect(portBox && networkBox && networkBox.y >= portBox.y + portBox.height).toBeTruthy();
        expect(await panel.evaluate((element) => {
          const bounds = element.getBoundingClientRect();
          return [...element.querySelectorAll<HTMLElement>(".gateway-api-section, .gateway-api-value, .gateway-api-controls, .gateway-api-toggle-setting")].every((child) => {
            const rect = child.getBoundingClientRect();
            return child.scrollWidth <= child.clientWidth + 1 && rect.left >= bounds.left && rect.right <= bounds.right + 1;
          });
        })).toBe(true);
        const panelWidth = await panel.evaluate((element) => element.clientWidth);
        for (const fields of await page.locator(".gateway-api-fields").all()) {
          const values = await fields.locator(".gateway-api-value").evaluateAll((nodes) => nodes.map((node) => {
            const { x, y, width, height } = node.getBoundingClientRect();
            return { x, y, width, height };
          }));
          expect(values).toHaveLength(2);
          if (panelWidth > 600) {
            expect(Math.abs(values[0].y - values[1].y)).toBeLessThanOrEqual(1);
            expect(values[0].x + values[0].width).toBeLessThan(values[1].x);
          } else {
            expect(values[0].y + values[0].height).toBeLessThan(values[1].y);
          }
        }
        await page.locator(".gateway-api-overview").scrollIntoViewIfNeeded();
        await page.mouse.move(0, 0);
        await page.screenshot({ path: `output/playwright/local-api-${locale}-${theme}-${width}.png` });
        await page.locator(".gateway-api-toggle-setting input").scrollIntoViewIfNeeded();
        await expect(page.locator(".gateway-api-toggle-setting input")).toBeInViewport();
      });
    }
  }
}
