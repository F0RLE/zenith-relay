import { expect, test } from "@playwright/test";
import type { Locator, Page } from "@playwright/test";

async function expectKeyboardFocus(control: Locator, restingBackground: string) {
  await expect(control).toBeFocused();
  await expect(control).toHaveCSS("outline-style", "none");
  await expect(control).toHaveCSS("outline-width", "0px");
  await expect(control).not.toHaveCSS("background-color", restingBackground);
  expect(await control.evaluate(element => element.matches(":focus-visible"))).toBe(true);
}

const desktopMenuQuery = "(min-width: 701px) and (hover: hover) and (pointer: fine)";
const picker = (page: Page) => page.locator(".language-switcher").getByRole("combobox");
const isDesktopMenu = (page: Page) => page.evaluate(query => matchMedia(query).matches, desktopMenuQuery);

async function chooseLanguage(page: Page, code: string, label: string) {
  if (await page.locator("html").getAttribute("lang") === code) return;
  if (await page.locator(".language-roller").count()) {
    await picker(page).click();
    const candidate = page.locator(".language-roller-list").getByRole("option");
    for (let step = 0; step < 3 && await candidate.getAttribute("aria-label") !== label; step++) {
      await page.locator('.language-roller-step[data-direction="next"]').click();
    }
    await expect(candidate).toHaveAttribute("aria-label", label);
    await candidate.click();
  } else if (await page.locator(".landing-language-switcher").count() || await isDesktopMenu(page)) {
    await picker(page).click();
    await page.getByRole("option", { name: label, exact: true }).click();
  } else {
    await picker(page).selectOption(code);
  }
  await expect(page.locator("html")).toHaveAttribute("lang", code);
}

test("language selection persists and fits the header in every locale", async ({ page }) => {
  await page.goto("/");
  expect(await picker(page).evaluate(element => element.tagName)).toBe("BUTTON");
  const controlBox = (await picker(page).boundingBox())!;
  const themeBox = (await page.locator(".landing-header .key-icon").boundingBox())!;
  expect(controlBox.width).toBe(themeBox.width);
  expect(controlBox.height).toBe(themeBox.height);
  await expect(page.locator(".language-native-chevron")).toHaveCount(0);
  for (const [code, label] of [["en", "English"], ["zh-CN", "简体中文"], ["ru", "Русский"]]) {
    await chooseLanguage(page, code!, label!);
    const flag = page.locator(".language-trigger .language-flag");
    await expect(flag).toBeVisible();
    await expect(flag).toHaveAttribute("data-locale", code!);
    await expect(flag).toHaveCSS("width", "24px");
    await expect(flag).toHaveCSS("height", "16px");
    const flagBox = (await flag.boundingBox())!;
    const pickerBox = (await picker(page).boundingBox())!;
    expect(Math.abs(flagBox.x + flagBox.width / 2 - pickerBox.x - pickerBox.width / 2)).toBeLessThanOrEqual(1);
    expect(Math.abs(flagBox.y + flagBox.height / 2 - pickerBox.y - pickerBox.height / 2)).toBeLessThanOrEqual(1);
    expect(await page.locator(".landing-header-actions").evaluate(element => {
      const rect = element.getBoundingClientRect();
      const brand = document.querySelector(".landing-header .landing-brand")!.getBoundingClientRect();
      return rect.left >= brand.right && rect.right <= innerWidth;
    })).toBe(true);
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    await page.reload();
    await expect(page.locator("html")).toHaveAttribute("lang", code!);
  }
});

