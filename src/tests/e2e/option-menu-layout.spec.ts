import { expect, test, type Locator, type Page } from "../bun-playwright";
import { installTauriMock } from "./tauri-mock";

async function expectFieldWidth(page: Page, trigger: Locator) {
  await trigger.click();
  const list = page.getByRole("listbox");
  await expect(list).toHaveAttribute("data-positioned", "true");
  const anchor = (await trigger.boundingBox())!;
  const panel = (await list.boundingBox())!;
  expect(Math.abs(panel.width - anchor.width)).toBeLessThanOrEqual(1);
  expect(Math.abs(panel.x - anchor.x)).toBeLessThanOrEqual(1);
  expect(panel.y).toBeGreaterThanOrEqual(44);
  expect(panel.y + panel.height).toBeLessThanOrEqual(page.viewportSize()!.height - 8 + 1);
  expect(await list.evaluate((element) => element.scrollWidth <= element.clientWidth + 1
    && [...element.querySelectorAll<HTMLElement>("button, button > span")]
      .every((item) => item.scrollWidth <= item.clientWidth + 1))).toBe(true);
  return list;
}

for (const locale of ["ru", "en"] as const) {
  for (const theme of ["dark", "light"] as const) {
    for (const width of [1160, 390]) {
      test(`usage dropdowns follow their fields with long choices ${locale} ${theme} ${width}`, async ({ page }) => {
        const longModel = `model-${"long-name-".repeat(20)}synthetic`;
        const longMember = `member-${"long-name-".repeat(20)}synthetic`;
        const russian = locale === "ru";
        await page.setViewportSize({ width, height: 900 });
        await installTauriMock(page, {
          locale, theme, populated: true,
          usageFilterHistory: Array.from({ length: 24 }, (_, index) => ({
            wireApi: "responses", transport: "http", errorCategory: "upstream_failure",
            requestedModel: index === 0 ? longModel : `model-${index}`,
            resolvedModel: index === 0 ? longModel : `model-${index}`,
            accountId: index === 0 ? longMember : `member-${index}`, sourceId: null,
          })),
        });
        await page.goto("/");
        await page.getByRole("button", { name: russian ? "Использование" : "Usage", exact: true }).click();
        await page.getByRole("button", { name: russian ? "Другие фильтры" : "More filters", exact: true }).click();
        const controls = page.locator(".usage-scope-controls .relay-option-trigger, .usage-filters .relay-option-trigger");
        await expect(controls).toHaveCount(7);
        for (const trigger of await controls.all()) {
          await expectFieldWidth(page, trigger);
          if ((await trigger.getAttribute("aria-label"))?.startsWith(russian ? "Модель:" : "Model:")) {
            await page.mouse.move(0, 0);
            await page.screenshot({ path: `output/playwright/dropdown-usage-${locale}-${theme}-${width}.png`, animations: "disabled" });
          }
          await page.keyboard.press("Escape");
          await expect(trigger).toBeFocused();
          await expect(page.getByRole("listbox")).toHaveCount(0);
        }
        const model = page.locator(".usage-model-menu .relay-option-trigger");
        const list = await expectFieldWidth(page, model);
        await list.locator(`[data-value="${longModel}"]`).click();
        await expect(model).toHaveAttribute("data-value", longModel);
        await expect(page.locator(".usage-request-table tbody tr")).toHaveCount(1);
        const reopened = await expectFieldWidth(page, model);
        const selected = reopened.locator('[aria-selected="true"]');
        await expect(selected).toBeFocused();
        await selected.press("Home");
        await expect(reopened.getByRole("option").first()).toBeFocused();
        await reopened.getByRole("option").first().press("Enter");
        await expect(model).toHaveAttribute("data-value", "");
        await expect(model).toBeFocused();
        await expect(page.locator(".usage-request-table tbody tr")).toHaveCount(24);
      });
    }
  }
}

for (const width of [1160, 390]) {
  test(`shared dropdowns follow compact controls and dialog fields at ${width}`, async ({ page }) => {
    await page.setViewportSize({ width, height: 900 });
    await installTauriMock(page, { locale: "en", populated: true, accountCount: 3 });
    await page.goto("/");
    await page.getByRole("button", { name: "Connections", exact: true }).click();
    for (const trigger of await page.locator(".account-filter-menu .relay-option-trigger").all()) {
      await expectFieldWidth(page, trigger);
      await page.keyboard.press("Escape");
      await expect(trigger).toBeFocused();
    }
    await page.getByRole("tab", { name: "Sources", exact: true }).click();
    const order = page.locator(".sources-sort-controls .relay-option-trigger");
    await expectFieldWidth(page, order);
    await page.keyboard.press("Escape");
    await page.locator(".source-card").first().getByRole("button", { name: "Edit", exact: true }).click();
    const dialog = page.getByRole("dialog");
    await expectFieldWidth(page, dialog.getByRole("button", { name: /^Proxy:/ }));
    await page.keyboard.press("Escape");
    await expect(dialog).toBeVisible();
    await dialog.getByRole("button", { name: "Close", exact: true }).click();
    await page.getByRole("button", { name: "Settings", exact: true }).click();
    for (const trigger of await page.locator(".settings-page .relay-option-trigger").all()) {
      await expectFieldWidth(page, trigger);
      await page.keyboard.press("Escape");
      await expect(trigger).toBeFocused();
    }
  });
}
