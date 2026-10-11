import { test, expect } from "../bun-playwright";
import { installTauriMock } from "./tauri-mock";
import { noOutputUsage, setUsageEventOverrides } from "./request-details-fixture";

for (const mode of ["local", "remote"] as const) {
  test(`${mode} shows all sends inside one request without changing its final result`, async ({ page }) => {
    await installTauriMock(page, { mode, locale: "en", populated: true, usageAttempts: true });
    await page.goto("/");
    await page.getByRole("button", { name: "Usage", exact: true }).click();
    await page.getByRole("button", { name: `Request details: req_synthetic_${mode}` }).click();
    const details = page.getByRole("dialog", { name: "Request details" });
    await expect(page.locator(".usage-request-table tbody tr")).toHaveCount(1);
    await expect(details.getByRole("tab", { name: "Attempts", exact: true })).toHaveCount(0);
    const sends = details.locator(".request-execution-list > li");
    await expect(sends).toHaveCount(3);
    await expect(sends.first()).toBeVisible();
    await expect(sends.nth(0)).toHaveAttribute("aria-label", "Send 1");
    await expect(sends.nth(0)).toContainText("HTTP 400");
    await expect(sends.nth(1)).toContainText("Selecting another connection");
    await expect(sends.nth(2)).toContainText("Success");
    await expect(details.locator(".request-details-identity")).toContainText("Success");
    await expect(details.locator(".request-details-error")).toHaveCount(0);
    const queries = await page.evaluate(() =>
      (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ args: { input?: Record<string, unknown> } }> })
        .__TAURI_TEST_INVOKES__.map((call) => call.args.input).filter((query) => query?.requestId));
    expect(queries.some((query) => query?.page === 2)).toBe(true);
    expect(queries.every((query) => query?.success === undefined && query?.range === undefined)).toBe(true);
    await page.setViewportSize({ width: 390, height: 800 });
    await sends.nth(0).scrollIntoViewIfNeeded();
    await expect(sends.nth(0)).toBeVisible();
    await sends.nth(2).scrollIntoViewIfNeeded();
    await expect(sends.nth(2)).toBeInViewport({ ratio: 1 });
    expect(await details.evaluate((element) => element.scrollWidth <= element.clientWidth + 1)).toBe(true);
  });
}

test("history failure is visible instead of an empty successful report", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, usageAttempts: true, usageAttemptsFail: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Usage", exact: true }).click();
  await page.getByRole("button", { name: "Request details: req_synthetic_local" }).click();
  const details = page.getByRole("dialog", { name: "Request details" });
  await expect(details.locator(".request-execution .request-section-badge")).toContainText("Unavailable");
  await expect(details.getByText("History is unavailable. Showing the saved request outcome.")).toBeVisible();
  await expect(details.getByText("No attempts found.", { exact: false })).toHaveCount(0);
  await expect(details.locator(".request-details-identity")).toContainText("Success");
  await details.getByRole("button", { name: "Refresh", exact: true }).click();
  await expect(details.getByText("History is unavailable. Showing the saved request outcome.")).toBeVisible();
});

test("single successful requests omit history and empty diagnostic sections", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Usage", exact: true }).click();
  await page.getByRole("button", { name: "Request details: req_synthetic_local" }).click();
  const details = page.getByRole("dialog", { name: "Request details" });
  await expect(details.locator(".request-execution")).toHaveCount(0);
  await expect(details.getByRole("tab", { name: "Attempts", exact: true })).toHaveCount(0);
  await expect(details.locator(".request-tool-details")).toHaveCount(0);
  await expect(details.getByRole("button", { name: "Refresh attempts", exact: true })).toHaveCount(0);
});

for (const mode of ["local", "remote"] as const) {
  test(`${mode} failures before output omit unknown metrics and empty tabs`, async ({ page }) => {
    await installTauriMock(page, { mode, locale: "en", populated: true, usageFailure: true });
    await page.goto("/");
    await setUsageEventOverrides(page, {
      ...noOutputUsage, requestedReasoningEffort: null, effectiveReasoningEffort: null,
      serviceTier: null, appliedServiceTier: null, toolUse: null,
    });
    await page.getByRole("button", { name: "Usage", exact: true }).click();
    const timing = page.locator('.usage-request-table tbody td[data-column="timing"]');
    await expect(timing).toHaveText(mode === "local" ? "428 ms" : "512 ms");
    await page.getByRole("button", { name: `Request details: req_synthetic_${mode}` }).click();
    const details = page.getByRole("dialog", { name: "Request details" });
    await expect(details.locator(".request-token-details")).toHaveCount(0);
    await expect(details.locator(".request-tool-details")).toHaveCount(0);
    await expect(details.locator(".request-details-metric")).toHaveCount(1);
    await expect(details.locator(".request-details-metric")).toContainText("Response time");
    await expect(details.locator(".request-details-identity")).toContainText("Failed");
    await expect(details.locator(".request-details-error")).toContainText("502");
  });
}

