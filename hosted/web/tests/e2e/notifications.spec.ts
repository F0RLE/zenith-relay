import { expect, test } from "@playwright/test";

test.beforeEach(async ({ page }) => { await page.goto("/preview.html?page=usage"); });

test("unread bell is filled without a dot, nudges rarely, and quiets when read", async ({ page }, testInfo) => {
  await page.emulateMedia({ reducedMotion: "no-preference" });
  const bell = page.getByRole("button", { name: "Уведомления: одно новое", exact: true });
  const icon = bell.locator("svg");
  await expect(bell).toHaveAttribute("data-unread", "true");
  await expect(icon).toHaveCSS("fill", await icon.evaluate(element => getComputedStyle(element).color));
  expect(await bell.evaluate(element => ["::before", "::after"].map(pseudo => getComputedStyle(element, pseudo).content)))
    .toEqual(["none", "none"]);
  await expect(icon).toHaveCSS("animation-name", "notification-nudge");
  await expect(icon).toHaveCSS("animation-duration", "45s");
  await expect(icon).toHaveCSS("animation-iteration-count", "infinite");
  const motion = await icon.evaluate(element => {
    const animation = element.getAnimations().find(item => item instanceof CSSAnimation && item.animationName === "notification-nudge")!;
    const trigger = element.parentElement!;
    const before = trigger.getBoundingClientRect().toJSON();
    animation.pause();
    const transforms = [135, 900, 22500, 44999].map(time => {
      animation.currentTime = time;
      return getComputedStyle(element).transform;
    });
    const after = trigger.getBoundingClientRect().toJSON();
    return { transforms, before, after };
  });
  expect(motion.transforms[0]).not.toBe("matrix(1, 0, 0, 1, 0, 0)");
  expect(motion.transforms.slice(1)).toEqual(Array(3).fill("matrix(1, 0, 0, 1, 0, 0)"));
  expect(motion.after).toEqual(motion.before);
  await page.locator(".hosted-tools").screenshot({ path: testInfo.outputPath("notification-bell-unread.png") });
  await bell.click();
  const panel = page.getByRole("dialog", { name: "Уведомления", exact: true });
  await panel.getByRole("button", { name: "Отметить прочитанным", exact: true }).click();
  const readBell = page.getByRole("button", { name: "Уведомления: нет новых", exact: true });
  await expect(readBell).toHaveAttribute("data-unread", "false");
  await expect(readBell.locator("svg")).toHaveCSS("fill", "none");
  await expect(readBell.locator("svg")).toHaveCSS("animation-name", "none");
  await page.keyboard.press("Escape");
  await expect(readBell).toBeFocused();
  await page.locator(".hosted-tools").screenshot({ path: testInfo.outputPath("notification-bell-read.png") });
});

test("reduced motion keeps the unread fill without animating the bell", async ({ page }) => {
  await page.emulateMedia({ reducedMotion: "reduce" });
  const bell = page.getByRole("button", { name: "Уведомления: одно новое", exact: true });
  const icon = bell.locator("svg");
  await expect(bell).toHaveAttribute("data-unread", "true");
  await expect(icon).toHaveCSS("fill", await icon.evaluate(element => getComputedStyle(element).color));
  await expect(icon).toHaveCSS("animation-name", "none");
  expect(await icon.evaluate(element => element.getAnimations().length)).toBe(0);
});

