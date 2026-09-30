import { expect, test } from "../bun-playwright";
import { installTauriMock } from "./tauri-mock";

for (const mode of ["local", "remote"] as const) {
  for (const narrow of [false, true]) {
    test(`provider error details survive ${mode} usage at ${narrow ? "mobile" : "desktop"} width`, async ({ page }, testInfo) => {
      await page.setViewportSize(narrow ? { width: 390, height: 844 } : { width: 1160, height: 760 });
      const message = `Invalid field: temperature. ${"Synthetic diagnostic context. ".repeat(15)}[redacted]`;
      await installTauriMock(page, {
        mode, locale: "en", populated: true, usageFailure: true,
        usageUpstreamError: { httpStatus: 422, code: "future_validation_error", errorType: "invalid_request_error", message, redacted: true, truncated: true },
      });
      await page.goto("/");
      await page.getByRole("button", { name: "Usage", exact: true }).click();
      await page.getByRole("button", { name: `Request details: req_synthetic_${mode}` }).click();
      const dialog = page.getByRole("dialog", { name: "Request details" });
      await expect(dialog.getByRole("heading", { name: "Provider response" })).toBeVisible();
      await expect(dialog.getByText("future_validation_error", { exact: true })).toBeVisible();
      await expect(dialog.getByText("invalid_request_error", { exact: true })).toBeVisible();
      await expect(dialog.locator(".request-upstream-message pre")).toHaveText(message);
      await expect(dialog.locator(".request-details-list > div").filter({ has: page.getByText("Provider HTTP status", { exact: true }) }).locator("dd")).toHaveText("422");
      await dialog.getByRole("button", { name: "Copy provider message", exact: true }).click();
      await expect.poll(() => page.evaluate(() => navigator.clipboard.readText())).toBe(message);
      const messageFits = await dialog.locator(".request-upstream-message").evaluate((element) => {
        const box = element.getBoundingClientRect();
        return element.scrollWidth <= element.clientWidth + 1 && box.left >= 0 && box.right <= window.innerWidth;
      });
      expect(messageFits).toBe(true);
      expect(await dialog.locator(".request-provider-fields dd").evaluateAll((elements) =>
        elements.every((element) => element.scrollWidth <= element.clientWidth + 1),
      )).toBe(true);
      await page.screenshot({ path: testInfo.outputPath("provider-error.png") });
    });
  }
}

test("legacy failures show absence without inventing a provider message", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, usageFailure: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Usage", exact: true }).click();
  await page.getByRole("button", { name: "Request details: req_synthetic_local" }).click();
  const dialog = page.getByRole("dialog", { name: "Request details" });
  await expect(dialog.getByText("No provider message recorded", { exact: true })).toBeVisible();
  await expect(dialog.getByRole("button", { name: "Copy provider message" })).toHaveCount(0);
});

test("Russian provider diagnostics fit a narrow dialog", async ({ page }, testInfo) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await installTauriMock(page, {
    mode: "local", locale: "ru", populated: true, usageFailure: true,
    usageUpstreamError: { httpStatus: 400, code: "future_validation_error", errorType: "INVALID_ARGUMENT", message: "Invalid field: temperature", redacted: false, truncated: false },
  });
  await page.goto("/");
  await page.getByRole("button", { name: "Использование", exact: true }).click();
  await page.getByRole("button", { name: "Сведения о запросе: req_synthetic_local" }).click();
  const dialog = page.getByRole("dialog", { name: "Сведения о запросе" });
  await expect(dialog.getByRole("heading", { name: "Ответ провайдера" })).toBeVisible();
  await expect(dialog.getByText("future_validation_error", { exact: true })).toBeVisible();
  expect(await dialog.locator(".request-provider-fields dd").evaluateAll((elements) =>
    elements.every((element) => element.scrollWidth <= element.clientWidth + 1),
  )).toBe(true);
  await page.screenshot({ path: testInfo.outputPath("provider-error-ru.png") });
});
