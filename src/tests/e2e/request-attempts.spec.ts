import { test, expect } from "../bun-playwright";
import { installTauriMock } from "./tauri-mock";
import { noOutputUsage, setUsageEventOverrides } from "./request-details-fixture";

for (const locale of ["en", "ru"] as const) {
  for (const category of [null, "upstream_quota_exhausted", "client_cancelled"] as const) {
    test(`request status keeps its reason in a tooltip: ${locale}, category=${category}`, async ({ page }) => {
      await installTauriMock(page, { mode: "local", locale, populated: true, usageFailure: true });
      await page.goto("/");
      await setUsageEventOverrides(page, { errorCategory: category });
      await page.getByRole("button", { name: locale === "ru" ? "Использование" : "Usage", exact: true }).click();
      const label = locale === "ru" ? "Ошибка" : "Failed";
      const reason = category === "client_cancelled"
        ? locale === "ru" ? "Клиент отменил запрос" : "Client cancelled the request"
        : category ? locale === "ru" ? "Квота аккаунта исчерпана" : "Account quota exhausted" : label;
      const status = page.locator('.usage-request-table tbody td[data-column="status"]');
      await expect(status).toHaveText("");
      const icon = status.getByRole("img", { name: reason, exact: true });
      await expect(icon).toHaveAttribute("data-status", category === "client_cancelled" ? "info" : "error");
      await expect(status.locator("small")).toHaveCount(0);
      if (category) {
        await status.locator(".usage-request-result").hover();
        await expect(page.getByRole("tooltip")).toHaveText(reason);
        await page.mouse.move(0, 0);
        await expect(page.getByRole("tooltip")).toHaveCount(0);
      }
      await icon.hover();
      await expect(page.getByRole("tooltip")).toHaveText(reason);
      await icon.focus();
      await expect(page.getByRole("tooltip")).toHaveText(reason);
    });
  }

  test(`successful request status has a hover and keyboard tooltip: ${locale}`, async ({ page }) => {
    await installTauriMock(page, { mode: "local", locale, populated: true });
    await page.goto("/");
    await page.getByRole("button", { name: locale === "ru" ? "Использование" : "Usage", exact: true }).click();
    const label = locale === "ru" ? "Успешно" : "Success";
    const status = page.locator('.usage-request-table tbody td[data-column="status"]');
    await expect(status).toHaveText("");
    const icon = status.getByRole("img", { name: label, exact: true });
    await expect(icon).toHaveAttribute("data-status", "ready");
    await icon.hover();
    await expect(page.getByRole("tooltip")).toHaveText(label);
    await icon.focus();
    await expect(page.getByRole("tooltip")).toHaveText(label);
  });
}

for (const mode of ["local"] as const) {
  for (const locale of ["en", "ru"] as const) {
    test(`${mode} ${locale} client cancellation is not displayed as an HTTP 502 provider failure`, async ({ page }) => {
      await page.setViewportSize({ width: 390, height: 844 });
      await installTauriMock(page, {
        mode, locale, theme: locale === "ru" ? "dark" : "light", populated: true,
        usageFailure: true, usageAttemptOutcome: "client_disconnected",
      });
      await page.goto("/");
      await setUsageEventOverrides(page, {
        errorCategory: "client_cancelled", httpStatus: 502,
        upstreamError: { httpStatus: 429, code: "rate_limit_exceeded", errorType: "tokens", message: "synthetic previous rejection", redacted: false, truncated: false },
      });
      await page.getByRole("button", { name: locale === "en" ? "Usage" : "Использование", exact: true }).click();
      const label = locale === "en" ? "Client cancelled the request" : "Клиент отменил запрос";
      const detailsLabel = locale === "en" ? "Request details" : "Сведения о запросе";
      await expect(page.locator(".usage-request-table").getByRole("img", { name: label })).toHaveAttribute("data-status", "info");
      await page.getByRole("button", { name: `${detailsLabel}: req_synthetic_${mode}` }).click();
      const details = page.getByRole("dialog", { name: detailsLabel });
      await expect(details.locator(".request-details-status")).toHaveAttribute("data-status", "cancelled");
      await expect(details.locator(".request-details-status")).toHaveText(label);
      await expect(details.locator(".request-details-error")).toHaveCount(0);
      await expect(details.locator(".request-outcome-note")).toHaveCount(0);
      await expect(details).not.toContainText("HTTP 502");
      await expect(details).not.toContainText("synthetic previous rejection");
      await expect.poll(() => details.evaluate((element) => element.scrollWidth <= element.clientWidth)).toBe(true);
      await details.screenshot({ path: `output/playwright/usage-cancelled-${locale}.png` });
    });

    test(`${mode} ${locale} catalog discovery never displays an HTTP 200 failure`, async ({ page }) => {
      const pendingLabel = locale === "en" ? "Tool lookup" : "Поиск инструментов";
      const detailsLabel = locale === "en" ? "Request details" : "Сведения о запросе";
      await page.setViewportSize({ width: 390, height: 844 });
      await installTauriMock(page, {
        mode, locale, populated: true, usageAttemptOutcome: "tool_discovery",
      });
      await page.goto("/");
      await setUsageEventOverrides(page, { success: false, httpStatus: 200, errorCategory: null, ttftMs: null });
      await page.getByRole("button", { name: locale === "en" ? "Usage" : "Использование", exact: true }).click();
      await expect(page.locator(".usage-request-table").getByRole("img", { name: pendingLabel })).toHaveAttribute("data-status", "info");
      await page.locator(".usage-request-table").getByRole("img", { name: pendingLabel }).hover();
      await expect(page.getByRole("tooltip")).toHaveText(pendingLabel);
      await page.getByRole("button", { name: `${detailsLabel}: req_synthetic_${mode}` }).click();
      const details = page.getByRole("dialog", { name: detailsLabel });
      await expect(details.locator(".request-details-status")).toHaveAttribute("data-status", "pending");
      await expect(details.locator(".request-details-status")).toHaveText(pendingLabel);
      await expect(details.locator(".request-details-error")).toHaveCount(0);
      expect(await details.evaluate((element) => element.scrollWidth <= element.clientWidth)).toBe(true);
      await details.screenshot({ path: `output/playwright/usage-discovery-${mode}-${locale}.png` });
    });
  }
}