test("notifications are a compact anchored panel, not a blocking request dialog", async ({ page }, testInfo) => {
  const bell = page.getByRole("button", { name: /^Уведомления:/ });
  await bell.click();
  const panel = page.getByRole("dialog", { name: "Уведомления", exact: true });
  await expect(panel).toHaveClass("notification-panel");
  await expect(page.locator("dialog.request-dialog")).toHaveCount(0);
  expect(await page.evaluate(() => document.body.style.overflow)).not.toBe("hidden");
  await expect(bell).toHaveAttribute("aria-expanded", "true");
  await expect(panel.getByRole("button", { name: "Закрыть уведомления", exact: true })).toBeFocused();
  await expect(panel.locator(".notification-count")).toHaveText("1");
  await expect(panel.locator(".notification-count")).toHaveAttribute("aria-label", "1 новое");
  await expect(panel.locator(".notification-panel-actions").getByRole("button", { name: "Отметить прочитанным", exact: true })).toBeVisible();
  await expect(panel.locator(".notification-panel-footer")).toHaveCount(0);
  await expect(panel.getByRole("article")).toHaveCount(1);
  await expect(panel).toContainText("Дождитесь завершения расчёта");
  expect(await panel.evaluate(element => {
    const bounds = element.getBoundingClientRect();
    return bounds.width <= 384 && bounds.height < 320 && bounds.left >= 12 && bounds.top >= 12
      && bounds.right <= innerWidth - 12 && bounds.bottom <= innerHeight - 12
      && element.scrollWidth <= element.clientWidth;
  })).toBe(true);
  await page.screenshot({ path: testInfo.outputPath("notification-panel-unread.png") });
  await panel.getByRole("button", { name: "Закрыть уведомления", exact: true }).click();
  await expect(panel).toHaveCount(0);
  await expect(bell).toBeFocused();
  await bell.click();
  await bell.click();
  await expect(panel).toHaveCount(0);
  await bell.click();
  await page.setViewportSize({ width: page.viewportSize()!.width, height: 180 });
  await expect.poll(() => panel.evaluate(element => {
    const bounds = element.getBoundingClientRect();
    return bounds.top >= 12 && bounds.bottom <= innerHeight - 12;
  })).toBe(true);
  await expect(panel.getByRole("button", { name: "Закрыть уведомления", exact: true })).toBeInViewport();
  await panel.getByRole("button", { name: "Закрыть уведомления", exact: true }).click();
  await expect(panel).toHaveCount(0);
});

test("read state persists; outside clicks and Escape dismiss; request details keep focus ownership", async ({ page }, testInfo) => {
  const mobile = page.viewportSize()!.width < 640;
  if (mobile) await page.getByRole("button", { name: "Дополнительные фильтры", exact: true }).click();
  const bell = page.getByRole("button", { name: /^Уведомления:/ });
  const panel = page.getByRole("dialog", { name: "Уведомления", exact: true });
  await bell.click();
  await panel.getByRole("button", { name: "Отметить прочитанным", exact: true }).click();
  await expect(bell).toHaveAttribute("data-unread", "false");
  await expect(panel.locator(".notification-count")).toHaveCount(0);
  await expect(panel).not.toContainText("Нет новых");
  await expect(panel).not.toContainText("Всё прочитано");
  await expect(panel.locator(".notification-unread-dot")).toHaveCount(0);
  await expect(panel.getByRole("button", { name: "Отметить прочитанным", exact: true })).toHaveCount(0);
  await expect(panel.getByRole("button", { name: "Закрыть уведомления", exact: true })).toBeFocused();
  await page.screenshot({ path: testInfo.outputPath("notification-panel-read.png") });
  // Click an exposed control, not the right-hand filter covered by the popover.
  const keyFilter = mobile
    ? page.getByRole("searchbox", { name: "ID запроса или клиент", exact: true })
    : page.getByRole("combobox", { name: "Период, UTC", exact: true });
  await keyFilter.click();
  await expect(panel).toHaveCount(0);
  if (!mobile) await expect(keyFilter).toHaveAttribute("aria-expanded", "true");
  await expect(keyFilter).toBeFocused();
  await page.keyboard.press("Escape");
  await bell.click();
  await expect(panel.locator(".notification-count")).toHaveCount(0);
  await page.keyboard.press("Escape");
  await expect(panel).toHaveCount(0);
  await expect(bell).toBeFocused();
  await bell.click();
  await panel.getByRole("button", { name: "Открыть запрос", exact: true }).click();
  await expect(panel).toHaveCount(0);
  const details = page.getByRole("dialog", { name: "Сведения о запросе", exact: true });
  await expect(details).toContainText("Ответ завершён, расчёт ожидается");
  await page.keyboard.press("Escape");
  await expect(details).toHaveCount(0);
  await expect(bell).toBeFocused();
});
