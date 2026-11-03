import { expect, test } from "@playwright/test";

test("landing opens the cabinet directly without a guide or real requests", async ({ page }, testInfo) => {
  const external: string[] = [], errors: string[] = [];
  page.on("request", request => { if (!request.url().startsWith("http://127.0.0.1:1442/")) external.push(request.url()); });
  page.on("pageerror", error => errors.push(error.message));
  await page.goto("/");
  await expect(page.getByRole("heading", { level: 1 })).toHaveText("Нейросети. В ваших приложениях.");
  await expect(page.getByRole("img", { name: /^Схема подключения:/ })).toBeVisible();
  await expect(page.locator(".landing")).not.toContainText(/демо|образец|в разработке|прототип/i);
  await expect(page.getByRole("link", { name: "Начать", exact: true })).toHaveAttribute("href", "/app/overview");
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  await page.screenshot({ path: testInfo.outputPath("landing.png") });
  await page.getByRole("link", { name: "Начать", exact: true }).click();
  await expect(page.getByRole("heading", { name: "Обзор", exact: true })).toBeVisible();
  await expect(page.getByRole("dialog")).toHaveCount(0);
  await expect(page.locator("body")).not.toHaveCSS("overflow", "hidden");
  await expect(page.getByRole("button", { name: "Баланс: 42,80 USD", exact: true })).toBeVisible();
  await expect(page).not.toHaveURL(/welcome=/);
  await page.reload();
  await expect(page.getByRole("heading", { name: "Обзор", exact: true })).toBeVisible();
  await expect(page.getByRole("dialog")).toHaveCount(0);
  await page.goto("/"); await page.getByRole("link", { name: "Начать", exact: true }).click();
  await expect(page.getByRole("heading", { name: "Обзор", exact: true })).toBeVisible();
  await expect(page.getByRole("dialog")).toHaveCount(0);
  expect(errors).toEqual([]); expect(external).toEqual([]);
});

test("removed guide cannot be enabled by legacy links", async ({ page }) => {
  for (const path of ["/app/overview?welcome=1", "/preview.html?welcome=1", "/admin/overview?welcome=1"]) {
    await page.goto(path);
    await expect(page.getByRole("heading", { name: "Обзор", exact: true })).toBeVisible();
    await expect(page.getByRole("dialog")).toHaveCount(0);
    await expect(page.locator(".getting-started, .welcome-dialog")).toHaveCount(0);
    await expect(page.locator("body")).not.toHaveCSS("overflow", "hidden");
    expect(await page.evaluate(() => sessionStorage.getItem("relay-hosted-preview:welcome-dismissed"))).toBeNull();
  }
});

test("disabled session storage does not prevent direct entry", async ({ page }) => {
  await page.addInitScript(() => {
    Object.defineProperty(window, "sessionStorage", { get() { throw new DOMException("Storage denied", "SecurityError"); } });
  });
  await page.goto("/preview.html?welcome=1");
  await expect(page.getByRole("heading", { name: "Обзор", exact: true })).toBeVisible();
  await expect(page.getByRole("dialog")).toHaveCount(0);
  await page.reload();
  await expect(page.getByRole("heading", { name: "Обзор", exact: true })).toBeVisible();
  await expect(page.getByRole("dialog")).toHaveCount(0);
});

test("unintegrated sign-in routes never invent a customer or owner session", async ({ page }) => {
  for (const path of ["/login", "/admin/login"]) {
    await page.goto(path);
    await expect(page.getByText("Вход, баланс и использование требуют подключения к серверу. Пока страница работает локально.", { exact: true })).toBeVisible();
    await expect(page.locator(".relay-shell")).toHaveCount(0);
    await expect(page.getByRole("link", { name: "На главную", exact: true })).toHaveAttribute("href", "/");
  }
});

