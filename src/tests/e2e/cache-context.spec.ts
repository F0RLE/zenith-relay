import { expect, test } from "../bun-playwright";
import type { CacheContextDiagnostics } from "../../src/features/relay/api/types";
import { installTauriMock } from "./tauri-mock";

function comparison(): CacheContextDiagnostics {
  return {
    baseline: "completed_request",
    scope: "client_session",
    clientChanges: ["tools", "instructions"],
    upstreamChanges: ["tools", "instructions", "reasoning"],
    relayChanges: ["reasoning"],
    clientHistory: {
      comparison: "appended", inputItems: 12, inputBytes: 643_210,
      sharedPrefixItems: 8, firstChangedItemKind: null,
    },
    upstreamHistory: {
      comparison: "rewritten", inputItems: 12, inputBytes: 643_198,
      sharedPrefixItems: 0, firstChangedItemKind: "developer",
    },
    relayHistory: {
      comparison: "rewritten", inputItems: 12, inputBytes: 643_198,
      sharedPrefixItems: 0, firstChangedItemKind: "developer",
    },
    candidateChanged: true,
    previousCompletedAgeMs: 12_000,
  };
}

for (const locale of ["ru", "en"] as const) {
  for (const mode of ["local", "remote"] as const) {
    test(`Context comparison ${locale} ${mode} fits a narrow dialog`, async ({ page }) => {
      await page.setViewportSize({ width: 390, height: 760 });
      await installTauriMock(page, { mode, locale, populated: true, diagnosticDebug: true, usageCacheContext: comparison() });
      await page.goto("/");
      await page.getByRole("button", { name: locale === "ru" ? "Использование" : "Usage", exact: true }).click();
      await page.locator(".usage-request-table tbody tr button").first().click();
      const dialog = page.getByRole("dialog");
      await dialog.getByRole("tab", { name: locale === "ru" ? "Контекст" : "Context", exact: true }).click();
      const section = dialog.getByRole("region", { name: locale === "ru" ? "Сравнение контекста" : "Context comparison" });
      await expect(section).toBeVisible();
      await expect(section).toContainText(locale === "ru" ? "Предыдущий завершённый запрос" : "Previous completed request");
      await expect(section).toContainText(locale === "ru" ? "Первое изменение: Инструкции" : "First change: Instructions");
      const relayChanges = section.locator("dl > div").filter({ has: page.getByText(locale === "ru" ? "Изменения Relay в этом запросе" : "Relay changes in this request", { exact: true }) });
      await expect(relayChanges.locator("dd")).toHaveText(locale === "ru" ? "Рассуждение" : "Reasoning");
      await expect(section).toContainText(locale === "ru" ? "Размер JSON — не число токенов" : "JSON bytes are not tokens");
      expect(await dialog.locator(".relay-dialog-body, .request-cache-context, .request-details-list dd").evaluateAll((elements) =>
        elements.every((element) => element.scrollWidth <= element.clientWidth + 1))).toBe(true);
      if (locale === "ru" && mode === "local") {
        await section.scrollIntoViewIfNeeded();
        await page.screenshot({ path: "output/playwright/context-comparison-ru-390.png" });
      }
    });
  }
}

for (const baseline of ["first_observation", "overlapping_requests", "unavailable", "size_limit"] as const) {
  test(`Context comparison never invents changes with ${baseline}`, async ({ page }) => {
    const diagnostics = comparison();
    diagnostics.baseline = baseline;
    diagnostics.clientHistory.comparison = "not_compared";
    diagnostics.upstreamHistory.comparison = "not_compared";
    diagnostics.relayHistory.comparison = "not_compared";
    await installTauriMock(page, { mode: "local", locale: "en", populated: true, diagnosticDebug: true, usageCacheContext: diagnostics });
    await page.goto("/");
    await page.getByRole("button", { name: "Usage", exact: true }).click();
    await page.getByRole("button", { name: "Request details: req_synthetic_local" }).click();
    await page.getByRole("tab", { name: "Context", exact: true }).click();
    const section = page.getByRole("region", { name: "Context comparison" });
    await expect(section).toBeVisible();
    await expect(section.getByText("Client parameter changes", { exact: true })).toHaveCount(0);
    await expect(section.getByText("Pool participant", { exact: true })).toHaveCount(0);
    await expect(section.getByText("Relay changes in this request", { exact: true })).toHaveCount(0);
  });
}

test("Old usage rows without context diagnostics remain readable", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Usage", exact: true }).click();
  await page.getByRole("button", { name: "Request details: req_synthetic_local" }).click();
  await page.getByRole("tab", { name: "Tokens", exact: true }).click();
  await expect(page.getByRole("region", { name: "Context comparison" })).toHaveCount(0);
  await expect(page.getByRole("dialog")).toContainText("Total tokens");
});

test("Context diagnostics are hidden outside debug mode", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, diagnosticDebug: false, usageCacheContext: comparison() });
  await page.goto("/");
  await page.getByRole("button", { name: "Usage", exact: true }).click();
  await page.getByRole("button", { name: "Request details: req_synthetic_local" }).click();
  await expect(page.getByRole("tab", { name: "Context", exact: true })).toHaveCount(0);
  await expect(page.getByRole("region", { name: "Context comparison" })).toHaveCount(0);
});