test("landing languages expand left as flags without moving the header", async ({ page }) => {
  await page.goto("/");
  await page.evaluate(() => document.fonts.ready);
  const control = picker(page);
  const buttonBox = (await control.boundingBox())!;
  const controls = page.locator(".landing-header .landing-brand, .landing-header .key-icon, .landing-header a.key-button");
  const before = await controls.evaluateAll(elements => elements.map(element => element.getBoundingClientRect().toJSON()));
  await control.click();
  const menu = page.getByRole("listbox", { name: "Язык", exact: true });
  await expect(menu).toBeVisible();
  await expect(menu.getByRole("option")).toHaveCount(3);
  await expect(menu.locator(".language-flag")).toHaveCount(3);
  expect(await menu.evaluate(element => element.closest(".language-switcher") !== null)).toBe(true);
  const menuBox = (await menu.boundingBox())!;
  expect(menuBox.y).toBe(buttonBox.y);
  expect(menuBox.x).toBeLessThan(buttonBox.x);
  expect(Math.abs(menuBox.x + menuBox.width - buttonBox.x - buttonBox.width)).toBeLessThan(1);
  const brandBox = (await controls.first().boundingBox())!;
  expect(menuBox.x).toBeGreaterThanOrEqual(brandBox.x + brandBox.width + 11);
  expect(await controls.evaluateAll(elements => elements.map(element => element.getBoundingClientRect().toJSON()))).toEqual(before);
  await expect(menu).toHaveAttribute("aria-orientation", "horizontal");
  await expect(menu).toHaveCSS("border-top-width", "0px");
  await expect(menu).toHaveCSS("box-shadow", "none");
  await expect(menu).toHaveText("");
  await expect(page.locator(".language-menu")).toHaveCount(0);
  await expect(menu.getByRole("option", { name: "Русский", exact: true })).toHaveAttribute("aria-selected", "true");
  for (const option of await menu.getByRole("option").all()) {
    expect(await option.evaluate(element => element.getBoundingClientRect().height)).toBe(buttonBox.height);
    expect(await option.evaluate(element => element.getBoundingClientRect().width)).toBe(buttonBox.width);
    await expect(option).toHaveCSS("border-radius", "8px");
    await expect(option.locator(".language-flag")).toHaveCSS("width", "24px");
    await expect(option.locator(".language-flag")).toHaveCSS("height", "16px");
    const optionBox = (await option.boundingBox())!, flagBox = (await option.locator(".language-flag").boundingBox())!;
    expect(Math.abs(flagBox.x + flagBox.width / 2 - optionBox.x - optionBox.width / 2)).toBeLessThanOrEqual(1);
    expect(Math.abs(flagBox.y + flagBox.height / 2 - optionBox.y - optionBox.height / 2)).toBeLessThanOrEqual(1);
  }
  expect(await menu.evaluate(element => {
    const rect = element.getBoundingClientRect();
    return rect.left >= 0 && rect.top >= 0 && rect.right <= innerWidth && rect.bottom <= innerHeight;
  })).toBe(true);
  await menu.getByRole("option", { name: "English", exact: true }).click();
  await expect(page.getByRole("listbox")).toHaveCount(0);
  await expect(control).toBeFocused();
  await expect(control).toHaveCSS("outline-style", "none");
  await expect(control.locator(".language-flag")).toHaveAttribute("data-locale", "en");
  await expect(page.locator("html")).toHaveAttribute("lang", "en");
  await control.click();
  await expect(page.getByRole("option", { name: "English", exact: true })).toHaveAttribute("aria-selected", "true");
  await expect(page.getByRole("option")).toHaveCount(3);
  await page.locator(".landing-header .key-icon").click();
  await expect(page.getByRole("listbox")).toHaveCount(0);
  await control.click();
  await page.locator(".landing-hero").click({ position: { x: 20, y: 20 } });
  await expect(page.getByRole("listbox")).toHaveCount(0);
});

