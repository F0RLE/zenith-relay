import { expect, test } from "../bun-playwright";
import { installTauriMock } from "./tauri-mock";

test("interactive controls have names and dialogs trap focus", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true });
  await page.goto("/");
  expect(await page.evaluate(() => [...document.querySelectorAll<HTMLElement>("button")].filter((button) => !button.innerText.trim() && !button.getAttribute("aria-label") && !button.getAttribute("title")).length)).toBe(0);
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await page.getByRole("button", { name: "Sign in" }).first().click();
  const dialog = page.getByRole("dialog");
  await expect(dialog).toBeVisible();
  await expect(dialog).toBeFocused();
  await expect(page.getByRole("tooltip")).toHaveCount(0);
  await page.keyboard.press("Shift+Tab");
  await expect(dialog.locator(":focus")).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(dialog).toBeHidden();
});
