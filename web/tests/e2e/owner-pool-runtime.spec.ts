import { expect, test } from "@playwright/test";
import type { OwnerPoolActivity } from "../../src/features/owner/poolActivity";

test("participant toolbar shows current account and model with matching card highlight", async ({ page }, info) => {
  await page.goto("/admin/pools");
  const toolbar = page.locator(".owner-pool-toolbar");
  await expect(toolbar.getByRole("heading", { name: "Распределение запросов", exact: true })).toBeVisible();
  await expect(toolbar).toContainText("Работает сейчас: Code account");
  await expect(toolbar).toContainText("В работе (1): demo-gpt");
  await expect(toolbar).not.toContainText("Следующий");
  await expect(page.locator(".owner-pool-next, .owner-resource-card[data-next]")).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Ротация", exact: true })).toHaveCount(0);
  await expect(page.getByRole("article", { name: "Code account", exact: true })).toHaveAttribute("data-current", "true");
  await expect(page.getByRole("article", { name: "Text account", exact: true })).toHaveAttribute("data-current", "false");
  expect(await toolbar.evaluate(node => node.scrollWidth <= node.clientWidth)).toBe(true);
  expect(await toolbar.locator("button, input").evaluateAll(nodes => nodes.filter(node => node.getClientRects().length).every(node => {
    const bounds = node.getBoundingClientRect(), parent = node.closest(".owner-pool-toolbar")!.getBoundingClientRect();
    return bounds.left >= parent.left && bounds.right <= parent.right;
  }))).toBe(true);
  if (page.viewportSize()!.width > 1000) {
    expect(await toolbar.locator(".owner-pool-route > strong").evaluate(node => node.scrollWidth <= node.clientWidth)).toBe(true);
  }
  const settings = toolbar.getByRole("button", { name: "Настройки ротации пула", exact: true });
  await expect(settings).toHaveAttribute("aria-haspopup", "dialog");
  await page.screenshot({ path: info.outputPath("participants-activity.png"), fullPage: true, animations: "disabled" });
  await settings.click();
  const dialog = page.getByRole("dialog", { name: "Настройки ротации пула", exact: true });
  const bounds = (await dialog.boundingBox())!;
  expect(bounds.y).toBeGreaterThanOrEqual(0);
  expect(bounds.x).toBeGreaterThanOrEqual(0);
  expect(bounds.x + bounds.width).toBeLessThanOrEqual(page.viewportSize()!.width);
  expect(bounds.y + bounds.height).toBeLessThanOrEqual(page.viewportSize()!.height);
  await expect(dialog.getByRole("region", { name: "Ротация пула", exact: true })).toBeVisible();
  await page.screenshot({ path: info.outputPath("rotation-dialog.png"), animations: "disabled" });
  await page.keyboard.press("Escape");
  await expect(settings).toBeFocused();
});

test("speed is an accessible preview control and does not simulate new traffic", async ({ page }) => {
  await page.goto("/admin/pools");
  const speed = page.getByRole("radiogroup", { name: "Скорость запросов", exact: true });
  const writes: string[] = [];
  page.on("request", request => { if (request.method() !== "GET") writes.push(request.method()); });
  await speed.getByRole("radio", { name: "Обычная", exact: true }).focus();
  await page.keyboard.press("ArrowRight");
  await expect(speed.getByRole("radio", { name: "Быстрая", exact: true })).toHaveAttribute("aria-checked", "true");
  await expect(speed.getByRole("radio", { name: "Быстрая", exact: true })).toBeFocused();
  await page.keyboard.press("End");
  await expect(speed.getByRole("radio", { name: "Ультрабыстрая", exact: true })).toHaveAttribute("aria-checked", "true");
  await page.getByRole("button", { name: "Настройки ротации пула", exact: true }).click();
  await page.keyboard.press("Escape");
  await expect(speed.getByRole("radio", { name: "Ультрабыстрая", exact: true })).toHaveAttribute("aria-checked", "true");
  await expect(page.locator('.owner-resource-card[data-current="true"]')).toHaveCount(1);
  await expect(page.locator(".owner-pool-active-models")).toHaveAttribute("data-active-request-count", "1");
  expect(writes).toEqual([]);
});

test("compact search retains filtering without hiding the global activity", async ({ page }) => {
  await page.goto("/admin/pools");
  const toolbar = page.locator(".owner-pool-toolbar");
  const toggle = toolbar.getByRole("button", { name: "Поиск сохранённых подключений", exact: true });
  await expect(toolbar.getByRole("searchbox")).toBeHidden();
  await toggle.click();
  await expect(toolbar.getByRole("searchbox")).toBeFocused();
  await toolbar.getByRole("searchbox").fill("Text account");
  await expect(page.locator(".owner-resource-grid:visible article")).toHaveCount(1);
  await expect(toolbar).toContainText("Работает сейчас: Code account");
  await toggle.click();
  await expect(page.locator(".owner-resource-grid:visible article")).toHaveCount(3);
});

for (const scenario of ["unknown", "idle", "multiple"] as const) {
  test(`activity ${scenario} never infers working members from saved pool configuration`, async ({ page }) => {
    const activity: OwnerPoolActivity | undefined = scenario === "unknown" ? undefined : scenario === "idle" ? { members: [], next: { id: "demo_account_1", kind: "account" } } : {
      members: [
        { id: "demo_account_1", kind: "account", activeRequests: 1, models: [{ id: "demo-gpt", requests: 1 }] },
        { id: "demo_provider_1", kind: "api", activeRequests: 2, models: [{ id: "runtime-model", requests: 2 }] },
      ],
    };
    await page.route("**/src/preview/snapshot.ts", async route => {
      const response = await route.fetch();
      await route.fulfill({ response, body: `${await response.text()}\npreviewSnapshot.owner.poolActivity = ${activity ? JSON.stringify(activity) : "undefined"};` });
    });
    await page.goto("/admin/pools");
    const runtime = page.locator(".owner-pool-runtime");
    await expect(runtime).toBeVisible();
    await expect(runtime).not.toContainText("Следующий");
    await expect(page.locator(".owner-pool-next, .owner-resource-card[data-next]")).toHaveCount(0);
    if (scenario === "multiple") {
      await expect(runtime).toContainText("Работает сейчас: Code account · Primary API");
      await expect(runtime).toContainText("В работе (3): demo-gpt · runtime-model × 2");
      await expect(page.locator('.owner-resource-card[data-current="true"]')).toHaveCount(2);
    } else {
      await expect(runtime).toContainText(scenario === "idle" ? "Ожидание запросов" : "Данные об активности пока не получены");
      await expect(page.locator('.owner-resource-card[data-current="true"]')).toHaveCount(0);
      await page.getByRole("checkbox", { name: "Выбрать: Code account", exact: true }).check();
      await expect(page.locator('.owner-resource-card[data-current="true"]')).toHaveCount(0);
    }
  });
}
