import { expect, test, type Page } from "../bun-playwright";
import { installTauriMock, emitTauriEvent } from "./tauri-mock";
import { completedUsage, receiptCacheShare, receiptField, setUsageEventOverrides } from "./request-details-fixture";

async function openCache(page: Page, overrides: Record<string, unknown>) {
  // The synthetic chat has a later 30m session, unrelated to this receipt.
  await installTauriMock(page, { locale: "en", populated: true, cachePreview: true });
  await page.goto("/");
  await setUsageEventOverrides(page, { ...completedUsage, ...overrides });
  await page.getByRole("button", { name: "Usage", exact: true }).click();
  await page.getByRole("button", { name: "Request details: req_synthetic_local" }).click();
  return page.getByRole("dialog");
}

test("a hit never inherits a deadline from another send or a warm socket", async ({ page }) => {
  const dialog = await openCache(page, { cacheWriteTtl: null, routing: { reason: "response_affinity",
    endpointKind: "chatgpt_siwc", transport: { upstream: "websocket" } } });
  await expect(receiptCacheShare(dialog).locator("dd")).toHaveText("83.4%");
  await dialog.locator(".request-technical-details > summary").click();
  const retention = receiptField(dialog, /retention|cache storage/i);
  await expect(retention).toHaveCount(0);
  await emitTauriEvent(page, "zenith-usage-recorded", null);
  await expect(retention).toHaveCount(0);
  expect(await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: { command: string }[] })
    .__TAURI_TEST_INVOKES__.filter(({ command }) => command === "get_local_cache_sessions"))).toEqual([]);
});

for (const [field, count] of [["5m", 8], ["30m", 3]] as const) {
  test(`per-prefix ${field} writes don't inherit the default or add tokens`, async ({ page }) => {
    const dialog = await openCache(page, { cacheWriteTtl: `${field}, 12h`, cacheWriteInputTokens: null,
      toolUse: { ...completedUsage.toolUse, providerUsage: {
        cacheMode: "explicit", cacheRetention: "1h", cacheWriteTokensByTtl: { "12h": 10, [field]: count, "15m": 0 },
      } } });
    await expect(dialog.getByText("Default cache retention")).toBeHidden();
    await dialog.locator(".request-technical-details > summary").click();
    await expect(receiptField(dialog, "Cache mode").locator("dd")).toHaveText("Marked prefixes");
    await expect(receiptField(dialog, "Default cache retention").locator("dd")).toHaveText("1 hr");
    const writes = receiptField(dialog, /^Cache writes for/);
    await expect(writes.locator("dt")).toHaveText([`Cache writes for ${field === "5m" ? "5 min" : "30 min"}`, "Cache writes for 12 hr"]);
    await expect(writes.locator("dd")).toHaveText([`${count} tokens`, "10 tokens"]);
    await expect(page.locator(".request-token-total")).toHaveText("19,340");
    await expect(page.locator(".request-ledger").getByText("Cache writes")).toHaveCount(0);
    await expect(page.locator(".request-ledger").getByText("Regular input")).toHaveCount(0);
  });
}

test("in-memory policy without a cache counter implies neither a hit nor a duration", async ({ page }) => {
  const dialog = await openCache(page, { cachedInputTokens: null, toolUse: { ...completedUsage.toolUse,
    providerUsage: { cacheMode: "implicit", cacheRetention: "in_memory" } } });
  await expect(receiptCacheShare(dialog)).toHaveCount(0);
  await expect(dialog.getByText("In memory")).toBeHidden();
  await dialog.locator(".request-technical-details > summary").click();
  await expect(receiptField(dialog, "Cache mode").locator("dd")).toHaveText("Automatic");
  await expect(receiptField(dialog, "Cache storage").locator("dd")).toHaveText("In memory");
  await expect(receiptField(dialog, /retention|writes for/i)).toHaveCount(0);
});

test("historical mixed windows without counts stay windows, not writes", async ({ page }) => {
  const dialog = await openCache(page, { cacheWriteTtl: "5m, 1h", cacheWriteInputTokens: 0, cachedInputTokens: 0 });
  await expect(receiptCacheShare(dialog).locator("dd")).toHaveText("0%");
  await dialog.locator(".request-technical-details > summary").click();
  await expect(receiptField(dialog, "Cache retention").locator("dd")).toHaveText("5 min / 1 hr");
  await expect(receiptField(dialog, /^Cache writes for/)).toHaveCount(0);
  await expect(page.locator(".request-ledger").getByRole("row").filter({ hasText: "Cache writes" }).locator(".request-token-count")).toHaveText("0");
});

test("a write with unknown input and no duration does not imply a hit", async ({ page }) => {
  const dialog = await openCache(page, { inputTokens: null, cachedInputTokens: null, cacheWriteInputTokens: 9, cacheWriteTtl: null });
  await expect(receiptCacheShare(dialog)).toHaveCount(0);
  await expect(dialog.getByRole("row").filter({ hasText: "Cache writes" }).locator(".request-token-count")).toHaveText("9");
  await dialog.locator(".request-technical-details > summary").click();
  await expect(receiptField(dialog, /retention|cache storage/i)).toHaveCount(0);
});

test("multiple storage durations stay out of the summary and the total is charged once", async ({ page }) => {
  await page.setViewportSize({ width: 840, height: 760 });
  await installTauriMock(page, { locale: "ru", theme: "dark", populated: true,
    usageRequestedModel: "demo-responses", usageResolvedModel: "demo-responses" });
  await page.goto("/");
  // No price was reported for the 768 written tokens.
  await setUsageEventOverrides(page, { ...completedUsage, cacheWriteInputTokens: 768, cacheWriteTtl: "30m, 12h",
    apiEquivalent: { microUsd: 23370, pricedTokens: 18572, unpricedTokens: 768 },
    apiEquivalentBreakdown: { ...completedUsage.apiEquivalentBreakdown,
      input: { microUsd: 9570, pricedTokens: 17652, unpricedTokens: 768 },
      uncachedInput: { microUsd: 5730, pricedTokens: 2292, unpricedTokens: 0 },
      cacheWrite: { microUsd: 0, pricedTokens: 0, unpricedTokens: 768 } },
    toolUse: { ...completedUsage.toolUse, providerUsage: { cacheMode: "explicit", cacheRetention: "30m",
      cacheWriteTokensByTtl: { "12h": 512, "30m": 256 } } } });
  await page.getByRole("button", { name: "Использование", exact: true }).click();
  await page.getByRole("button", { name: "Сведения о запросе: req_synthetic_local" }).click();
  const dialog = page.getByRole("dialog");
  await expect(receiptCacheShare(dialog).locator("dd")).toHaveText(/83,4\s*%/);
  await expect(dialog.getByText("256 ток.")).toBeHidden();
  await expect(dialog.locator(".request-ledger").getByRole("row").filter({ hasText: "Записано в кэш" }).locator("td")).toHaveText(["768", "—"]);
  await expect(dialog.locator(".request-ledger tfoot td")).toHaveText(["19 340", "≈0,02337 $"]);
  await page.mouse.move(0, 0);
  await dialog.screenshot({ path: "output/playwright/request-inspector-writes-ru.png" });
  await dialog.locator(".request-technical-details > summary").click();
  const writes = receiptField(dialog, /^Запись в кэш на/);
  await expect(writes.locator("dt")).toHaveText(["Запись в кэш на 30 мин", "Запись в кэш на 12 ч"]);
  await expect(writes.locator("dd")).toHaveText(["256 ток.", "512 ток."]);
});
