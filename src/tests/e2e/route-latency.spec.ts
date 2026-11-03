import { expect, test } from "../bun-playwright";
import { installTauriMock } from "./tauri-mock";
import { completedUsage, setUsageEventOverrides } from "./request-details-fixture";

for (const locale of ["en", "ru"] as const) {
  test(`route latency diagnostics remain compact and accessible in ${locale}`, async ({ page }) => {
    await page.setViewportSize({ width: 390, height: 900 });
    await installTauriMock(page, { mode: "local", locale, populated: true, diagnosticDebug: true });
    await page.goto("/");
    await setUsageEventOverrides(page, {
      ...completedUsage,
      routing: {
        reason: "only_eligible", eligibleCandidates: 1, quotaRemainingBasisPoints: null,
        inFlightBefore: 0, dispatchesBefore: 20,
        routeLatency: {
          state: "ready", sampleCount: 20, unknownCount: 2, minimumSamples: 20,
          windowMs: 600_000, asOfMs: 1_000, lastObservedAtMs: 900,
          p50Ms: 0, p90Ms: 180, p95Ms: 190,
        },
      },
    });
    await page.getByRole("button", { name: locale === "en" ? "Usage" : "Использование", exact: true }).click();
    await page.getByRole("button", {
      name: `${locale === "en" ? "Request details" : "Сведения о запросе"}: req_synthetic_local`, exact: true,
    }).click();
    const dialog = page.getByRole("dialog");
    await dialog.locator(".request-technical-details > summary").click();
    const quantiles = dialog.getByText(/^P50 0/);
    await expect(quantiles).toContainText("P90 180");
    await expect(quantiles).toContainText("P95 190");
    await expect(dialog.getByText(locale === "en" ? /20 measured · 2 unmeasured/ : /20 измерено · 2 без замера/)).toBeVisible();
    const freshness = dialog.locator("dl > div").filter({ has: page.locator("dt", {
      hasText: locale === "en" ? "Sample age at selection" : "Возраст замера при выборе",
    }) });
    await expect(freshness.locator("dd")).toHaveText(locale === "en" ? "100 ms" : "100 мс");
    await quantiles.focus();
    for (let activation = 0; activation < 3; activation++) {
      await page.keyboard.press("Shift+Tab");
      await page.keyboard.press("Tab");
      await expect(quantiles).toBeFocused();
      await expect(page.getByRole("tooltip")).toContainText(locale === "en" ? "Successful attempts" : "Успешные попытки");
      await expect(quantiles).toHaveAttribute("aria-describedby", await page.getByRole("tooltip").getAttribute("id") as string);
    }
    await expect(quantiles).not.toHaveAttribute("title");
    expect(await dialog.locator(".request-details-list dd").evaluateAll((cells) =>
      cells.every((cell) => cell.scrollWidth <= cell.clientWidth + 1))).toBe(true);
  });
}
