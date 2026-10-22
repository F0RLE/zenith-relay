import { expect, test } from "../bun-playwright";
import { emitTauriEvent, installTauriMock } from "./tauri-mock";

for (const theme of ["dark", "light"] as const) {
  test(`cache remaining preview ${theme}`, async ({ page }) => {
    await page.setViewportSize({ width: 1280, height: 980 });
    await installTauriMock(page, { mode: "local", locale: "ru", theme, populated: true, accountCount: 2, cachePreview: true });
    await page.goto("/");
    await page.getByRole("button", { name: "Использование", exact: true }).click();
    await expect(page.getByRole("tab", { name: "Кэш", exact: true })).toHaveCount(0);
    await page.getByRole("button", { name: "Сведения о запросе: req_synthetic_local" }).click();
    const dialog = page.getByRole("dialog");
    await expect(dialog.getByRole("tab")).toHaveCount(0);
    await expect(dialog.getByText("ещё ≈ 18 мин")).toBeVisible();
    await expect(dialog.locator(".request-details-token-group").first().locator(".request-cache-retention")).toContainText("Хранение кэша (оценка)");
    await expect(dialog.locator(".request-details-token-group").last().locator(".request-cache-retention")).toHaveCount(0);
    await expect(dialog.locator(".request-details-token-child dd > small")).toHaveCount(0);
    await dialog.screenshot({ path: `output/cache-remaining-${theme}.png` });
  });
}

for (const mode of ["local"] as const) {
  for (const locale of ["en", "ru"] as const) {
    test(`cache reads and zero writes stay separate from retention: ${mode} ${locale}`, async ({ page }) => {
      await page.setViewportSize({ width: 390, height: 760 });
      await installTauriMock(page, { mode, locale, populated: true,
        usageCache: { cachedInputTokens: 10, cacheWriteInputTokens: 0, cacheWriteTtl: "30m" } });
      await page.goto("/");
      await page.getByRole("button", { name: locale === "ru" ? "Использование" : "Usage", exact: true }).click();
      await page.getByRole("button", { name: `${locale === "ru" ? "Сведения о запросе" : "Request details"}: req_synthetic_${mode}` }).click();
      const dialog = page.getByRole("dialog");
      const reads = dialog.locator(".request-details-token-child").filter({ has: page.getByText(locale === "ru" ? "Прочитано из кэша" : "Cache reads", { exact: true }) });
      const writes = dialog.locator(".request-details-token-child").filter({ has: page.getByText(locale === "ru" ? "Записано в кэш" : "Cache writes", { exact: true }) });
      await expect(reads.locator(".request-token-count")).toHaveText("10");
      await expect(writes.locator(".request-token-count")).toHaveText("0");
      await expect(reads.locator(".request-token-cost")).toHaveText("—");
      await expect(writes.locator(".request-token-cost")).toHaveText("—");
      await expect(dialog.locator(".request-cache-retention")).toContainText(locale === "ru" ? "ещё ≈ 30 мин" : "≈ 30 min left");
      expect(await dialog.locator(".relay-dialog-body, .request-details-list dd").evaluateAll((elements) =>
        elements.every((element) => element.scrollWidth <= element.clientWidth + 1))).toBe(true);
    });
  }
}

test("GPT model names do not invent a private provider retention window", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, cachePreview: true,
    cacheSessions: [], usageCache: { cacheWriteInputTokens: null } });
  await page.goto("/");
  await page.getByRole("button", { name: "Usage", exact: true }).click();
  await page.getByRole("button", { name: "Request details: req_synthetic_local" }).click();
  const dialog = page.getByRole("dialog");
  await expect(dialog.locator(".request-cache-retention")).toHaveCount(0);
  await expect(dialog.getByText("window not reported", { exact: true })).toHaveCount(0);
  const writes = dialog.locator(".request-details-token-child").filter({ has: page.getByText("Cache writes", { exact: true }) });
  await expect(writes.locator(".request-token-count")).toHaveText("—");
  await expect(writes.locator(".request-token-cost")).toHaveText("—");
});

test("mixed prefix windows do not become one longest-lived cache", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, usageCache: { cacheWriteTtl: "5m, 1h" } });
  await page.goto("/");
  await page.getByRole("button", { name: "Usage", exact: true }).click();
  await page.getByRole("button", { name: "Request details: req_synthetic_local" }).click();
  await expect(page.locator(".request-cache-retention dd")).toHaveText("separate windows: 5m, 1h");
});

test("a cache miss retains the prior deadline and a new observed hit refreshes the open dialog", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, cachePreview: true,
    usageCache: { cachedInputTokens: 0, cacheWriteInputTokens: 0 } });
  await page.goto("/");
  await page.getByRole("button", { name: "Usage", exact: true }).click();
  await page.getByRole("button", { name: "Request details: req_synthetic_local" }).click();
  const retention = page.locator(".request-cache-retention dd");
  await expect(retention).toHaveText("≈ 18 min left");
  await page.evaluate(() => {
    const internals = (window as unknown as { __TAURI_INTERNALS__: { invoke: (command: string, args?: unknown, options?: unknown) => Promise<unknown> } }).__TAURI_INTERNALS__;
    const invoke = internals.invoke.bind(internals);
    internals.invoke = async (command, args, options) => {
      const result = await invoke(command, args, options);
      if (command === "get_local_cache_sessions") {
        return (result as Record<string, unknown>[]).map((session) => ({ ...session, touchedAt: new Date().toISOString() }));
      }
      return result;
    };
  });
  await emitTauriEvent(page, "zenith-usage-recorded", null);
  await expect(retention).toHaveText("≈ 30 min left");
  const writes = page.locator(".request-details-token-child").filter({ has: page.getByText("Cache writes", { exact: true }) });
  await expect(writes.locator(".request-token-count")).toHaveText("0");
  await expect(writes.locator(".request-token-cost")).toHaveText("—");
});
