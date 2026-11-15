import { expect, test } from "@playwright/test";

test.beforeEach(async ({ page }) => { await page.goto("/app/keys"); });

test("language navigation replaces only bell and theme in the same header row without a popup", async ({ page }, testInfo) => {
  const header = page.locator(".hosted-tools");
  const picker = header.getByRole("combobox");
  const bell = header.locator(".notification-button");
  const theme = header.locator('[data-language-companion]').last();
  const balance = header.locator(".hosted-balance");
  const before = { header: await header.boundingBox(), bell: await bell.boundingBox(), theme: await theme.boundingBox(), balance: await balance.boundingBox() };
  await picker.click();
  const list = header.getByRole("listbox", { name: "Язык", exact: true });
  const rail = header.locator(".language-roller-rail");
  await expect(list).toBeVisible();
  await expect(list.getByRole("option")).toHaveCount(1);
  await expect(rail.getByRole("button", { name: /^Предыдущий язык:/ })).toBeVisible();
  await expect(rail.getByRole("button", { name: /^Следующий язык:/ })).toBeVisible();
  await expect(bell).toBeHidden();
  await expect(theme).toBeHidden();
  await expect(bell).toHaveAttribute("inert", "");
  await expect(theme).toHaveAttribute("inert", "");
  await expect(page.locator(".language-menu")).toHaveCount(0);
  await expect(rail).not.toHaveCSS("background-color", "rgba(0, 0, 0, 0)");
  await expect(rail).toHaveCSS("box-shadow", "none");
  await expect(rail).toHaveCSS("border-top-width", "0px");
  expect(await header.boundingBox()).toEqual(before.header);
  expect(await balance.boundingBox()).toEqual(before.balance);
  const box = (await rail.boundingBox())!;
  expect(box.x).toBeCloseTo(before.bell!.x, 1);
  expect(box.x + box.width).toBeCloseTo(before.theme!.x + before.theme!.width, 1);
  expect(box.y).toBeCloseTo(before.bell!.y, 1);
  expect(box.height).toBe(before.bell!.height);
  await rail.getByRole("button", { name: /^Следующий язык:/ }).click();
  await expect(list.getByRole("option", { name: "English", exact: true })).toHaveAttribute("aria-selected", "true");
  await page.screenshot({ path: testInfo.outputPath("header-inline-flags.png") });
  await list.getByRole("option", { name: "English", exact: true }).click();
  await expect(list).toBeHidden();
  await expect(bell).toBeVisible();
  await expect(theme).toBeVisible();
  await expect(picker).toBeFocused();
  await expect(picker.locator(".language-flag")).toHaveAttribute("data-locale", "en");
  expect(await header.boundingBox()).toEqual(before.header);
  await expect(page.locator(".hosted-app")).toHaveAttribute("lang", "en");
  await page.reload();
  await expect(picker.locator(".language-flag")).toHaveAttribute("data-locale", "en");
});

test("wheel previews flags in both directions without committing or scrolling the page", async ({ page }) => {
  const picker = page.locator(".language-roller-trigger");
  const content = page.locator(".relay-content");
  await picker.click();
  const list = page.locator(".language-roller-list");
  const scrollTop = await content.evaluate(element => element.scrollTop);
  await list.hover();
  await page.mouse.wheel(0, 100);
  await expect(list.getByRole("option", { name: "English", exact: true })).toHaveAttribute("aria-selected", "true");
  await expect(page.locator("html")).toHaveAttribute("lang", "ru");
  expect(await content.evaluate(element => element.scrollTop)).toBe(scrollTop);
  await page.mouse.wheel(0, -100);
  await expect(list.getByRole("option", { name: "Русский", exact: true })).toHaveAttribute("aria-selected", "true");
  await page.mouse.wheel(100, 0);
  await expect(list.getByRole("option", { name: "English", exact: true })).toHaveAttribute("aria-selected", "true");
  await list.dispatchEvent("wheel", { deltaY: 100, ctrlKey: true });
  await expect(list.getByRole("option", { name: "English", exact: true })).toHaveAttribute("aria-selected", "true");
  await list.dispatchEvent("wheel", { deltaY: 3, deltaMode: 1 });
  await expect(list.getByRole("option", { name: "简体中文", exact: true })).toHaveAttribute("aria-selected", "true");
  await page.keyboard.press("Escape");
  await expect(picker).toBeFocused();
  await expect(picker.locator(".language-flag")).toHaveAttribute("data-locale", "ru");
  await picker.click();
  await list.hover();
  await page.mouse.wheel(0, 100);
  await expect(list.locator('[data-active="true"]')).toHaveAttribute("aria-label", "English");
  await list.locator('[data-active="true"]').click();
  await expect(page.locator("html")).toHaveAttribute("lang", "en");
});