for (const outcome of ["execution_uncertain", "client_disconnected", null] as const) {
  test(`failed requests explain their recorded stop decision: ${outcome}`, async ({ page }) => {
    await installTauriMock(page, {
      mode: "local", locale: "en", populated: true, usageAttempts: true, usageFailure: true,
      usageAttemptOutcome: outcome,
      usageUpstreamError: { httpStatus: 502, code: "synthetic_failure", errorType: null,
        message: "Synthetic provider failure", redacted: false, truncated: false },
    });
    await page.goto("/");
    await page.getByRole("button", { name: "Usage", exact: true }).click();
    await page.getByRole("button", { name: "Request details: req_synthetic_local" }).click();
    const details = page.getByRole("dialog", { name: "Request details" });
    await expect(details.locator(".request-execution-list > li")).toHaveCount(3);
    await expect(details.locator(".request-stop-reason")).toHaveText(outcome === "execution_uncertain"
      ? "Retry stopped: the previous send may have executed. Sending again could duplicate the request."
      : outcome === "client_disconnected" ? "Retry stopped: the client disconnected."
        : "The reason retries stopped was not recorded.");
    await details.locator(".request-send-details").first().locator("summary").click();
    await expect(details.locator(".request-send-details").first().locator("pre")).toHaveText("Synthetic provider failure");
    await expect(details.locator(".request-details-identity")).toContainText("Failed");
    await expect(details.locator(".request-details-error")).toContainText("502");
    await expect(details.locator(".request-execution-list > li").first()).toContainText("HTTP 400");
  });
}

test("partial history does not invent missing sends", async ({ page }) => {
  await installTauriMock(page, { mode: "remote", locale: "en", populated: true, usageAttempts: true, usageAttemptsPartial: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Usage", exact: true }).click();
  await page.getByRole("button", { name: "Request details: req_synthetic_remote" }).click();
  const details = page.getByRole("dialog", { name: "Request details" });
  await expect(details.locator(".request-execution .request-section-badge")).toContainText("Incomplete");
  await expect(details.getByText("Part of the history is missing. Showing only saved sends.")).toBeVisible();
  await expect(details.locator(".request-execution-list > li")).toHaveCount(0);
  await expect(details.locator(".request-details-identity")).toContainText("Success");
});

test("different client request IDs stay separate and have distinguishable labels", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, usageFailure: true, usageRepeatedRequests: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Usage", exact: true }).click();
  const records = page.locator(".usage-request-table tbody tr");
  await expect(records).toHaveCount(4);
  const ids = await records.locator(".request-link code").allTextContents();
  expect(new Set(ids).size).toBe(4);
  await expect(records.first().locator(".usage-connection-value")).toContainText("Upstream failure");
});

for (const theme of ["dark", "light"] as const) {
  for (const width of [390, 1160]) {
    test(`Russian request activity fits ${theme} ${width}`, async ({ page }) => {
      await page.setViewportSize({ width, height: 1000 });
      await installTauriMock(page, { mode: "local", locale: "ru", theme, populated: true, usageAttempts: true,
        usageFailure: true, usageAttemptOutcome: "execution_uncertain" });
      await page.goto("/");
      await page.getByRole("button", { name: "Использование", exact: true }).click();
      await page.getByRole("button", { name: "Сведения о запросе: req_synthetic_local" }).click();
      const details = page.getByRole("dialog", { name: "Сведения о запросе" });
      await expect(details.locator(".request-execution-list > li")).toHaveCount(3);
      await expect(details.locator(".request-execution-list > li").first()).toBeVisible();
      expect(await details.locator(".relay-dialog-body, .request-send-content, .request-history-notice, .request-details-list dd")
        .evaluateAll((elements) => elements.every((element) => element.scrollWidth <= element.clientWidth + 1))).toBe(true);
      await page.screenshot({ path: `output/playwright/request-balanced-execution-${theme}-${width}.png` });
    });
  }
}