test("questions have visible answers and preserve theme on direct entry", async ({ page }) => {
  await page.goto("/#features");
  const originalTheme = await page.locator(".relay-app").evaluate(element => getComputedStyle(element).colorScheme);
  const nextTheme = originalTheme === "dark" ? "light" : "dark";
  const faq = page.locator(".landing-questions article");
  await expect(faq).toHaveCount(4);
  await expect(page.locator(".landing-questions details")).toHaveCount(0);
  for (const answer of await faq.locator("p").all()) await expect(answer).toBeVisible();
  await expect(faq.first()).toContainText("Через Relay можно подключать нейросети");
  await page.getByRole("button", { name: /^(Тёмная|Светлая) тема$/ }).click();
  await expect(page.locator(".relay-app")).toHaveCSS("color-scheme", nextTheme);
  await expect(page.getByRole("link", { name: "В кабинет", exact: true })).toHaveAttribute("href", "/app/overview");
  await page.getByRole("link", { name: "В кабинет", exact: true }).click();
  await expect(page.getByRole("heading", { name: "Обзор", exact: true })).toBeVisible();
  await expect(page.getByRole("dialog")).toHaveCount(0);
  await expect(page.locator(".relay-app")).toHaveCSS("color-scheme", nextTheme);
  await page.reload(); await expect(page.locator(".relay-app")).toHaveCSS("color-scheme", nextTheme);
  await page.goto("/#faq");
  await expect(page.getByRole("link", { name: "Перейти в кабинет", exact: true })).toHaveAttribute("href", "/app/overview");
  await page.getByRole("link", { name: "Перейти в кабинет", exact: true }).click();
  await expect(page.getByRole("heading", { name: "Обзор", exact: true })).toBeVisible();
  await expect(page.getByRole("dialog")).toHaveCount(0);
  await page.goto("/#faq");
  if (page.viewportSize()!.width >= 1000) {
    for (const width of [541, 640, 800]) {
      await page.setViewportSize({ width, height: 900 });
      expect(await page.locator(".landing-questions h3").evaluateAll(elements => elements.every(element => element.scrollWidth <= element.clientWidth))).toBe(true);
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    }
  }
});

test("landing theme and direct entry work without browser storage", async ({ page }) => {
  await page.addInitScript(() => {
    Object.defineProperty(window, "localStorage", { get() { throw new DOMException("Storage denied", "SecurityError"); } });
  });
  await page.goto("/");
  const originalTheme = await page.locator(".relay-app").evaluate(element => getComputedStyle(element).colorScheme);
  await page.getByRole("button", { name: /^(Dark|Light) theme$/ }).click();
  await expect(page.locator(".relay-app")).not.toHaveCSS("color-scheme", originalTheme);
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  await page.getByRole("link", { name: "Get started", exact: true }).click();
  await expect(page.getByRole("heading", { name: "Overview", exact: true })).toBeVisible();
  await expect(page.getByRole("dialog")).toHaveCount(0);
});

test("woven diagram retains all geometry and is still under reduced motion", async ({ page }) => {
  await page.emulateMedia({ reducedMotion: "reduce" });
  await page.goto("/");
  await expect(page.locator(".weave-thread")).toHaveCount(66);
  await expect(page.locator(".weave-request")).toHaveCount(3);
  await expect(page.locator(".request-weave")).toHaveAttribute("data-motion", "paused");
  for (const path of await page.locator(".weave-request").all()) await expect(path).toHaveCSS("animation-name", "none");
  const before = await page.locator(".weave-request").evaluateAll(elements => elements.map(element => getComputedStyle(element).strokeDashoffset));
  await page.waitForTimeout(160);
  expect(await page.locator(".weave-request").evaluateAll(elements => elements.map(element => getComputedStyle(element).strokeDashoffset))).toEqual(before);
  await expect(page.locator(".weave-labels")).toHaveText("Ваше приложениеRelayМодель");
  await expect(page.locator(".ascii-flow, canvas")).toHaveCount(0);
});

test("woven strands fit vertically without clipping at short and wide sizes", async ({ page }) => {
  await page.emulateMedia({ reducedMotion: "reduce" });
  await page.goto("/");
  await page.evaluate(() => document.fonts.ready);
  const viewport = page.viewportSize()!;
  const sizes = [viewport, { width: viewport.width, height: viewport.width <= 360 ? 568 : 600 }];
  if (viewport.width > 1000) sizes.push({ width: 2560, height: 1440 });
  for (const size of sizes) {
    await page.setViewportSize(size);
    const envelope = await page.locator(".request-weave-art").evaluate(element => {
      const art = element.getBoundingClientRect();
      const stage = element.parentElement!.getBoundingClientRect();
      const strokes = Array.from(element.querySelectorAll("path"), path => {
        const rect = path.getBoundingClientRect();
        const matrix = path.getScreenCTM()!;
        const halfStroke = parseFloat(getComputedStyle(path).strokeWidth) * Math.max(Math.abs(matrix.a), Math.abs(matrix.d)) / 2;
        return { top: rect.top - halfStroke, bottom: rect.bottom + halfStroke };
      });
      const labels = document.querySelector(".weave-labels")!.getBoundingClientRect();
      return {
        artTop: art.top, artBottom: art.bottom, stageTop: stage.top,
        top: Math.min(...strokes.map(stroke => stroke.top)),
        bottom: Math.max(...strokes.map(stroke => stroke.bottom)),
        labelsTop: labels.top,
      };
    });
    expect(envelope.top).toBeGreaterThan(envelope.artTop);
    expect(envelope.top).toBeGreaterThan(envelope.stageTop);
    expect(envelope.bottom).toBeLessThan(envelope.artBottom);
    expect(envelope.artBottom).toBeLessThanOrEqual(envelope.labelsTop - 8);
  }
});

