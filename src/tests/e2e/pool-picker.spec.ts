import { expect, test } from "../bun-playwright";
import { installTauriMock } from "./tauri-mock";

for (const mode of ["local", "remote"] as const) {
  test(`${mode} pool picker preserves selections across sections and searches only visible connections`, async ({ page }) => {
    await installTauriMock(page, { mode, locale: "en", populated: true, accountCount: 4, sourceCount: 3, poolMembers: false });
    await page.goto("/");
    await page.getByRole("button", { name: "Pool", exact: true }).click();
    await page.getByRole("button", { name: "Add member", exact: true }).first().click();
    const dialog = page.getByRole("dialog", { name: "Add connections to pool" });
    const search = dialog.getByRole("searchbox", { name: "Find a connection" });
    await dialog.getByText("Personal Plus", { exact: true }).click();
    await dialog.getByRole("navigation").getByRole("button", { name: "Sources", exact: true }).click();
    await search.fill("Backup API");
    await expect(dialog.locator('.pool-member-options > label')).toHaveCount(2);
    await dialog.getByRole("checkbox", { name: "Select shown", exact: true }).check();
    await expect(dialog.getByRole("button", { name: "Add selected (3)" })).toBeEnabled();
    await search.fill("no-such-connection");
    await expect(dialog.getByText("No connections found", { exact: true })).toBeVisible();
    await expect(dialog.getByRole("button", { name: "Add selected (3)" })).toBeEnabled();
    await search.fill("");
    await dialog.getByRole("navigation").getByRole("button", { name: "All connections", exact: true }).click();
    await expect(dialog.locator(".pool-picker-option input:checked")).toHaveCount(3);
    await dialog.getByRole("navigation").getByRole("button", { name: /^Selected/ }).click();
    await expect(dialog.locator(".pool-picker-option")).toHaveCount(3);
    await dialog.getByRole("button", { name: "Add selected (3)" }).click();
    await expect(dialog).toHaveCount(0);
    await expect(page.locator(".pool-member-card")).toHaveCount(3);
    await expect(page.locator(".pool-member-card").filter({ hasText: "Example compatible API" })).toHaveCount(0);
  });
}

for (const theme of ["light", "dark"] as const) {
  for (const viewport of [{ width: 1160, height: 900 }, { width: 840, height: 560 }, { width: 390, height: 700 }]) {
    test(`pool picker layout ${theme} ${viewport.width}x${viewport.height}`, async ({ page }) => {
      await installTauriMock(page, { mode: "local", locale: "ru", theme, populated: true, accountCount: 6, sourceCount: 8, poolMembers: false });
      await page.setViewportSize(viewport);
      await page.goto("/");
      await page.getByRole("button", { name: "Пул", exact: true }).click();
      await page.getByRole("button", { name: "Добавить участника", exact: true }).first().click();
      const dialog = page.getByRole("dialog", { name: "Добавить подключения в пул" });
      await dialog.locator(".pool-picker-option").first().getByRole("checkbox").check();
      const list = dialog.locator(".pool-picker-list");
      expect(await list.evaluate((element) => element.scrollHeight > element.clientHeight)).toBe(true);
      expect(await dialog.evaluate((element) => {
        const bounds = element.getBoundingClientRect();
        const body = element.querySelector(".relay-dialog-body")!;
        const footer = element.querySelector("footer")!.getBoundingClientRect();
        return bounds.left >= 0 && bounds.right <= innerWidth && bounds.top >= 36 && bounds.bottom <= innerHeight
          && element.scrollWidth <= element.clientWidth && body.scrollHeight <= body.clientHeight + 1
          && footer.bottom <= bounds.bottom;
      })).toBe(true);
      await expect(dialog.getByRole("button", { name: "Добавить выбранные (1)" })).toBeInViewport();
      await dialog.screenshot({ path: `output/playwright/pool-picker-${theme}-${viewport.width}.png` });
      await list.evaluate((element) => { element.scrollTop = element.scrollHeight; });
      await expect(dialog.getByText("Backup API 7", { exact: true })).toBeInViewport();
      await expect(dialog.getByRole("searchbox")).toBeInViewport();
      await expect(dialog.getByRole("button", { name: "Добавить выбранные (1)" })).toBeInViewport();
    });
  }
}