test("keyboard selection, Tab and outside dismissal restore utilities without changing unread state", async ({ page }) => {
  const picker = page.locator(".language-roller-trigger");
  const bell = page.locator(".notification-button");
  const theme = page.locator('.hosted-tools [data-language-companion]').last();
  const unread = await bell.getAttribute("data-unread");
  const appearance = await page.locator(".hosted-app").getAttribute("data-theme");
  await bell.click();
  await expect(page.locator("#notifications-panel")).toBeVisible();
  await picker.click();
  await expect(page.locator("#notifications-panel")).toHaveCount(0);
  const list = page.locator(".language-roller-list");
  await page.keyboard.press("End");
  await expect(list.getByRole("option", { name: "简体中文", exact: true })).toHaveAttribute("aria-selected", "true");
  await expect(list.locator('[data-active="true"]')).toHaveCSS("outline-style", "none");
  await page.keyboard.press("ArrowRight");
  await expect(list.getByRole("option", { name: "Русский", exact: true })).toHaveAttribute("aria-selected", "true");
  await page.keyboard.press("e");
  await expect(list.getByRole("option", { name: "English", exact: true })).toHaveAttribute("aria-selected", "true");
  await page.keyboard.press("Enter");
  await expect(picker).toBeFocused();
  await expect(page.locator("html")).toHaveAttribute("lang", "en");
  await picker.press("Space");
  await page.keyboard.press("Tab");
  await expect(list).toBeHidden();
  await expect(theme).toBeFocused();
  await picker.press("Space");
  await page.keyboard.press("Shift+Tab");
  await expect(bell).toBeFocused();
  await picker.click();
  await page.getByRole("heading", { name: "API keys", exact: true }).click();
  await expect(list).toBeHidden();
  await expect(bell).toHaveAttribute("data-unread", unread!);
  await expect(page.locator(".hosted-app")).toHaveAttribute("data-theme", appearance!);
  await picker.click();
  await page.locator("#hosted-content").focus();
  await expect(list).toBeHidden();
  await expect(page.locator("#hosted-content")).toBeFocused();
});

test("inline navigation retains reduced motion, high contrast and stable geometry after resizing", async ({ page }) => {
  await page.emulateMedia({ reducedMotion: "reduce", forcedColors: "active" });
  const picker = page.locator(".language-roller-trigger");
  await picker.press("Space");
  const list = page.locator(".language-roller-list");
  await expect(list.locator('[data-active="true"]')).toHaveCSS("outline-width", "1px");
  for (const arrow of await page.locator(".language-roller-step").all()) {
    await expect(arrow).toHaveCSS("transition-duration", "0s");
    expect(await arrow.evaluate(el => getComputedStyle(el).color !== getComputedStyle(el.parentElement!).backgroundColor)).toBe(true);
  }
  for (const flag of await list.locator(".language-flag").all()) {
    expect(await flag.evaluate(el => getComputedStyle(el).transitionDuration.split(",").every(value => parseFloat(value) <= .08))).toBe(true);
    await expect(flag).toHaveCSS("transform", "none");
    await expect(flag).toHaveCSS("forced-color-adjust", "none");
  }
  for (const width of [320, 390, 768, 1440]) {
    await page.setViewportSize({ width, height: 844 });
    await expect(list).toBeVisible();
    expect(await page.locator(".language-roller-rail").evaluate(element => {
      const rect = element.getBoundingClientRect(), header = element.closest(".hosted-tools")!.getBoundingClientRect();
      return rect.left >= 0 && rect.right <= innerWidth && rect.top >= header.top && rect.bottom <= header.bottom;
    })).toBe(true);
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  }
  await page.keyboard.press("Escape");
  await expect(picker).toBeFocused();
});

test("rolling preserves flag identity and closing keeps an inert exit transition", async ({ page }) => {
  const picker = page.locator(".language-roller-trigger");
  const header = page.locator(".hosted-tools");
  await expect(picker).toBeVisible();
  const before = await header.boundingBox();
  await picker.click();
  const list = page.locator(".language-roller-list");
  const next = list.locator('[role="option"][aria-label="English"]').last();
  const stableId = await next.getAttribute("id");
  await page.keyboard.press("ArrowRight");
  const centered = list.locator('[data-active="true"]');
  await expect(centered).toHaveAttribute("id", stableId!);
  await expect(centered).toHaveAttribute("aria-label", "English");
  await expect(centered).toHaveCSS("transition-property", "transform");
  const rail = page.locator(".language-roller-rail");
  await expect(rail).toHaveCSS("transition-property", "clip-path, opacity, visibility");
  await page.keyboard.press("Escape");
  await expect(list).toHaveAttribute("inert", "");
  await expect(list).toHaveAttribute("aria-hidden", "true");
  await expect(rail).toHaveAttribute("inert", "");
  await expect(rail).toHaveAttribute("aria-hidden", "true");
  await expect(list).toHaveCSS("pointer-events", "none");
  await expect(list).toBeHidden();
  await expect(page.getByRole("listbox")).toHaveCount(0);
  expect(await header.boundingBox()).toEqual(before);
});

