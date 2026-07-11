import { expect, test } from "@playwright/test";
import { installTauriMock } from "./tauri-mock";

const modes = ["local", "remote", "zenith"] as const;
const themes = ["light", "dark"] as const;
const locales = ["en", "ru"] as const;
const viewports = [{ width: 1160, height: 760 }, { width: 840, height: 560 }] as const;

for (const locale of locales) {
  for (const mode of modes) {
    for (const theme of themes) {
      for (const viewport of viewports) {
        test(`${locale} ${mode} ${theme} ${viewport.width}x${viewport.height}`, async ({ page }) => {
          await installTauriMock(page, { locale, mode, theme, populated: true });
          await page.setViewportSize(viewport);
          await page.goto("/");
          const nav = page.locator(".relay-sidebar nav button");
          const expectedPages = mode === "zenith" ? 6 : 7;
          await expect(nav).toHaveCount(expectedPages);
          for (let index = 0; index < expectedPages; index += 1) {
            await nav.nth(index).click();
            const settingsSections = page.locator(".settings-layout > nav button");
            const tabControls = page.locator(".relay-tabs [role=tab]");
            const stateControls = await settingsSections.count() ? settingsSections : tabControls;
            const stateCount = Math.max(1, await stateControls.count());

            for (let state = 0; state < stateCount; state += 1) {
              if (await stateControls.count()) await stateControls.nth(state).click();
              await expect(page.locator(".relay-page-header h1")).toBeVisible();
              await expect(page.locator("body")).not.toContainText(/(?:common|nav|overview|connections|pool|gateway|usage|profiles|settings)\.[a-z]/);
              expect(await page.evaluate(() => document.documentElement.scrollWidth <= document.documentElement.clientWidth)).toBe(true);
              expect(await page.evaluate(() => [...document.querySelectorAll<HTMLElement>(".relay-page button, .relay-page input, .relay-page select")].filter((element) => {
                const rect = element.getBoundingClientRect();
                const intersectsViewport = rect.right > 0 && rect.left < innerWidth && rect.bottom > 36 && rect.top < innerHeight;
                return intersectsViewport && (rect.left < 0 || rect.right > innerWidth);
              }).map((element) => element.outerHTML.slice(0, 160)))).toEqual([]);
              expect(await page.evaluate(() => [...document.querySelectorAll<HTMLElement>(".relay-table td.row-actions, [data-page='profiles'] .relay-table tbody td:last-child")].filter((cell) => {
                const wrap = cell.closest<HTMLElement>(".relay-table-wrap");
                if (!wrap) return false;
                const cellRect = cell.getBoundingClientRect();
                const wrapRect = wrap.getBoundingClientRect();
                return cellRect.left < wrapRect.left - 1 || cellRect.right > wrapRect.right + 1;
              }).map((cell) => cell.outerHTML.slice(0, 160)))).toEqual([]);

              const stateSuffix = stateCount > 1 ? `-tab-${state + 1}` : "";
              await page.screenshot({ path: `output/playwright/${locale}-${mode}-${theme}-${viewport.width}x${viewport.height}-page-${index + 1}${stateSuffix}.png` });
              if (state === 0 && stateCount > 1) {
                await page.screenshot({ path: `output/playwright/${locale}-${mode}-${theme}-${viewport.width}x${viewport.height}-page-${index + 1}.png` });
              }
            }
          }
        });
      }
    }
  }
}

for (const viewport of viewports) {
  test(`proxy controls ${viewport.width}x${viewport.height}`, async ({ page }) => {
    await installTauriMock(page, { locale: "en", mode: "local", theme: "light", populated: true });
    await page.setViewportSize(viewport);
    await page.goto("/");
    await page.getByRole("button", { name: "Gateway", exact: true }).click();
    await page.locator(".proxy-settings").scrollIntoViewIfNeeded();
    await expect(page.locator(".proxy-settings")).toBeVisible();
    await page.screenshot({ path: `output/playwright/proxy-common-${viewport.width}x${viewport.height}.png` });

    await page.getByRole("button", { name: "Connections", exact: true }).click();
    await page.getByRole("button", { name: "Common", exact: true }).click();
    await expect(page.getByRole("dialog", { name: "Proxy for Personal Plus" })).toBeVisible();
    await page.screenshot({ path: `output/playwright/proxy-account-${viewport.width}x${viewport.height}.png` });
    await page.getByRole("dialog").getByRole("button", { name: "Close" }).click();
    await page.getByRole("button", { name: "Assign proxies" }).click();
    await expect(page.getByRole("dialog", { name: "Assign account proxies" })).toBeVisible();
    await page.screenshot({ path: `output/playwright/proxy-bulk-${viewport.width}x${viewport.height}.png` });

    expect(await page.evaluate(() => document.documentElement.scrollWidth <= document.documentElement.clientWidth)).toBe(true);
    expect(await page.evaluate(() => {
      const dialog = document.querySelector<HTMLElement>(".relay-dialog");
      if (!dialog) return false;
      const rect = dialog.getBoundingClientRect();
      return rect.left >= 0 && rect.right <= innerWidth && rect.top >= 0 && rect.bottom <= innerHeight;
    })).toBe(true);
  });
}
