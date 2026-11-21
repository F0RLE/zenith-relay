import { expect, test } from "@playwright/test";
import type { Locator, Page } from "@playwright/test";

async function openRotation(page: Page) {
  await page.goto("/admin/pools");
  await page.getByRole("button", { name: "Настройки ротации пула", exact: true }).click();
  return page.getByRole("region", { name: "Ротация пула", exact: true });
}

async function memberOrder(panel: Locator) {
  return panel.getByRole("listitem").evaluateAll(rows => rows.map(row => row.getAttribute("data-member-id")));
}

test("rotation retains Desktop composition without clipped controls in any strategy", async ({ page }, info) => {
  const panel = await openRotation(page);
  await expect(panel.getByRole("radio")).toHaveCount(3);
  await expect(panel.getByRole("listitem")).toHaveCount(3);
  await expect(panel.getByRole("button", { name: "Рекомендуемый", exact: true })).toHaveAttribute("aria-pressed", "true");
  for (const [mode, screenshot] of [["Автоматически", "automatic"], ["По кругу", "weighted"], ["Основной и резерв", "failover"]] as const) {
    await panel.getByRole("radio", { name: mode, exact: true }).click();
    if (screenshot === "weighted") {
      await panel.getByRole("switch").check();
      const box = (await panel.getByRole("switch").boundingBox())!;
      expect(box.width).toBe(36);
      expect(box.height).toBe(22);
      expect(await panel.getByRole("switch").evaluate(el => getComputedStyle(el, "::before").content)).toBe("none");
    }
    const box = (await panel.boundingBox())!;
    expect(box.x).toBeGreaterThanOrEqual(0);
    expect(box.x + box.width).toBeLessThanOrEqual(page.viewportSize()!.width);
    expect(await panel.locator(".owner-rotation-editor, .owner-rotation-preset-options, .owner-rotation-modes, .owner-rotation-member").evaluateAll(nodes => nodes.every(node => node.scrollWidth <= node.clientWidth + 1))).toBe(true);
    expect(await panel.locator("button, input").evaluateAll(nodes => nodes.every(node => {
      const box = node.getBoundingClientRect(); const parent = node.closest(".owner-rotation-member, .owner-rotation-modes, .owner-rotation-presets, .owner-rotation-shares")!.getBoundingClientRect();
      return box.left >= parent.left - 1 && box.right <= parent.right + 1 && box.top >= parent.top - 1 && box.bottom <= parent.bottom + 1;
    }))).toBe(true);
    expect(await panel.locator("input[placeholder]").evaluateAll(inputs => inputs.every(input => {
      const style = getComputedStyle(input); const canvas = document.createElement("canvas").getContext("2d")!;
      canvas.font = `${style.fontSize} ${style.fontFamily}`;
      return canvas.measureText(input.getAttribute("placeholder")!).width + parseFloat(style.paddingLeft) + parseFloat(style.paddingRight) <= input.clientWidth + 1;
    }))).toBe(true);
    if (box.width > 722) expect((await panel.locator(".owner-rotation-presets").boundingBox())!.width).toBe(196);
    await page.screenshot({ path: info.outputPath(`pool-rotation-${screenshot}.png`), fullPage: true, animations: "disabled" });
  }
});

test("request-share switch keeps its thumb inside the track in both states", async ({ page }) => {
  const panel = await openRotation(page);
  await panel.getByRole("radio", { name: "По кругу", exact: true }).click();
  const toggle = panel.getByRole("switch");
  const weight = panel.getByRole("spinbutton", { name: "Доля запросов: Code account", exact: true });
  const expectThumb = async (checked: boolean) => {
    await expect(toggle).toBeChecked({ checked });
    await expect.poll(() => toggle.evaluate(node => {
      const track = getComputedStyle(node);
      const thumb = getComputedStyle(node, "::after");
      const offset = thumb.transform === "none" ? { m41: 0, m42: 0 } : new DOMMatrixReadOnly(thumb.transform);
      const box = node.getBoundingClientRect();
      const left = parseFloat(track.borderLeftWidth) + parseFloat(thumb.left) + offset.m41;
      const top = parseFloat(track.borderTopWidth) + parseFloat(thumb.top) + offset.m42;
      return {
        position: thumb.position,
        left,
        right: box.width - left - parseFloat(thumb.width),
        top,
        bottom: box.height - top - parseFloat(thumb.height),
      };
    })).toEqual({ position: "absolute", left: checked ? 18 : 4, right: checked ? 4 : 18, top: 4, bottom: 4 });
    await expect(weight).toHaveCount(checked ? 1 : 0);
  };
  await expectThumb(false);
  await toggle.click();
  await expectThumb(true);
  await toggle.focus();
  await page.keyboard.press("Space");
  await expectThumb(false);
  await panel.locator(".owner-rotation-shares > span").click();
  await expectThumb(true);
});

