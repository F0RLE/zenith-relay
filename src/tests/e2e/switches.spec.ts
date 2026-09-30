import { expect, test } from "../bun-playwright";
import { installTauriMock } from "./tauri-mock";

for (const theme of ["light", "dark"] as const) {
  for (const mode of ["local", "remote"] as const) {
    test(`${mode} ${theme} model switches match settings and support keyboard changes`, async ({ page }) => {
      await installTauriMock(page, { mode, theme, locale: "en", populated: true });
      await page.goto("/");
      await page.getByRole("button", { name: "Pool", exact: true }).click();
      await page.getByRole("tab", { name: "Model Rules", exact: true }).click();
      const toggle = page.locator('[data-model-toggle="gpt-5.4"]');
      await expect(toggle).toBeChecked();
      const modelSize = await toggle.evaluate((element) => ({
        width: getComputedStyle(element).width, height: getComputedStyle(element).height,
        radius: getComputedStyle(element).borderRadius,
      }));
      await toggle.focus();
      await page.keyboard.press("Space");
      await expect(toggle).not.toBeChecked();
      await expect(toggle).toBeEnabled();
      await page.keyboard.press("Space");
      await expect(toggle).toBeChecked();
      await expect(toggle).toBeEnabled();
      await page.mouse.move(0, 0);
      await page.keyboard.press("Escape");
      await page.screenshot({ path: `output/playwright/switches-${mode}-${theme}.png` });
      await page.setViewportSize({ width: 390, height: 844 });
      await page.getByRole("button", { name: "Collapse sidebar", exact: true }).click();
      await expect(toggle).toBeInViewport();
      await page.screenshot({ path: `output/playwright/switches-${mode}-${theme}-mobile.png` });
      const settingsPage = mode === "remote" ? await page.context().newPage() : page;
      if (mode === "remote") {
        await installTauriMock(settingsPage, { mode: "local", theme, locale: "en", populated: true });
        await settingsPage.goto("/");
      }
      await settingsPage.getByRole("button", { name: "Settings", exact: true }).click();
      const settingsSwitch = settingsPage.locator(".setting-toggle input").first();
      await expect(settingsSwitch).toBeVisible();
      expect(await settingsSwitch.evaluate((element) => ({
        width: getComputedStyle(element).width, height: getComputedStyle(element).height,
        radius: getComputedStyle(element).borderRadius,
      }))).toEqual(modelSize);
      if (settingsPage !== page) await settingsPage.close();
    });
  }
}