test("request strokes travel linearly without theme resets and pause offscreen", async ({ page }) => {
  await page.setViewportSize({ width: page.viewportSize()!.width, height: 600 });
  await page.emulateMedia({ reducedMotion: "no-preference" });
  await page.goto("/");
  const diagram = page.locator(".request-weave"), packet = page.locator(".weave-request").first();
  await expect(diagram).toHaveAttribute("data-motion", "running");
  await expect(packet).toHaveCSS("animation-timing-function", "linear");
  const offset = await packet.evaluate(element => getComputedStyle(element).strokeDashoffset);
  await expect.poll(() => packet.evaluate(element => getComputedStyle(element).strokeDashoffset)).not.toBe(offset);
  await diagram.hover();
  const time = await packet.evaluate(element => Number(element.getAnimations()[0]!.currentTime));
  await page.getByRole("button", { name: /^(Тёмная|Светлая) тема$/ }).click();
  expect(await packet.evaluate(element => Number(element.getAnimations()[0]!.currentTime))).toBeGreaterThan(time);
  await page.locator(".faq-start").scrollIntoViewIfNeeded();
  await expect.poll(() => diagram.evaluate(element => element.getBoundingClientRect().bottom)).toBeLessThanOrEqual(0);
  await expect(diagram).toHaveAttribute("data-motion", "paused");
  await expect(packet).toHaveCSS("animation-play-state", "paused");
  const paused = await packet.evaluate(async element => {
    // CSS pause has a pending frame; sample only after the browser commits it.
    await Promise.all(element.getAnimations().map(animation => animation.ready));
    return getComputedStyle(element).strokeDashoffset;
  });
  await page.waitForTimeout(180);
  expect(await packet.evaluate(element => getComputedStyle(element).strokeDashoffset)).toBe(paused);
  await page.getByRole("link", { name: "Наверх", exact: true }).click();
  await expect(diagram).toHaveAttribute("data-motion", "running");
  await page.emulateMedia({ reducedMotion: "reduce" });
  await expect(diagram).toHaveAttribute("data-motion", "paused");
  await expect(packet).toHaveCSS("animation-name", "none");
});