for (const mode of ["local"] as const) {
  test(`${mode} shows each send with its own consumption and keeps logical counts`, async ({ page }) => {
    await installTauriMock(page, { mode, locale: "en", populated: true, usageAttempts: true });
    await page.goto("/");
    await page.getByRole("button", { name: "Usage", exact: true }).click();
    const rows = page.locator(".usage-request-table tbody tr");
    await expect(rows).toHaveCount(3);
    await expect(rows.locator('td[data-column="tokens"]')).toHaveText(["84", "56", "28"]);
    await expect(rows.last().getByRole("img", { name: "Tool lookup" })).toHaveAttribute("data-status", "info");
    await expect(rows.nth(1).getByRole("img", { name: "Upstream failure" })).toHaveAttribute("data-status", "error");
    await expect(page.locator(".usage-overview .usage-metric").filter({ hasText: "Client requests" }).locator("strong")).toHaveText("1");
    await expect(page.locator(".usage-token-metric strong")).toHaveText("168");
    await page.screenshot({ path: "output/playwright/usage-physical-sends.png" });
    await rows.first().getByRole("button", { name: /^Request details:/ }).click();
    const details = page.getByRole("dialog", { name: "Request details" });
    await expect(details.getByRole("tab", { name: "Attempts", exact: true })).toHaveCount(0);
    await expect(details.locator(".request-execution")).toHaveCount(0);
    await expect(details.locator('.request-details-status[data-status="success"]')).toHaveAccessibleName("Success");
    await expect(details.locator(".request-details-error")).toHaveCount(0);
    const queries = await page.evaluate(() =>
      (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ args: { input?: Record<string, unknown> } }> })
        .__TAURI_TEST_INVOKES__.map((call) => call.args.input).filter((query) => query?.requestId));
    expect(queries).toHaveLength(0);
    await page.keyboard.press("Escape");
    await rows.last().getByRole("button", { name: /^Request details:/ }).click();
    await expect(details.locator(".request-details-status")).toHaveText("Tool lookup");
    await expect(details.locator(".request-details-error")).toHaveCount(0);
    await page.keyboard.press("Escape");
    await page.getByRole("tab", { name: "Models", exact: true }).click();
    const lookupGroup = page.locator(".usage-aggregate-table tbody tr").filter({ hasText: "lookup-model" });
    await expect(lookupGroup.locator('td[data-column="requests"]')).toHaveText("0");
    await expect(lookupGroup.locator('td[data-column="input"]')).toHaveText("20");
    await expect(lookupGroup.locator('td[data-column="output"]')).toHaveText("8");
    await expect(lookupGroup).not.toContainText("NaN");
    await page.getByRole("tab", { name: "Errors", exact: true }).click();
    await expect(page.locator(".usage-error-table tbody tr")).toHaveCount(1);
    await page.getByRole("tab", { name: "Sends", exact: true }).click();
    await rows.last().getByRole("button", { name: /^Request details:/ }).click();
    await page.setViewportSize({ width: 390, height: 800 });
    expect(await details.evaluate((element) => element.scrollWidth <= element.clientWidth + 1)).toBe(true);
  });
}

