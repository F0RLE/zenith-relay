import { test, expect } from "../bun-playwright";
import { installTauriMock } from "./tauri-mock";

for (const { locale, theme } of [
  { locale: "en", theme: "light" }, { locale: "ru", theme: "light" },
  { locale: "en", theme: "dark" }, { locale: "ru", theme: "dark" },
] as const) {
  test(`request diagnostics exports only the selected identifier through native IPC (${locale}, ${theme})`, async ({ page }, testInfo) => {
    await installTauriMock(page, { mode: "local", locale, theme, populated: true });
    await page.goto("/");
    await page.getByRole("button", { name: locale === "ru" ? "Использование" : "Usage", exact: true }).click();
    await page.getByRole("button", { name: locale === "ru" ? /^Сведения о запросе:/ : /^Request details:/ }).first().click();
    const details = page.getByRole("dialog", { name: locale === "ru" ? "Сведения о запросе" : "Request details" });
    const save = details.getByRole("button", { name: locale === "ru" ? "Сохранить диагностику" : "Save diagnostics", exact: true });
    await expect(details.locator(".request-details-identity").getByRole("button", {
      name: locale === "ru" ? "Сохранить диагностику" : "Save diagnostics", exact: true,
    })).toBeVisible();
    await expect(details.locator(".request-details-model-label")).toHaveCount(0);
    await expect(details.locator("footer")).toHaveCount(0);
    const modelBounds = await details.locator(".request-details-model > h3").boundingBox();
    const saveBounds = await save.boundingBox();
    const identityBounds = await details.locator(".request-details-identity").boundingBox();
    expect(saveBounds!.x).toBeGreaterThanOrEqual(modelBounds!.x + modelBounds!.width);
    expect(Math.abs(saveBounds!.x + saveBounds!.width - (identityBounds!.x + identityBounds!.width))).toBeLessThan(1);
    await expect(save).toHaveCSS("border-radius", "6px");
    await expect(details.locator('.request-details-status[data-status="success"]')).toHaveText("");
    await expect(details.locator('.request-details-status[data-status="success"]')).toHaveAccessibleName(locale === "ru" ? "Успешно" : "Success");
    await expect(save.locator(".request-details-export-label")).toHaveText(locale === "ru" ? "Диагностика" : "Diagnostics");
    expect(Math.abs(saveBounds!.y - modelBounds!.y)).toBeLessThan(10);
    await details.locator(".request-details-identity").screenshot({ path: `output/playwright/request-details-header-${locale}-${theme}.png` });
    await details.screenshot({ path: testInfo.outputPath(`request-diagnostics-${locale}-desktop.png`) });
    await save.click();
    const calls = await page.evaluate(() => (window as unknown as {
      __TAURI_TEST_INVOKES__: Array<{ command: string; args: Record<string, unknown> }>;
    }).__TAURI_TEST_INVOKES__.filter((call) => call.command === "export_request_diagnostics"));
    expect(calls).toHaveLength(1);
    expect(Object.keys(calls[0].args)).toEqual(["requestId"]);
    expect(calls[0].args.requestId).toBe("req_synthetic_local");
    await page.setViewportSize({ width: 390, height: 800 });
    await expect(save).toBeVisible();
    expect(await details.evaluate((element) => element.scrollWidth <= element.clientWidth + 1)).toBe(true);
    await page.screenshot({ path: testInfo.outputPath(`request-diagnostics-${locale}-mobile.png`) });
  });
}