test("inline language choices retain keyboard focus, navigation and dismissal", async ({ page }, testInfo) => {
  await page.goto("/");
  const control = picker(page);
  const restingBackground = await control.evaluate(element => getComputedStyle(element).backgroundColor);
  await page.locator(".landing-header .landing-brand").focus();
  await page.keyboard.press("Tab");
  await expectKeyboardFocus(control, restingBackground);
  await page.screenshot({ path: testInfo.outputPath("language-keyboard-focus.png") });
  await page.keyboard.press("Space");
  await expect(page.getByRole("listbox")).toBeFocused();
  await expect(page.getByRole("option", { name: "Русский", exact: true })).toHaveAttribute("data-active", "true");
  await page.keyboard.press("ArrowRight");
  await expect(page.getByRole("option", { name: "English", exact: true })).toHaveAttribute("data-active", "true");
  await page.keyboard.press("ArrowRight");
  await expect(page.getByRole("option", { name: "简体中文", exact: true })).toHaveAttribute("data-active", "true");
  await page.keyboard.press("ArrowLeft");
  await expect(page.getByRole("option", { name: "English", exact: true })).toHaveAttribute("data-active", "true");
  await page.keyboard.press("Enter");
  await expect(control.locator(".language-flag")).toHaveAttribute("data-locale", "en");
  await expectKeyboardFocus(control, restingBackground);
  await page.keyboard.press("End");
  await expect(page.getByRole("option", { name: "简体中文", exact: true })).toHaveAttribute("data-active", "true");
  await page.keyboard.press("Home");
  await expect(page.getByRole("option", { name: "Русский", exact: true })).toHaveAttribute("data-active", "true");
  await page.keyboard.press("Escape");
  await expect(control.locator(".language-flag")).toHaveAttribute("data-locale", "en");
  await expect(page.getByRole("listbox")).toHaveCount(0);
  await page.keyboard.press("ArrowUp");
  await page.keyboard.press("Tab");
  await expect(page.getByRole("listbox")).toHaveCount(0);
  await expect(page.locator(".landing-header .key-icon")).toBeFocused();
  await page.keyboard.press("Shift+Tab");
  await page.keyboard.press("Home");
  await page.keyboard.press("Enter");
  await expect(page.locator("html")).toHaveAttribute("lang", "ru");
  await page.keyboard.press("Space");
  await page.keyboard.press("e");
  await expect(page.getByRole("option", { name: "English", exact: true })).toHaveAttribute("data-active", "true");
  await page.keyboard.press("Escape");
  await page.keyboard.press("Space");
  await page.keyboard.press("Shift+Tab");
  await expect(page.locator(".landing-header .landing-brand")).toBeFocused();
  await expect(page.getByRole("listbox")).toHaveCount(0);
});

test("inline languages remain usable during resize and scroll on narrow headers", async ({ page }) => {
  await page.setViewportSize({ width: 1024, height: 800 });
  await page.goto("/");
  await picker(page).click();
  await expect(page.getByRole("listbox")).toBeVisible();
  await page.setViewportSize({ width: 320, height: 740 });
  const list = page.getByRole("listbox");
  await expect(list).toBeVisible();
  await expect(page.locator(".language-select")).toHaveCount(0);
  await expect.poll(() => list.evaluate(element => element.scrollWidth > element.clientWidth)).toBe(true);
  const scrollBefore = await page.evaluate(() => scrollY);
  await list.hover();
  await page.mouse.wheel(0, 160);
  await expect.poll(() => list.evaluate(element => element.scrollLeft)).toBeGreaterThan(0);
  expect(await page.evaluate(() => scrollY)).toBe(scrollBefore);
  await list.getByRole("option", { name: "简体中文", exact: true }).click();
  await expect(page.locator("html")).toHaveAttribute("lang", "zh-CN");
  await page.setViewportSize({ width: 1024, height: 800 });
  await expect(picker(page).locator(".language-flag")).toHaveAttribute("data-locale", "zh-CN");
  await picker(page).click();
  await expect(page.getByRole("option", { name: "简体中文", exact: true })).toHaveAttribute("aria-selected", "true");
  await expect(page.getByRole("option")).toHaveCount(3);
  expect(await list.evaluate(element => element.scrollWidth <= element.clientWidth)).toBe(true);
});

