import { expect, test } from "../../bun-playwright";
import { installTauriMock } from "../tauri-mock";
import { chooseOption } from "./helpers";

for (const locale of ["en", "ru"] as const) {
  test(`weekly threshold stays opt-in and saves the typed IPC value (${locale})`, async ({ page }) => {
    await installTauriMock(page, { mode: "local", locale, theme: "dark", populated: true });
    await page.setViewportSize(locale === "ru" ? { width: 840, height: 560 } : { width: 1160, height: 760 });
    await page.goto("/");
    await page.getByRole("button", { name: locale === "en" ? "Connections" : "Подключения", exact: true }).click();
    await page.getByRole("tab", { name: locale === "en" ? "Automations" : "Автоматизация" }).click();
    await page.getByRole("button", { name: locale === "en" ? "Edit" : "Изменить", exact: true }).click();
    const dialog = page.getByRole("dialog");
    await chooseOption(page, dialog, locale === "en" ? "Automation type" : "Тип автоматизации", "weekly");
    const timing = locale === "en" ? "Reset timing" : "Когда сбрасывать";
    await expect(dialog.getByRole("spinbutton")).toHaveCount(0);
    await chooseOption(page, dialog, timing, "threshold");
    const threshold = dialog.getByRole("spinbutton");
    const save = dialog.getByRole("button", { name: locale === "en" ? "Save" : "Сохранить", exact: true });
    await threshold.fill("101");
    await expect(save).toBeDisabled();
    await threshold.fill("20");
    await expect(save).toBeEnabled();
    await expect(dialog.locator(".automation-trigger-note")).toContainText("20%");
    expect(await dialog.evaluate((element) => {
      const rect = element.getBoundingClientRect();
      return rect.left >= 0 && rect.right <= innerWidth && rect.bottom <= innerHeight
        && element.scrollWidth <= element.clientWidth;
    })).toBe(true);
    await page.screenshot({ path: `output/playwright/weekly-threshold-${locale}.png` });
    await save.click();
    const call = await page.evaluate(() => (window as unknown as {
      __TAURI_TEST_INVOKES__: Array<{ command: string; args: { input?: Record<string, unknown> } }>;
    }).__TAURI_TEST_INVOKES__.findLast((item) => item.command === "update_quota_wake_automation"));
    expect(call?.args.input).toMatchObject({
      trigger: { kind: "weekly" }, weeklyResetRemainingBasisPoints: 2_000,
    });
  });
}

test("successful HTTP envelopes do not turn a terminal failure into a successful receipt", async ({ page }) => {
  await installTauriMock(page, {
    mode: "local", locale: "en", theme: "dark", populated: true, usageFailure: true,
    diagnosticDebug: true,
    usageTransportDiagnostics: {
      upstream: "http", mode: "native", responseFormat: "sse",
      upstreamHttpStatus: 200, downstreamHttpStatus: 200, failureKind: "upstream",
    },
  });
  await page.goto("/");
  await page.getByRole("button", { name: "Usage", exact: true }).click();
  await page.locator(".usage-request-table .request-link").first().click();
  const dialog = page.getByRole("dialog");
  await expect(dialog.locator(".request-details-status")).toHaveAttribute("data-status", "error");
  await expect(dialog.getByRole("region", { name: "Error", exact: true })).toContainText("502");
  await dialog.getByText("Request parameters and IDs", { exact: true }).click();
  await expect(dialog.locator(".request-id-list")).not.toContainText("200");
  await page.screenshot({ path: "output/playwright/http-failure-evidence-en.png" });
});
