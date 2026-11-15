import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";

const routes = ["/", "/app/overview", "/admin/overview"];
const toggle = (page: Page) => page.locator("[data-theme-toggle]");
const shell = (page: Page) => page.locator(".relay-app");
const settled = (page: Page) => expect(page.locator("html")).not.toHaveAttribute("data-theme-transition");

test("theme icon shows the current theme without shifting any header controls", async ({ page }) => {
  await page.addInitScript(() => localStorage.setItem("relay-hosted-theme", "light"));
  for (const route of routes) {
    await page.goto(route);
    await page.evaluate(() => document.fonts.ready);
    const button = toggle(page);
    await expect(button.locator("svg")).toHaveCount(1);
    await expect(button.locator("svg")).toHaveAttribute("data-current-theme", "light");
    await expect(button).toHaveAccessibleName("Тёмная тема");
    const before = await button.boundingBox();
    await expect(button.locator("svg")).toHaveCSS("width", "20px");
    await expect(button.locator("svg")).toHaveCSS("height", "20px");
    await button.click();
    await expect(shell(page)).toHaveAttribute("data-theme", "dark");
    await settled(page);
    await expect(button.locator("svg")).toHaveAttribute("data-current-theme", "dark");
    await expect(button.locator("svg")).toHaveCount(1);
    await expect(button).toHaveAccessibleName("Светлая тема");
    expect(await button.boundingBox()).toEqual(before);
    expect(await page.evaluate(() => localStorage.getItem("relay-hosted-theme"))).toBe("dark");
    await button.press("Enter");
    await expect(shell(page)).toHaveAttribute("data-theme", "light");
    await settled(page);
    await expect(button).toBeFocused();
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  }
});

test("theme reveal starts at the button and covers the viewport in both directions", async ({ page }) => {
  await page.emulateMedia({ reducedMotion: "no-preference" });
  for (const route of routes) {
    await page.goto(route);
    for (let step = 0; step < 2; step++) {
      // Measure at the click, after Playwright's actionability and layout waits.
      await toggle(page).evaluate(button => {
        button.addEventListener("click", () => {
          const { x, y, width, height } = button.getBoundingClientRect();
          button.setAttribute("data-test-click-box", JSON.stringify({ x, y, width, height }));
        }, { once: true });
      });
      await toggle(page).click();
      const box = await toggle(page).evaluate(button => JSON.parse(button.getAttribute("data-test-click-box")!) as {
        x: number; y: number; width: number; height: number;
      });
      await expect.poll(() => page.evaluate(() => document.getAnimations().some(animation =>
        (animation.effect as KeyframeEffect | null)?.pseudoElement === "::view-transition-new(root)"
      ))).toBe(true);
      const effect = await page.evaluate(() => {
        const animation = document.getAnimations().find(animation => (animation.effect as KeyframeEffect | null)?.pseudoElement === "::view-transition-new(root)")!;
        const effect = animation.effect as KeyframeEffect;
        return { frames: effect.getKeyframes().map(frame => frame.clipPath), duration: effect.getTiming().duration };
      });
      expect(effect.duration).toBe(450);
      // CSS serializes subpixel coordinates with fewer digits than DOMRect.
      const origin = /^circle\(0px at ([\d.]+)px ([\d.]+)px\)$/.exec(String(effect.frames[0]));
      expect(origin).not.toBeNull();
      expect(Number(origin![1])).toBeCloseTo(box.x + box.width / 2, 2);
      expect(Number(origin![2])).toBeCloseTo(box.y + box.height / 2, 2);
      const radius = parseFloat(String(effect.frames[1]).slice("circle(".length));
      const viewport = page.viewportSize()!;
      expect(radius).toBeGreaterThan(Math.hypot(Math.max(box.x + box.width / 2, viewport.width - box.x - box.width / 2), Math.max(box.y + box.height / 2, viewport.height - box.y - box.height / 2)) - 1);
      await settled(page);
    }
  }
});

test("rapid theme changes settle to the last choice without stale overlays", async ({ page }) => {
  const failures: string[] = [];
  page.on("pageerror", error => failures.push(error.message));
  await page.goto("/app/overview");
  const initial = await shell(page).getAttribute("data-theme");
  await toggle(page).click();
  await toggle(page).click();
  await toggle(page).click();
  const final = initial === "light" ? "dark" : "light";
  await expect(shell(page)).toHaveAttribute("data-theme", final);
  await settled(page);
  await expect(toggle(page)).toBeEnabled();
  expect(await page.evaluate(() => localStorage.getItem("relay-hosted-theme"))).toBe(final);
  expect(failures).toEqual([]);
  await page.reload();
  await expect(shell(page)).toHaveAttribute("data-theme", final);
});

test("reduced motion changes the theme without a circular reveal", async ({ page }) => {
  await page.emulateMedia({ reducedMotion: "reduce" });
  for (const route of routes) {
    await page.goto(route);
    const initial = await shell(page).getAttribute("data-theme");
    await toggle(page).click();
    await expect(shell(page)).not.toHaveAttribute("data-theme", initial!);
    await settled(page);
    expect(await page.evaluate(() => document.getAnimations().some(animation => (animation.effect as KeyframeEffect | null)?.pseudoElement?.startsWith("::view-transition")))).toBe(false);
    await expect(toggle(page).locator("svg")).toHaveCount(1);
  }
});

test("theme switching has a fallback when view transitions and storage are unavailable", async ({ page }) => {
  const failures: string[] = [];
  page.on("pageerror", error => failures.push(error.message));
  await page.addInitScript(() => {
    Object.defineProperty(document, "startViewTransition", { value: undefined, configurable: true });
    Object.defineProperty(window, "localStorage", { get() { throw new DOMException("Denied", "SecurityError"); } });
  });
  for (const route of routes) {
    await page.goto(route);
    const initial = await shell(page).getAttribute("data-theme");
    await toggle(page).click();
    await expect(shell(page)).not.toHaveAttribute("data-theme", initial!);
    await settled(page);
    await expect(page.locator("html")).not.toHaveAttribute("data-theme-fade");
  }
  expect(failures).toEqual([]);
});