test("language menu works without storage and on the unavailable screen", async ({ page }) => {
  await page.addInitScript(() => {
    Object.defineProperty(window, "localStorage", { get() { throw new DOMException("Denied", "SecurityError"); } });
  });
  await page.goto("/login");
  if (await isDesktopMenu(page)) {
    await picker(page).click();
    const menu = page.getByRole("listbox");
    await expect(menu).toBeVisible();
    const triggerBox = (await picker(page).boundingBox())!;
    expect((await menu.boundingBox())!.y).toBeGreaterThanOrEqual(triggerBox.y + triggerBox.height + 6);
    expect(await menu.evaluate(element => {
      const rect = element.getBoundingClientRect();
      return rect.left >= 0 && rect.right <= innerWidth;
    })).toBe(true);
    await page.keyboard.press("Escape");
  }
  await chooseLanguage(page, "en", "English");
  await expect(page.locator(".integration-unavailable")).toBeVisible();
  await expect(page.locator(".relay-shell")).toHaveCount(0);
  await chooseLanguage(page, "zh-CN", "简体中文");
});

test("inline language selection works without storage", async ({ page }) => {
  await page.addInitScript(() => {
    Object.defineProperty(window, "localStorage", { get() { throw new DOMException("Denied", "SecurityError"); } });
  });
  await page.goto("/");
  await chooseLanguage(page, "zh-CN", "简体中文");
  await chooseLanguage(page, "ru", "Русский");
});

test("inline flags retain reduced-motion, forced-color and zoom behavior", async ({ page }) => {
  await page.emulateMedia({ reducedMotion: "reduce" });
  await page.goto("/");
  await picker(page).click();
  const list = page.getByRole("listbox");
  await expect(list).toHaveCSS("transition-duration", "0s");
  for (const option of await list.getByRole("option").all()) await expect(option).toHaveCSS("transition-duration", "0s");
  await page.setViewportSize({ width: 320, height: 740 });
  await expect.poll(() => list.evaluate(element => element.scrollWidth > element.clientWidth)).toBe(true);
  expect(await list.evaluate(element => {
    const event = new WheelEvent("wheel", { deltaY: 120, ctrlKey: true, cancelable: true, bubbles: true });
    element.dispatchEvent(event);
    return event.defaultPrevented;
  })).toBe(false);
  await page.emulateMedia({ forcedColors: "active" });
  await page.keyboard.press("End");
  await expect(list).toHaveCSS("mask-image", "none");
  await expect(list.locator('[data-active="true"]')).toHaveCSS("outline-style", "solid");
  await page.keyboard.press("Escape");
  await expect(picker(page)).toBeFocused();
});

test("touch can scroll the inline flags without selecting them", async ({ page, context }) => {
  await page.setViewportSize({ width: 320, height: 740 });
  await page.goto("/");
  await picker(page).click();
  const list = page.getByRole("listbox");
  await expect.poll(() => list.evaluate(element => element.scrollWidth > element.clientWidth)).toBe(true);
  const box = (await list.boundingBox())!;
  const session = await context.newCDPSession(page);
  const y = box.y + box.height / 2;
  await session.send("Input.dispatchTouchEvent", { type: "touchStart", touchPoints: [{ x: box.x + box.width - 6, y }] });
  for (const distance of [12, 24, 36, 48]) await session.send("Input.dispatchTouchEvent", {
    type: "touchMove", touchPoints: [{ x: box.x + box.width - 6 - distance, y }],
  });
  await session.send("Input.dispatchTouchEvent", { type: "touchEnd", touchPoints: [] });
  await expect.poll(() => list.evaluate(element => element.scrollLeft)).toBeGreaterThan(0);
  await expect(page.locator("html")).toHaveAttribute("lang", "ru");
  await expect(list).toBeVisible();
  await session.detach();
});

