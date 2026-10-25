import { expect, test } from "../bun-playwright";
import { installTauriMock } from "./tauri-mock";

for (const theme of ["light", "dark"] as const) {
  test(`borderless controls retain visible keyboard focus ${theme}`, async ({ page }) => {
    await installTauriMock(page, { mode: "local", locale: "en", theme, populated: true });
    await page.goto("/");
    await page.getByRole("button", { name: "Connections", exact: true }).click();
    await page.mouse.move(0, 0);
    const signIn = page.getByRole("button", { name: "Sign in", exact: true }).first();
    const background = await signIn.evaluate((element) => getComputedStyle(element).backgroundColor);
    await page.keyboard.press("Tab");
    await signIn.focus();
    await expect(signIn).toBeFocused();
    await expect.poll(() => signIn.evaluate((element) => getComputedStyle(element).backgroundColor))
      .not.toBe(background);
    expect(await signIn.evaluate((element) => getComputedStyle(element).outlineStyle)).toBe("none");

    const tabs = page.locator(".relay-tabs");
    const inactiveTab = tabs.getByRole("tab", { name: "Sources", exact: true });
    const tabBackground = await inactiveTab.evaluate((element) => getComputedStyle(element).backgroundColor);
    await inactiveTab.focus();
    await expect(inactiveTab).toBeFocused();
    await expect.poll(() => inactiveTab.evaluate((element) => getComputedStyle(element).backgroundColor))
      .not.toBe(tabBackground);
    await expect(inactiveTab).toHaveAttribute("aria-selected", "false");
  });
}

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
  await expect(dialog.getByRole("button", { name: "Notes", exact: true })).toBeFocused();
  await page.keyboard.press("Tab");
  await expect(dialog.getByRole("button", { name: "Sign-in and Basis Points", exact: true })).toBeFocused();
  await expect(page.getByRole("tooltip")).toBeVisible();
  // Passive tooltips must not require a second Escape to close a dialog.
  await page.keyboard.press("Escape");
  await expect(page.getByRole("tooltip")).toHaveCount(0);
  await expect(dialog).toBeHidden();
});
