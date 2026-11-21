import { expect, test } from "../bun-playwright";
import { installTauriMock } from "./tauri-mock";
import { en } from "../../src/i18n/locales/en";
import { ru } from "../../src/i18n/locales/ru";

for (const variant of [
  { width: 1160, theme: "light" as const, direction: "ltr" },
  { width: 390, theme: "dark" as const, direction: "rtl" },
]) {
  test(`language search, persistence and native labels ${variant.width}/${variant.direction}`, async ({ page }) => {
    await installTauriMock(page, { locale: "en", theme: variant.theme, populated: true });
    await page.setViewportSize({ width: variant.width, height: 900 });
    await page.goto("/");
    await page.getByRole("button", { name: "Settings", exact: true }).click();
    await expect(page.locator(".settings-page")).toBeVisible();
    await page.evaluate(direction => { document.documentElement.dir = direction; }, variant.direction);
    const picker = page.locator(".relay-language-picker .relay-option-trigger");
    await expect(picker.locator("span")).toHaveText("English");
    expect((await picker.locator("span").boundingBox())!.width).toBeGreaterThan(40);
    await picker.click();
    const search = page.getByRole("searchbox", { name: "Find a language" });
    await expect(search).toBeFocused();
    const popup = (await page.locator(".relay-language-options").boundingBox())!;
    expect(popup.x).toBeGreaterThanOrEqual(8);
    expect(popup.x + popup.width).toBeLessThanOrEqual(variant.width - 7);
    await search.fill("Russian");
    await expect(page.getByRole("option")).toHaveCount(1);
    await search.press("ArrowDown");
    await page.keyboard.press("Enter");
    await expect(page.locator("html")).toHaveAttribute("lang", "ru");
    await expect(page.locator("html")).toHaveAttribute("dir", "ltr");
    await expect.poll(() => page.evaluate(() => localStorage.getItem("relay.language"))).toBe("ru");
    await expect.poll(() => page.evaluate(() => {
      const calls = (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: { language?: string; messages?: { quit?: string } } }> }).__TAURI_TEST_INVOKES__;
      return calls.filter(call => call.command === "set_ui_language").at(-1)?.args.messages?.quit;
    })).toBe(ru.native.quit);
    await picker.click();
    await page.getByRole("searchbox", { name: ru.localization.search }).fill("en");
    await page.getByRole("option", { name: "English", exact: true }).click();
    await expect(page.locator("html")).toHaveAttribute("lang", "en");
    await page.evaluate(direction => { document.documentElement.dir = direction; }, variant.direction);
    if (variant.direction === "rtl") {
      const sidebar = await page.locator(".relay-sidebar").boundingBox();
      const content = await page.locator(".relay-content").boundingBox();
      expect(sidebar!.x).toBeGreaterThan(content!.x);
    }
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1)).toBe(true);
    await page.screenshot({ path: `output/playwright/localization-${variant.width}-${variant.direction}.png` });
    await page.reload();
    await expect(page.locator("html")).toHaveAttribute("lang", "en");
    expect(await page.evaluate(() => JSON.parse(localStorage.getItem("relay.startupCopy")!).copy.reload)).toBe(en.startup.reload);
  });
}

test("pre-bundle fallback works with storage denied", async ({ page }) => {
  await installTauriMock(page, { locale: "en", populated: true });
  await page.addInitScript(() => {
    Storage.prototype.getItem = () => { throw new DOMException("Synthetic storage denial", "SecurityError"); };
    Storage.prototype.setItem = () => { throw new DOMException("Synthetic storage denial", "SecurityError"); };
  });
  await page.goto("/");
  await expect(page.locator("#splash-screen")).toHaveCount(0);
  // Without saved preferences the existing first-run setup is expected.
  await page.getByRole("button", { name: "Skip setup", exact: true }).click();
  await expect(page.locator(".relay-shell")).toBeVisible();
});

test("failed native preference saves are diagnosed without reverting the selected language", async ({ page }) => {
  await installTauriMock(page, { locale: "en", populated: true });
  await page.addInitScript(() => {
    const internals = (window as unknown as {
      __TAURI_INTERNALS__: { invoke: (name: string, args?: Record<string, unknown>, options?: unknown) => Promise<unknown> };
    }).__TAURI_INTERNALS__;
    const invoke = internals.invoke.bind(internals);
    internals.invoke = async (name, args, options) => {
      const result = await invoke(name, args, options);
      if (name === "set_ui_language" && args?.language === "ru") throw new Error("Could not save UI language");
      return result;
    };
  });
  await page.goto("/");
  await page.getByRole("button", { name: "Settings", exact: true }).click();
  const picker = page.locator(".relay-language-picker .relay-option-trigger");
  await picker.click();
  await page.getByRole("searchbox", { name: en.localization.search }).fill("Russian");
  await page.getByRole("option", { name: "Русский", exact: true }).click();
  await expect(page.locator("html")).toHaveAttribute("lang", "ru");
  await expect(page.locator(".relay-language-picker [role='alert']")).toHaveCount(0);
  await expect.poll(() => page.evaluate(() => {
    const calls = (window as unknown as {
      __TAURI_TEST_INVOKES__: Array<{ command: string; args: { input?: { operation?: string; message?: string } } }>;
    }).__TAURI_TEST_INVOKES__;
    return calls.some(call => call.command === "record_frontend_diagnostic" && call.args.input?.operation === "set_ui_language" && call.args.input.message === "Could not save UI language");
  })).toBe(true);
  await picker.click();
  await page.getByRole("searchbox", { name: ru.localization.search }).fill("English");
  await page.getByRole("option", { name: "English", exact: true }).click();
  await expect(page.locator("html")).toHaveAttribute("lang", "en");
  await expect.poll(() => page.evaluate(() => {
    const calls = (window as unknown as {
      __TAURI_TEST_INVOKES__: Array<{ command: string; args: { language?: string } }>;
    }).__TAURI_TEST_INVOKES__;
    return calls.filter(call => call.command === "set_ui_language").at(-1)?.args.language;
  })).toBe("en");
});