test("rapid reversals and reopen settle on the committed flag without changing header size", async ({ page }) => {
  const picker = page.locator(".language-roller-trigger");
  const header = page.locator(".hosted-tools");
  await expect(picker).toBeVisible();
  const before = await header.boundingBox();
  await picker.click();
  const list = page.locator(".language-roller-list");
  for (let index = 0; index < 7; index++) await page.keyboard.press("ArrowLeft");
  for (let index = 0; index < 8; index++) await page.keyboard.press("ArrowRight");
  await expect(list.locator('[data-active="true"]')).toHaveAttribute("aria-label", "English");
  await page.keyboard.press("Escape");
  await picker.press("Space");
  await expect(list.locator('[data-active="true"]')).toHaveAttribute("aria-label", "Русский");
  await page.keyboard.press("ArrowLeft");
  await page.keyboard.press("Enter");
  await picker.press("Space");
  await expect(list.locator('[data-active="true"]')).toHaveAttribute("aria-label", "简体中文");
  await expect(list.getByRole("option")).toHaveCount(1);
  expect(await list.evaluate(el => {
    const ids = [...el.querySelectorAll('[id]')].map(option => option.id);
    return ids.length === new Set(ids).size;
  })).toBe(true);
  expect(await header.boundingBox()).toEqual(before);
});

test("touch users can browse and confirm languages with forty-four pixel targets", async ({ browser }, testInfo) => {
  test.skip(testInfo.project.name !== "desktop-light", "One dedicated touch session covers both mobile and tablet.");
  const context = await browser.newContext({ viewport: { width: 320, height: 740 }, hasTouch: true, baseURL: testInfo.project.use.baseURL! });
  try {
    const page = await context.newPage();
    for (const width of [320, 390, 1024]) {
      await page.setViewportSize({ width, height: 844 });
      await page.goto("/admin/overview");
      const picker = page.locator(".language-roller-trigger");
      const committed = await picker.locator(".language-flag").getAttribute("data-locale");
      const choices = ["ru", "en", "zh-CN"];
      const nextLocale = choices[(choices.indexOf(committed!) + 1) % choices.length];
      await picker.tap();
      const list = page.locator(".language-roller-list");
      for (const control of await page.locator('.language-roller-step, .language-roller-list [data-active="true"]').all()) {
        await expect(control).toHaveCSS("width", "44px");
        await expect(control).toHaveCSS("height", "44px");
      }
      await page.locator('.language-roller-step[data-direction="next"]').tap();
      await expect(list.getByRole("option").locator(".language-flag")).toHaveAttribute("data-locale", nextLocale!);
      await expect(page.locator("html")).toHaveAttribute("lang", committed!);
      await list.getByRole("option").tap();
      await expect(picker.locator(".language-flag")).toHaveAttribute("data-locale", nextLocale!);
      await expect(page.locator(".notification-button")).toBeVisible();
    }
  } finally { await context.close(); }
});

test("visible arrows preview both directions and retain wheel browsing across the whole rail", async ({ page }) => {
  await page.locator(".language-roller-trigger").click();
  const rail = page.locator(".language-roller-rail");
  const list = rail.getByRole("listbox");
  const previous = rail.locator('[data-direction="previous"]');
  const next = rail.locator('[data-direction="next"]');
  await previous.click();
  await expect(list.getByRole("option")).toHaveAttribute("aria-label", "简体中文");
  await next.click();
  await expect(list.getByRole("option")).toHaveAttribute("aria-label", "Русский");
  await next.focus();
  await next.press("Enter");
  await expect(list.getByRole("option")).toHaveAttribute("aria-label", "English");
  await previous.focus();
  await previous.press("Space");
  await expect(list.getByRole("option")).toHaveAttribute("aria-label", "Русский");
  await next.hover();
  await page.mouse.wheel(0, 100);
  await expect(list.getByRole("option")).toHaveAttribute("aria-label", "English");
  await expect(page.locator("html")).toHaveAttribute("lang", "ru");
  await expect(list).toBeFocused();
  await expect(async () => {
    expect(await list.evaluate(el => {
      const viewport = el.getBoundingClientRect();
      return [...el.querySelectorAll('[data-outside] .language-flag')].every(flag => {
        const bounds = flag.getBoundingClientRect();
        return bounds.right <= viewport.left || bounds.left >= viewport.right;
      });
    })).toBe(true);
  }).toPass({ timeout: 3_000 });
  await page.keyboard.press("Escape");
  await expect(rail.getByRole("button")).toHaveCount(0);
  await expect(page.locator(".language-roller-trigger .language-flag")).toHaveAttribute("data-locale", "ru");
});