test("native language picker hides the pointer ring but keeps keyboard focus", async ({ page }, testInfo) => {
  await page.setViewportSize({ width: 320, height: 740 });
  await page.goto("/login");
  const control = picker(page);
  const restingBackground = await control.evaluate(element => getComputedStyle(element).backgroundColor);
  await control.click();
  await expect(control).toHaveCSS("outline-style", "none");
  await page.keyboard.press("Escape");
  await page.keyboard.press("Tab");
  await page.keyboard.press("Shift+Tab");
  await expectKeyboardFocus(control, restingBackground);
  await page.screenshot({ path: testInfo.outputPath("language-keyboard-focus.png") });
});

test("language picker retains an explicit keyboard indicator in high contrast", async ({ page }) => {
  await page.emulateMedia({ forcedColors: "active" });
  await page.goto("/");
  await page.locator(".landing-header .landing-brand").focus();
  await page.keyboard.press("Tab");
  const control = picker(page);
  await expect(control).toBeFocused();
  await expect(control).toHaveCSS("outline-style", "solid");
  await expect(control).toHaveCSS("outline-width", "1px");
});

test("landing controls use flat actions and quiet header utilities", async ({ page }) => {
  await page.goto("/");
  for (const control of await page.locator(".landing .key-button").all()) {
    await expect(control).toHaveCSS("box-shadow", "none");
    await expect(control).toHaveCSS("border-radius", "8px");
  }
  for (const control of [picker(page), page.locator(".landing-header .key-icon")]) {
    await expect(control).toHaveCSS("background-color", "rgba(0, 0, 0, 0)");
    await expect(control).toHaveCSS("border-top-color", "rgba(0, 0, 0, 0)");
  }
  const original = await page.locator(".landing-home").getAttribute("data-theme");
  await page.locator(".landing-header .key-icon").click();
  await expect(page.locator(".landing-home")).toHaveAttribute("data-theme", original === "light" ? "dark" : "light");
  await expect(page.locator(".landing-header a.key-button")).toHaveAttribute("href", "/app/overview");
});

test("Hosted flag sits beside theme, preserves locale and fits both workspaces", async ({ page }) => {
  for (const path of ["/app/overview", "/admin/overview", "/preview.html?page=keys"]) {
    await page.goto(path);
    const theme = page.getByRole("button", { name: /^(Тёмная|Светлая) тема$/ });
    await expect(picker(page)).toBeVisible();
    await expect(picker(page)).toHaveCSS("background-color", "rgba(0, 0, 0, 0)");
    const control = (await picker(page).boundingBox())!;
    const themeBox = (await theme.boundingBox())!;
    expect(control.width).toBe(themeBox.width);
    expect(control.height).toBe(themeBox.height);
    expect(control.y).toBe(themeBox.y);
    expect(themeBox.x - control.x - control.width).toBeGreaterThanOrEqual(4);
    expect(themeBox.x - control.x - control.width).toBeLessThanOrEqual(8);
    expect(await page.locator(".hosted-tools").evaluate(element => {
      const rect = element.getBoundingClientRect();
      return rect.left >= 0 && rect.right <= innerWidth && element.scrollWidth <= element.clientWidth;
    })).toBe(true);
    expect(await page.locator(".relay-content").evaluate(element => element.scrollWidth <= element.clientWidth)).toBe(true);
    await chooseLanguage(page, "en", "English");
    const flag = page.locator(".language-roller-trigger .language-flag");
    await expect(flag).toHaveAttribute("data-locale", "en");
    await expect(flag).toHaveCSS("width", "24px");
    await expect(flag).toHaveCSS("height", "16px");
    await expect(page.locator(".hosted-app")).toHaveAttribute("lang", "en");
    await page.reload();
    await expect(flag).toHaveAttribute("data-locale", "en");
    await chooseLanguage(page, "ru", "Русский");
    await expect(page.getByRole("button", { name: "Настройки", exact: true, includeHidden: true })).toHaveCount(0);
    await picker(page).press("Space");
    const list = page.locator(".language-roller-list");
    await expect(list.getByRole("option")).toHaveCount(1);
    await expect(page.locator(".language-roller-rail").getByRole("button")).toHaveCount(2);
    expect(await page.locator(".language-roller-rail").evaluate(element => { const box = element.getBoundingClientRect(); return box.left >= 0 && box.right <= innerWidth; })).toBe(true);
    await page.keyboard.press("Escape");
    await expect(list).toBeHidden();
    await expect(list).toHaveAttribute("inert", "");
    await expect(picker(page)).toBeFocused();
    await picker(page).press("Tab");
    await expect(theme).toBeFocused();
  }
});