test("send details no longer depend on a second history request", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, usageAttempts: true, usageAttemptsFail: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Usage", exact: true }).click();
  await page.getByRole("button", { name: "Request details: req_synthetic_local" }).first().click();
  const details = page.getByRole("dialog", { name: "Request details" });
  await expect(details.locator(".request-execution")).toHaveCount(0);
  await expect(details.getByText("No attempts found.", { exact: false })).toHaveCount(0);
  await expect(details.locator('.request-details-status[data-status="success"]')).toHaveAccessibleName("Success");
  await expect(details.getByRole("button", { name: "Refresh", exact: true })).toHaveCount(0);
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

for (const mode of ["local"] as const) {
  test(`${mode} failures before output omit unknown metrics and empty tabs`, async ({ page }) => {
    await installTauriMock(page, { mode, locale: "en", populated: true, usageFailure: true });
    await page.goto("/");
    await setUsageEventOverrides(page, {
      ...noOutputUsage, requestedReasoningEffort: null, effectiveReasoningEffort: null,
      serviceTier: null, appliedServiceTier: null, toolUse: null,
    });
    await page.getByRole("button", { name: "Usage", exact: true }).click();
    const timing = page.locator('.usage-request-table tbody td[data-column="timing"]');
    await expect(timing).toHaveText(mode === "local" ? "— / 428 ms" : "— / 512 ms");
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
    await page.getByRole("button", { name: "Request details: req_synthetic_local" }).first().click();
    const details = page.getByRole("dialog", { name: "Request details" });
    await expect(details.locator(".request-execution-list > li")).toHaveCount(0);
    if (outcome === "execution_uncertain") await expect(details.locator(".request-outcome-note")).toHaveText(
      "Retry stopped: the previous send may have executed. Sending again could duplicate the request.");
    else await expect(details.locator(".request-outcome-note")).toHaveCount(0);
    if (outcome === "client_disconnected") {
      await expect(details.locator(".request-details-error")).toHaveCount(0);
      await expect(details.locator(".request-details-status")).toHaveAttribute("data-status", "cancelled");
      await expect(details.locator(".request-details-identity")).toContainText("Client cancelled the request");
      await expect(details).not.toContainText("Synthetic provider failure");
    } else {
      await expect(details.locator(".request-details-error")).toContainText("Synthetic provider failure");
      await expect(details.locator(".request-details-identity")).toContainText("Failed");
      await expect(details.locator(".request-details-error")).toContainText("502");
    }
  });
}

test("partial history does not invent missing sends", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, usageAttempts: true, usageAttemptsPartial: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Usage", exact: true }).click();
  await page.getByRole("button", { name: "Request details: req_synthetic_local" }).click();
  const details = page.getByRole("dialog", { name: "Request details" });
  await expect(page.locator(".usage-request-table tbody tr")).toHaveCount(1);
  await expect(details.locator(".request-execution")).toHaveCount(0);
  await expect(details.locator(".request-execution-list > li")).toHaveCount(0);
  await expect(details.locator('.request-details-status[data-status="success"]')).toHaveAccessibleName("Success");
});

test("different client request IDs stay separate and have distinguishable labels", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, usageFailure: true, usageRepeatedRequests: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Usage", exact: true }).click();
  const records = page.locator(".usage-request-table tbody tr");
  await expect(records).toHaveCount(4);
  const ids = await records.locator(".request-link").evaluateAll((buttons) => buttons.map((button) => button.getAttribute("aria-label")));
  expect(new Set(ids).size).toBe(4);
  const status = records.first().locator('td[data-column="status"]');
  await expect(status).toHaveText("");
  await expect(status.getByRole("img", { name: "Upstream failure", exact: true })).toHaveAttribute("data-status", "error");
  await expect(records.first().locator('.usage-request-result')).toHaveAttribute("data-relay-tooltip", "Upstream failure");
  await expect(records.first().locator('td[data-column="connection"]')).not.toContainText("Upstream failure");
  for (const record of await records.all()) {
    const button = record.getByRole("button", { name: /^Request details:/ });
    await expect(button).toHaveText("Details");
    const id = await button.getAttribute("data-relay-tooltip");
    expect(id).toBeTruthy();
    await button.click();
    const details = page.getByRole("dialog", { name: "Request details" });
    await details.locator(".request-technical-details > summary").click();
    await expect(details.getByText(id!, { exact: true })).toBeVisible();
    await page.keyboard.press("Escape");
  }
});

for (const theme of ["dark", "light"] as const) {
  for (const width of [390, 1160]) {
    test(`Russian request activity fits ${theme} ${width}`, async ({ page }) => {
      await page.setViewportSize({ width, height: 1000 });
      await installTauriMock(page, { mode: "local", locale: "ru", theme, populated: true, usageAttempts: true,
        usageFailure: true, usageAttemptOutcome: "execution_uncertain" });
      await page.goto("/");
      await page.getByRole("button", { name: "Использование", exact: true }).click();
      await page.getByRole("button", { name: "Сведения о запросе: req_synthetic_local" }).first().click();
      const details = page.getByRole("dialog", { name: "Сведения о запросе" });
      await expect(details.locator(".request-execution")).toHaveCount(0);
      await expect(details.locator(".request-outcome-note")).toBeVisible();
      expect(await details.locator(".relay-dialog-body, .request-send-content, .request-history-notice, .request-details-list dd")
        .evaluateAll((elements) => elements.every((element) => element.scrollWidth <= element.clientWidth + 1))).toBe(true);
      await page.screenshot({ path: `output/playwright/request-balanced-execution-${theme}-${width}.png` });
    });
  }
}