test("keyboard mode changes and numeric controls retain preview edits without save buttons", async ({ page }) => {
  const panel = await openRotation(page);
  const automatic = panel.getByRole("radio", { name: "Автоматически", exact: true });
  const round = panel.getByRole("radio", { name: "По кругу", exact: true });
  await automatic.focus();
  await page.keyboard.press("ArrowRight");
  await expect(round).toBeFocused();
  await expect(round).toHaveAttribute("aria-checked", "true");
  await expect(panel.getByRole("spinbutton", { name: "Доля запросов: Code account", exact: true })).toHaveCount(0);
  await panel.getByRole("switch").check();
  await panel.getByRole("spinbutton", { name: "Доля запросов: Code account", exact: true }).fill("8");
  const limit = panel.getByRole("spinbutton", { name: "Одновременных запросов: Code account", exact: true });
  await limit.fill("4");
  await round.focus();
  await page.keyboard.press("End");
  await expect(panel.getByRole("radio", { name: "Основной и резерв", exact: true })).toBeFocused();
  await page.keyboard.press("Home");
  await expect(automatic).toBeFocused();
  await expect(panel.getByRole("spinbutton", { name: "Доля запросов: Code account", exact: true })).toHaveValue("8");
  await page.keyboard.press("Escape");
  await page.getByRole("button", { name: "Настройки ротации пула", exact: true }).click();
  await expect(limit).toHaveValue("4");
  await limit.fill("");
  await expect(limit).toHaveAttribute("aria-valuetext", "Без лимита");
  await expect(panel.getByRole("button", { name: "Сохранить", exact: true })).toHaveCount(0);
  await expect(panel.getByRole("button", { name: "Отменить", exact: true })).toHaveCount(0);
});

test("local presets restore defaults and preserve accounts-before-API semantics", async ({ page }) => {
  const panel = await openRotation(page);
  await panel.getByRole("spinbutton", { name: "Доля запросов: Code account", exact: true }).fill("7");
  await expect(panel.locator(".owner-rotation-custom")).toHaveText("Свой");
  await panel.getByRole("button", { name: "Аккаунты → API", exact: true }).click();
  await expect(panel.getByRole("radio", { name: "Основной и резерв", exact: true })).toHaveAttribute("aria-checked", "true");
  expect((await memberOrder(panel)).map(key => key!.split(":")[0])).toEqual(["account", "account", "source"]);
  await panel.getByRole("button", { name: "Равномерно", exact: true }).click();
  await expect(panel.getByRole("switch")).not.toBeChecked();
  await panel.getByRole("button", { name: "Рекомендуемый", exact: true }).click();
  expect(await panel.locator('input[type="number"]').evaluateAll(inputs => inputs.map(input => (input as HTMLInputElement).value))).toEqual(["1", "", "1", "", "1", ""]);
});

test("arrow and pointer reordering stay synchronized with participant cards", async ({ page }) => {
  const panel = await openRotation(page);
  await panel.getByRole("radio", { name: "По кругу", exact: true }).click();
  const original = await memberOrder(panel);
  await panel.getByRole("button", { name: "Переместить Code account вниз", exact: true }).click();
  expect(await memberOrder(panel)).toEqual([original[1], original[0], original[2]]);
  await page.keyboard.press("Escape");
  await expect(page.locator(".owner-resource-card:visible").first()).toContainText("Text account");
  await page.getByRole("button", { name: "Настройки ротации пула", exact: true }).click();
  await panel.getByRole("listitem").nth(1).scrollIntoViewIfNeeded();
  const handle = (await panel.getByRole("button", { name: "Переместить Text account", exact: true }).boundingBox())!;
  const target = (await panel.getByRole("listitem").nth(1).locator(".owner-rotation-identity").boundingBox())!;
  await page.mouse.move(handle.x + handle.width / 2, handle.y + handle.height / 2);
  await page.mouse.down();
  await page.mouse.move(target.x + target.width / 2, target.y + target.height / 2, { steps: 12 });
  await page.mouse.up();
  expect(await memberOrder(panel)).toEqual(original);
  await expect(panel.locator("[data-dragging], [data-drop-target]")).toHaveCount(0);
  await expect(panel.getByRole("button", { name: "Переместить Code account вверх", exact: true })).toBeDisabled();
});