test("entry language popup leaves header controls in place and clickable", async ({ page }) => {
  await page.goto("/");
  test.skip(!await isDesktopMenu(page), "Native picker does not overlay the custom header.");
  for (const path of ["/login"]) {
    await page.goto(path);
    await expect(picker(page)).toBeVisible();
    const controls = page.locator(".landing-header :is(a, button, select):visible, .hosted-tools :is(button, select):visible, .hosted-entry-header :is(a, button, select):visible");
    await page.evaluate(() => document.fonts.ready);
    const before = await controls.evaluateAll(elements => elements.map(element => element.getBoundingClientRect().toJSON()));
    await picker(page).click();
    const menu = page.locator(".language-menu");
    await expect(menu).toBeVisible();
    expect(await controls.evaluateAll(elements => elements.map(element => element.getBoundingClientRect().toJSON()))).toEqual(before);
    expect(await controls.evaluateAll(elements => {
      const menu = document.querySelector(".language-menu")!.getBoundingClientRect();
      return elements.every(element => {
        const box = element.getBoundingClientRect();
        const hit = document.elementFromPoint(box.x + box.width / 2, box.y + box.height / 2);
        const overlaps = box.left < menu.right && box.right > menu.left && box.top < menu.bottom && box.bottom > menu.top;
        return !overlaps && !!hit && element.contains(hit);
      });
    })).toBe(true);
    await expect(picker(page)).toHaveCSS("border-top-color", "rgba(0, 0, 0, 0)");
    await expect(picker(page)).toHaveCSS("outline-style", "none");
    const theme = page.locator(".landing-header .key-icon, .hosted-tools > .icon-button").last();
    if (await theme.isVisible()) {
      const original = await page.locator(".relay-app").getAttribute("data-theme");
      await theme.click();
      await expect(page.locator(".relay-app")).toHaveAttribute("data-theme", original === "light" ? "dark" : "light");
    } else await page.keyboard.press("Escape");
    await expect(menu).toHaveCount(0);
  }
});

test("entry flag menu retains reduced-motion and high-contrast behavior", async ({ page }) => {
  await page.goto("/");
  test.skip(!await isDesktopMenu(page), "Touch and narrow screens use the native picker.");
  for (const path of ["/login"]) {
    await page.goto(path);
    await page.emulateMedia({ reducedMotion: "reduce", forcedColors: "none" });
    await picker(page).click();
    await expect(picker(page)).toHaveCSS("transition-duration", "0s");
    for (const option of await page.locator(".language-menu").getByRole("option").all()) await expect(option).toHaveCSS("transition-duration", "0s");
    await page.keyboard.press("Escape");
    await page.emulateMedia({ forcedColors: "active" });
    await picker(page).press("Space");
    await expect(page.locator(".language-menu")).toHaveCSS("border-top-width", "1px");
    await expect(page.locator(".language-menu")).toHaveCSS("box-shadow", "none");
    await expect(page.locator('.language-menu [data-active="true"]')).toHaveCSS("outline-style", "solid");
    await page.keyboard.press("Escape");
    await expect(picker(page)).toBeFocused();
  }
});
