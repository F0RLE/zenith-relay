import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { customerPages, ownerPages } from "../../src/features/app/routes";

async function expectConsistentIcons(page: Page) {
  await expect(page.locator("svg.lucide")).toHaveCount(0);
  const glyphs = await page.locator("svg.relay-icon:visible").evaluateAll(elements => elements.map(element => {
    const svg = element as SVGSVGElement, style = getComputedStyle(svg), geometry = svg.getBBox();
    return { name: svg.dataset.icon, width: parseFloat(style.width), height: parseFloat(style.height),
      stroke: style.strokeWidth, fill: style.fill, color: style.color, hidden: svg.getAttribute("aria-hidden"),
      unreadBell: svg.dataset.icon === "notifications" && !!svg.closest('.notification-button[data-unread="true"]'),
      noticeEmpty: !!svg.closest(".owner-notices-empty"),
      proxyEmpty: !!svg.closest(".owner-proxy-empty"),
      role: svg.getAttribute("role"), label: svg.getAttribute("aria-label"),
      vector: getComputedStyle(svg.firstElementChild!).vectorEffect,
      inside: geometry.width > 0 && geometry.height > 0 && geometry.x >= 0 && geometry.y >= 0 && geometry.x + geometry.width <= 20 && geometry.y + geometry.height <= 20 };
  }));
  expect(glyphs.length).toBeGreaterThan(0);
  for (const glyph of glyphs) {
    expect(glyph.height, glyph.name).toBe(glyph.width);
    if (glyph.noticeEmpty) expect(glyph.width, glyph.name).toBe(24);
    else if (glyph.proxyEmpty) expect(glyph.width, glyph.name).toBe(28);
    else expect([14, 16, 18, 20], glyph.name).toContain(glyph.width);
    expect(glyph.stroke, glyph.name).toBe("1.5px");
    expect(glyph.fill, glyph.name).toBe(glyph.unreadBell ? glyph.color : "none");
    if (glyph.label) {
      expect(glyph.role, glyph.name).toBe("img");
      expect(glyph.hidden, glyph.name).not.toBe("true");
    } else expect(glyph.hidden, glyph.name).toBe("true");
    expect(glyph.vector, glyph.name).toBe("non-scaling-stroke");
    expect(glyph.inside, glyph.name).toBe(true);
  }
}

test("all customer and owner pages use one icon system without changing page layout", async ({ page }) => {
  for (const [base, routes] of [["app", customerPages], ["admin", ownerPages]] as const) {
    for (const route of routes) {
      await page.goto(`/${base}/${route.id}`);
      await expect(page.getByRole("heading", { level: 1 })).toBeVisible();
      await expectConsistentIcons(page);
      expect(await page.locator(".relay-content").evaluate(element => element.scrollWidth <= element.clientWidth)).toBe(true);
    }
  }
});

test("request details and notification controls keep their semantics and actions", async ({ page }) => {
  await page.goto("/app/usage");
  await page.getByRole("button", { name: "Детали запроса demo_request_0003", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "Сведения о запросе", exact: true });
  await expect(dialog.getByRole("img", { name: "Завершён", exact: true })).toBeVisible();
  await expectConsistentIcons(page);
  await dialog.getByRole("button", { name: "Закрыть", exact: true }).click();
  const bell = page.getByRole("button", { name: /^Уведомления:/ });
  await expect(bell).toHaveAccessibleName("Уведомления: одно новое");
  expect(await bell.evaluate(element => getComputedStyle(element, "::after").content)).toBe("none");
  await expect(bell.locator("svg")).toHaveCSS("fill", await bell.evaluate(element => getComputedStyle(element).color));
  await bell.click();
  await expect(page.getByRole("dialog", { name: "Уведомления", exact: true })).toBeVisible();
  await expectConsistentIcons(page);
  await page.getByRole("button", { name: "Отметить прочитанным", exact: true }).click();
  await page.keyboard.press("Escape");
  await expect(bell).toHaveAccessibleName("Уведомления: нет новых");
  await expect(bell.locator("svg")).toHaveCSS("fill", "none");
  expect(await bell.evaluate(element => getComputedStyle(element, "::after").content)).toBe("none");
});

test("landing shares functional glyphs while its mark and woven artwork stay separate", async ({ page }) => {
  await page.goto("/");
  await expect(page.getByRole("heading", { level: 1 })).toBeVisible();
  await expectConsistentIcons(page);
  await expect(page.locator(".landing-mark")).toHaveAttribute("viewBox", "0 0 28 32");
  await expect(page.locator(".request-weave-art")).toHaveAttribute("viewBox", "0 -20 1200 360");
  const before = await page.locator(".landing").getAttribute("data-theme");
  await page.getByRole("button", { name: /^(Тёмная|Светлая) тема$/ }).click();
  await expect(page.locator(".landing")).not.toHaveAttribute("data-theme", before!);
  await expectConsistentIcons(page);
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
});