test.describe("touch input", () => {
  test.use({ hasTouch: true });
  test("touch drag reorders while swiping a member still scrolls normally", async ({ page }) => {
    const panel = await openRotation(page);
    await panel.getByRole("radio", { name: "По кругу", exact: true }).click();
    const original = await memberOrder(panel);
    const dragHandle = panel.getByRole("button", { name: "Переместить Code account", exact: true });
    // The workspace header is sticky; center the handle, not the target row,
    // so the native gesture starts on the control rather than behind the header.
    await dragHandle.evaluate(el => el.scrollIntoView({ block: "center", behavior: "instant" }));
    const handle = (await dragHandle.boundingBox())!;
    const target = (await panel.getByRole("listitem").nth(1).locator(".owner-rotation-identity").boundingBox())!;
    const cdp = await page.context().newCDPSession(page);
    const start = { x: handle.x + handle.width / 2, y: handle.y + handle.height / 2 };
    const end = { x: target.x + target.width / 2, y: target.y + target.height / 2 };
    expect(await page.evaluate(point => document.elementFromPoint(point.x, point.y)?.closest("button")?.classList.contains("owner-rotation-handle"), start)).toBe(true);
    await cdp.send("Input.dispatchTouchEvent", { type: "touchStart", touchPoints: [{ ...start, id: 1 }] });
    for (let step = 1; step <= 10; step++) await cdp.send("Input.dispatchTouchEvent", { type: "touchMove", touchPoints: [{ x: start.x + (end.x - start.x) * step / 10, y: start.y + (end.y - start.y) * step / 10, id: 1 }] });
    await cdp.send("Input.dispatchTouchEvent", { type: "touchEnd", touchPoints: [] });
    await expect.poll(() => memberOrder(panel)).toEqual([original[1], original[0], original[2]]);
    await expect(panel.locator("[data-dragging], [data-drop-target]")).toHaveCount(0);
    if (page.viewportSize()!.width <= 390) {
      const scrolled = () => panel.evaluate(el => {
        for (let parent: HTMLElement | null = el as HTMLElement; parent; parent = parent.parentElement) {
          if (parent.scrollHeight > parent.clientHeight && /auto|scroll/.test(getComputedStyle(parent).overflowY)) return parent.scrollTop;
        }
        return 0;
      });
      await panel.getByRole("listitem").first().locator("strong").evaluate(el => el.scrollIntoView({ block: "center", behavior: "instant" }));
      const before = await scrolled();
      expect(before).toBeGreaterThan(0);
      const name = (await panel.getByRole("listitem").first().locator("strong").boundingBox())!;
      const point = { x: name.x + name.width / 2, y: name.y + name.height / 2 };
      await cdp.send("Input.dispatchTouchEvent", { type: "touchStart", touchPoints: [{ ...point, id: 2 }] });
      for (let step = 1; step <= 8; step++) await cdp.send("Input.dispatchTouchEvent", { type: "touchMove", touchPoints: [{ x: point.x, y: point.y + step * 12, id: 2 }] });
      await cdp.send("Input.dispatchTouchEvent", { type: "touchEnd", touchPoints: [] });
      await expect.poll(scrolled).toBeLessThan(before);
      expect(await memberOrder(panel)).toEqual([original[1], original[0], original[2]]);
    }
    await cdp.detach();
  });
});

test("membership removal and re-addition do not leave stale rotation rows or controls", async ({ page }) => {
  const panel = await openRotation(page);
  await panel.getByRole("spinbutton", { name: "Доля запросов: Code account", exact: true }).fill("9");
  await page.keyboard.press("Escape");
  await page.getByRole("checkbox", { name: "Выбрать все показанные подключения" }).check();
  await page.locator(".owner-resource-bulk:visible").getByRole("button", { name: "Убрать из пула", exact: true }).click();
  await page.getByRole("button", { name: "Настройки ротации пула", exact: true }).click();
  await expect(panel.getByRole("listitem")).toHaveCount(0);
  await expect(panel.getByRole("status")).toBeVisible();
  await page.keyboard.press("Escape");
  await page.getByRole("button", { name: "Добавить участника", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "Добавить подключения в пул", exact: true });
  await dialog.locator(".owner-pool-picker-option").filter({ hasText: "Code account" }).getByRole("checkbox").check();
  await dialog.getByRole("button", { name: "Добавить в пул", exact: true }).click();
  await page.getByRole("button", { name: "Настройки ротации пула", exact: true }).click();
  await expect(panel.getByRole("listitem")).toHaveCount(1);
  await expect(panel.getByRole("spinbutton", { name: "Доля запросов: Code account", exact: true })).toHaveValue("1");
});

test("English and Chinese labels retain the same accessible responsive controls", async ({ page }) => {
  for (const [locale, tab, automatic, round] of [["en", "Pool rotation settings", "Automatic", "Round robin"], ["zh-CN", "资源池轮换设置", "自动", "轮询"]] as const) {
    await page.goto("/admin/pools");
    await page.evaluate(value => localStorage.setItem("relay-hosted-locale", value), locale);
    await page.reload();
    await page.getByRole("button", { name: tab, exact: true }).click();
    const panel = page.locator(".owner-pool-rotation");
    await expect(panel.getByRole("radio", { name: automatic, exact: true })).toBeVisible();
    await panel.getByRole("radio", { name: round, exact: true }).click();
    await panel.getByRole("switch").check();
    expect(await panel.evaluate(el => el.scrollWidth <= el.clientWidth + 1)).toBe(true);
    await expect(panel).not.toContainText("pool.rotation.");
  }
});