test("compact landing keeps open answers and native scrolling with working anchors", async ({ page }) => {
  await page.goto("/");
  await expect(page.locator(".landing-main > section")).toHaveCount(2);
  await expect(page.locator(".landing :is(table, pre, [role=tablist], dialog)")).toHaveCount(0);
  await expect(page.locator(".landing")).not.toContainText(/YOUR_API_KEY|your-relay\.example|MODEL_ID|\$|USD/);
  await expect(page.locator("html")).toHaveCSS("scroll-snap-type", "none");
  await page.goto("/#faq");
  await expect(page).toHaveURL(/#faq$/);
  const title = page.getByRole("heading", { name: "Вопросы", exact: true });
  await expect(title).toBeInViewport();
  const titleBox = (await title.boundingBox())!, header = (await page.locator(".landing-header").boundingBox())!;
  expect(titleBox.y).toBeGreaterThanOrEqual(header.y + header.height);
  await page.getByRole("link", { name: "Наверх", exact: true }).click();
  await expect(page.getByRole("heading", { level: 1 })).toBeInViewport();
});

test("landing actions have keyboard and press feedback without motion dependence", async ({ page }) => {
  await page.goto("/");
  const start = page.getByRole("link", { name: "Начать", exact: true });
  await page.keyboard.press("Tab");
  await expect(page.getByRole("link", { name: "К содержимому", exact: true })).toBeFocused();
  await start.focus();
  await expect(start).toHaveCSS("outline-style", "none");
  await start.hover(); await page.mouse.down();
  await expect(start).toHaveCSS("transform", "matrix(1, 0, 0, 1, 0, 1)");
  await page.mouse.move(0, 0); await page.mouse.up();
  await page.emulateMedia({ reducedMotion: "reduce" });
  await start.hover();
  await expect(start).toHaveCSS("transition-duration", "0s");
  await page.mouse.down(); await expect(start).toHaveCSS("transform", "none");
  await page.mouse.move(0, 0); await page.mouse.up();
  await expect(page.locator(".landing")).toBeVisible();
});

test("header controls keep keyboard navigation in both themes without focus rings", async ({ page }) => {
  await page.goto("/");
  for (const theme of ["light", "dark"]) {
    await page.evaluate(value => localStorage.setItem("relay-hosted-theme", value), theme);
    await page.reload();
    await page.keyboard.press("Tab");
    await expect(page.getByRole("link", { name: "К содержимому", exact: true })).toBeFocused();
    for (const control of await page.locator(".landing-header :is(a, button, select):visible").all()) {
      await page.keyboard.press("Tab");
      await expect(control).toBeFocused();
      await expect(control).toHaveCSS("outline-style", "none");
      await expect(control).toHaveCSS("outline-width", "0px");
    }
    const toggle = page.locator(".landing-header .key-icon");
    await toggle.click();
    await expect(toggle).toHaveCSS("outline-style", "none");
  }
});

test("landing controls and content fit with readable type at narrow widths", async ({ page }) => {
  await page.goto("/");
  await page.evaluate(() => document.fonts.ready);
  await expect(page.getByRole("heading", { level: 1 })).toHaveCSS("font-family", /Golos Text/);
  const touchSized = await page.evaluate(() => matchMedia("(max-width: 700px), (pointer: coarse)").matches);
  for (const control of await page.locator(".landing a:visible, .landing button:visible, .landing select:visible").all()) {
    if (await control.evaluate(element => element.classList.contains("skip-link"))) continue;
    const box = (await control.boundingBox())!;
    const headerAction = await control.evaluate(element => !!element.closest(".landing-header-actions"));
    expect(box.height).toBeGreaterThanOrEqual(headerAction && !touchSized ? 40 : 44);
    expect(box.x).toBeGreaterThanOrEqual(0);
    expect(box.x + box.width).toBeLessThanOrEqual(page.viewportSize()!.width);
  }
  expect(await page.locator(".landing :is(h1, h2, h3, p)").evaluateAll(elements => elements.every(element => element.scrollWidth <= element.clientWidth))).toBe(true);
});

test("landing offers direct model pricing without replacing the main entry", async ({ page }) => {
  await page.goto("/");
  const theme = await page.locator(".landing-home").getAttribute("data-theme");
  await expect(page.locator(".landing-copy p")).toHaveText("Модели, API-ключи и расходы — в одном кабинете.");
  await expect(page.getByRole("link", { name: "Начать", exact: true })).toHaveAttribute("href", "/app/overview");
  const models = page.getByRole("link", { name: "Модели и цены", exact: true });
  await expect(models).toHaveAttribute("href", "/app/models");
  await page.keyboard.press("Tab");
  await models.focus();
  await expect(models).toHaveCSS("text-decoration-line", "underline");
  await models.press("Enter");
  await expect(page).toHaveURL(/\/app\/models$/);
  await expect(page.getByRole("heading", { name: "Модели и цены", level: 1, exact: true })).toBeVisible();
  await expect(page.locator(".hosted-app")).toHaveAttribute("data-theme", theme!);
  await expect(page.getByRole("dialog")).toHaveCount(0);
});

test("short landing keeps actions and flow labels visible in every locale", async ({ page }, testInfo) => {
  const width = page.viewportSize()!.width;
  const height = width <= 360 ? 568 : 720;
  await page.setViewportSize({ width, height });
  await page.goto("/");
  for (const [locale, label] of [["en", "Models & pricing"], ["ru", "Модели и цены"], ["zh-CN", "模型与价格"]]) {
    await page.evaluate(value => localStorage.setItem("relay-hosted-locale", value), locale!);
    await page.reload();
    await page.evaluate(() => document.fonts.ready);
    await expect(page.getByRole("link", { name: label!, exact: true })).toBeInViewport();
    const header = (await page.locator(".landing-header-inner").boundingBox())!;
    for (const control of await page.locator(".landing-header :is(a, button, select):visible").all()) {
      const box = (await control.boundingBox())!;
      expect(box.x).toBeGreaterThanOrEqual(header.x - 1);
      expect(box.x + box.width).toBeLessThanOrEqual(header.x + header.width + 1);
    }
    const geometry = await page.evaluate(() => {
      const actions = document.querySelector(".landing-actions")!.getBoundingClientRect();
      const labels = document.querySelector(".weave-labels")!.getBoundingClientRect();
      const weave = document.querySelector(".request-weave")!.getBoundingClientRect();
      const art = document.querySelector(".request-weave-art")!.getBoundingClientRect();
      return { actionsBottom: actions.bottom, labelsTop: labels.top, labelsBottom: labels.bottom, weaveTop: weave.top, artBottom: art.bottom, clientWidth: document.documentElement.clientWidth, scrollWidth: document.documentElement.scrollWidth };
    });
    expect(geometry.scrollWidth).toBeLessThanOrEqual(geometry.clientWidth);
    expect(geometry.weaveTop).toBeGreaterThanOrEqual(geometry.actionsBottom);
    expect(geometry.labelsTop).toBeGreaterThan(geometry.weaveTop);
    expect(geometry.artBottom).toBeLessThanOrEqual(geometry.labelsTop - 8);
    expect(geometry.labelsBottom).toBeLessThanOrEqual(height);
    for (const link of await page.locator(".landing-actions a").all()) {
      const box = (await link.boundingBox())!;
      expect(box.height).toBeGreaterThanOrEqual(44);
      expect(box.x).toBeGreaterThanOrEqual(0);
      expect(box.x + box.width).toBeLessThanOrEqual(width);
    }
    await page.screenshot({ path: testInfo.outputPath(`landing-short-${locale}.png`) });
  }
});

test("hero edges align with the logo and FAQ without widening the reading column", async ({ page }, testInfo) => {
  await page.goto("/");
  await page.evaluate(() => document.fonts.ready);
  const widths = testInfo.project.name.startsWith("desktop")
    ? [768, 1024, 1440, 1920, 2560]
    : [page.viewportSize()!.width];
  for (const width of widths) {
    await page.setViewportSize({ width, height: 1440 });
    const layout = await page.evaluate(() => {
      const rect = (selector: string) => {
        const { x, right, width } = document.querySelector(selector)!.getBoundingClientRect();
        return { x, right, width };
      };
      return {
        header: rect(".landing-header-inner"),
        logo: rect(".landing-header .landing-brand"),
        cabinet: rect(".landing-header-actions > a"),
        headerFrame: rect(".landing-header"),
        mainFrame: rect(".landing-main"),
        footerFrame: rect(".landing-footer"),
        hero: rect(".landing-hero"),
        copy: rect(".landing-copy"),
        labels: rect(".weave-labels"),
        faq: rect(".landing-faq"),
        footer: rect(".footer-bottom"),
        clientWidth: document.documentElement.clientWidth,
        scrollWidth: document.documentElement.scrollWidth,
      };
    });
    expect(layout.scrollWidth).toBeLessThanOrEqual(layout.clientWidth);
    const gutter = width <= 360 ? 16 : width <= 700 ? 20 : width <= 1000 ? 32 : 56;
    expect(Math.abs(layout.hero.width - Math.min(1328, layout.clientWidth - 2 * gutter))).toBeLessThanOrEqual(1);
    expect(Math.abs(layout.hero.x - (layout.clientWidth - layout.hero.right))).toBeLessThanOrEqual(1);
    for (const frame of [layout.headerFrame, layout.mainFrame, layout.footerFrame, layout.header, layout.faq, layout.footer]) {
      expect(Math.abs(frame.x - layout.hero.x)).toBeLessThanOrEqual(1);
      expect(Math.abs(frame.right - layout.hero.right)).toBeLessThanOrEqual(1);
    }
    expect(Math.abs(layout.logo.x - layout.hero.x)).toBeLessThanOrEqual(1);
    expect(Math.abs(layout.cabinet.right - layout.hero.right)).toBeLessThanOrEqual(1);
    expect(layout.copy.x).toBeGreaterThan(layout.hero.x);
    expect(layout.copy.right).toBeLessThan(layout.hero.right);
    expect(Math.abs(layout.labels.x - layout.copy.x)).toBeLessThanOrEqual(1);
    expect(Math.abs(layout.labels.right - layout.copy.right)).toBeLessThanOrEqual(1);
  }
});

test("landing scrollbar follows the selected theme rather than system preference", async ({ page }) => {
  await page.goto("/");
  const palettes: Record<string, string> = {
    light: "rgb(129, 144, 135) rgb(244, 243, 238)",
    dark: "rgb(118, 126, 120) rgb(20, 20, 20)",
  };
  for (const theme of ["light", "dark"]) {
    if (await page.locator(".landing-home").getAttribute("data-theme") !== theme) {
      await page.getByRole("button", { name: /^(Тёмная|Светлая) тема$/ }).click();
    }
    await expect(page.locator("html")).toHaveCSS("color-scheme", theme);
    await expect(page.locator("html")).toHaveCSS("scrollbar-color", palettes[theme]!);
  }
  await page.getByRole("link", { name: "Начать", exact: true }).click();
  await expect(page.locator(".landing-home")).toHaveCount(0);
  await expect(page.locator("html")).toHaveCSS("color-scheme", "light dark");
});
