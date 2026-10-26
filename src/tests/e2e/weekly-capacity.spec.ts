import { expect, test } from "../bun-playwright";
import { emitTauriEvent, installTauriMock } from "./tauri-mock";
import { chartWindows } from "../../src/features/relay/pages/overview/overviewAnalyticsModel";
import type { WeeklyCapacityReport } from "../../src/features/relay/api/types/weeklyCapacity";

function fixture(now: number): WeeklyCapacityReport {
  const windows = chartWindows("today", "en", new Date(now));
  const points = windows.slice(-6).filter((_, i) => i !== 3).map((window, i) => ({
    atMs: window.startMs, observedAtMs: Math.min(window.startMs + 60_000, now),
    sampleStartMs: window.startMs - 60_000, validUntilMs: now + 86_400_000,
    medianMicroUsd: (100 + i * 5) * 1_000_000, lowMicroUsd: 90_000_000, highMicroUsd: 140_000_000,
    accountCount: 3, consumedBasisPoints: 1500,
  }));
  const latest = points.at(-1)!;
  Object.assign(latest, {
    accountCount: 1, consumedBasisPoints: 500,
    lowMicroUsd: latest.medianMicroUsd, highMicroUsd: latest.medianMicroUsd,
  });
  const group = { id: "plus-model", plan: "plus", workload: "model · standard · short · high", cacheBand: 75, outputBand: 10,
    pricingRevision: "synthetic-v1", points, current: points.at(-1)!, stale: false };
  const summaryPoints = points.map((point) => ({
    ...point, medianMicroUsd: point.medianMicroUsd + 50_000_000,
    lowMicroUsd: point.lowMicroUsd + 50_000_000, highMicroUsd: point.highMicroUsd + 50_000_000,
  }));
  return { generatedAtMs: now,
    summaries: [
      { plan: "plus", pricingRevision: group.pricingRevision, points: summaryPoints, current: summaryPoints.at(-1)!, stale: false },
      { plan: "pro", pricingRevision: group.pricingRevision, points, current: group.current, stale: false },
    ],
    models: [{ ...group, model: "model" }, { ...group, model: "other-model" }, { ...group, model: "model", plan: "pro" }],
    groups: [group, { ...group, id: "plus-other", workload: "other-model · standard · short · high" }, { ...group, id: "pro-model", plan: "pro" }],
    events: [{ atMs: windows.at(-4)!.startMs, plan: "plus", kind: "reset", count: 1 }] };
}

for (const locale of ["ru", "en"] as const) {
  test(`weekly capacity is compact, grouped and keyboard-accessible (${locale})`, async ({ page }, testInfo) => {
    const now = Date.now();
    await page.clock.setFixedTime(now);
    await installTauriMock(page, { locale, theme: locale === "ru" ? "dark" : "light", weeklyCapacity: fixture(now) });
    await page.goto("/");
    const chart = page.locator(".weekly-capacity");
    await expect(chart.locator(".overview-chart-summary")).not.toHaveText("—");
    await expect(chart.locator(".overview-chart-dot:not(.overview-capacity-event)")).toHaveCount(5);
    await expect(chart.locator(".overview-capacity-band")).toHaveCount(5);
    await expect(chart.locator(".overview-capacity-event")).toHaveCount(1);
    await expect(chart.locator(".relay-option-trigger")).toHaveCount(2);
    const allModels = locale === "ru" ? "Все модели" : "All models";
    await expect(chart.locator(".relay-option-trigger").last()).toHaveText(allModels);
    await expect(chart.locator(".overview-chart-summary")).toContainText("170");
    await chart.scrollIntoViewIfNeeded();
    // Keep browser frames live and finish scrolling before keyboard hints open.
    await page.evaluate(() => new Promise<void>((resolve) => requestAnimationFrame(() => requestAnimationFrame(() => resolve()))));
    await chart.locator(".relay-option-trigger").last().focus();
    await page.keyboard.press("Tab");
    await expect(chart.locator(".overview-chart-summary")).toBeFocused();
    await expect(page.getByRole("tooltip", { name: locale === "ru" ? /^Данные на / : /^Data as of / })).toBeVisible();
    await page.screenshot({ path: testInfo.outputPath(`weekly-capacity-${locale}-summary.png`) });
    await chart.locator(".overview-chart-dot:not(.overview-capacity-event)").first().focus();
    const sampledTooltip = chart.locator('.overview-chart-dot:not(.overview-capacity-event) [role="tooltip"]').first();
    await expect(sampledTooltip).toBeVisible();
    await expect(sampledTooltip.locator("strong")).toContainText(/\d/);
    await expect(sampledTooltip).not.toContainText(/Аккаунтов в расчёте|Accounts sampled|Примерная оценка|Approximate estimate/);
    await expect(sampledTooltip).toContainText(locale === "ru" ? "Диапазон оценок:" : "Estimate range:");
    await expect(sampledTooltip.locator("small")).toHaveCount(1);
    await expect(sampledTooltip).not.toContainText("synthetic");
    await expect(sampledTooltip).not.toContainText("model ·");
    await chart.locator(".relay-option-trigger").last().click();
    await page.getByRole("option", { name: "model", exact: true }).click();
    await expect(chart.locator(".overview-chart-summary")).toContainText("120");
    await chart.locator(".relay-option-trigger").last().click();
    await page.getByRole("option", { name: allModels, exact: true }).click();
    await expect(chart.locator(".overview-chart-summary")).toContainText("170");
    await chart.locator(".relay-option-trigger").first().click();
    await page.getByRole("option", { name: "pro", exact: true }).click();
    await expect(chart.locator(".relay-option-trigger")).toHaveCount(2);
    await expect(chart.locator(".overview-capacity-controls")).toContainText(allModels);
    await expect(chart.locator(".overview-chart-summary")).toContainText("120");
    await chart.locator(".relay-option-trigger").first().click();
    await page.getByRole("option", { name: "plus", exact: true }).click();
    await expect(chart.locator(".relay-option-trigger").last()).toHaveText(allModels);
    await expect(chart.locator(".overview-chart-summary")).toContainText("170");
    await page.setViewportSize({ width: 860, height: 760 });
    await chart.scrollIntoViewIfNeeded();
    const bounds = await chart.evaluate((element) => ({ width: element.clientWidth, scroll: element.scrollWidth }));
    expect(bounds.scroll).toBeLessThanOrEqual(bounds.width + 1);
    expect((await chart.locator(".relay-option-trigger").first().boundingBox())!.width).toBeLessThan(100);
    expect((await chart.locator(".relay-option-trigger").last().boundingBox())!.width).toBeLessThan(150);
    const latestDot = chart.locator(".overview-chart-dot:not(.overview-capacity-event)").last();
    await latestDot.hover();
    const singleTooltip = latestDot.getByRole("tooltip");
    await expect(singleTooltip).toBeVisible();
    await expect(singleTooltip.locator("strong")).toContainText(/\d/);
    await expect(singleTooltip).not.toContainText(/Аккаунтов в расчёте|Accounts sampled|Примерная оценка|Approximate estimate/);
    await expect(singleTooltip.locator("small")).toHaveCount(0);
    await expect(singleTooltip).not.toContainText(locale === "ru" ? "Диапазон" : "range");
    const tooltipBounds = await singleTooltip.boundingBox();
    expect(tooltipBounds!.height).toBeLessThan(100);
    expect(tooltipBounds!.width).toBeLessThan(300);
    expect(tooltipBounds!.x).toBeGreaterThanOrEqual(0);
    expect(tooltipBounds!.x + tooltipBounds!.width).toBeLessThanOrEqual(860);
    await page.screenshot({ path: testInfo.outputPath(`weekly-capacity-${locale}-compact.png`) });
    await page.setViewportSize({ width: 1280, height: 900 });
    await chart.scrollIntoViewIfNeeded();
    await latestDot.hover();
    await expect(singleTooltip).toBeVisible();
    await page.screenshot({ path: testInfo.outputPath(`weekly-capacity-${locale}-wide.png`) });
  });
}

test("model menu collapses workload variants and pricing revisions into plain names", async ({ page }, testInfo) => {
  const now = Date.now();
  await page.clock.install({ time: now });
  const report = fixture(now);
  report.summaries = report.summaries.filter((summary) => summary.plan === "plus");
  const base = report.groups[0]!;
  report.groups = Array.from({ length: 20 }, (_, index) => ({
    ...base, id: `variant-${index}`, cacheBand: index % 4 * 25,
    workload: `example-model · standard · ${index % 2 ? "long" : "short"} · high${index % 3 ? "" : " + other-model · priority · long · high"}`,
  }));
  report.models = [
    { ...report.models[0]!, model: "example-model" },
    { ...report.models[0]!, model: "example-model", pricingRevision: "older", current: { ...base.current!, medianMicroUsd: 900_000_000 } },
    { ...report.models[1]!, model: "other-model" },
  ];
  await installTauriMock(page, { locale: "ru", theme: "dark", weeklyCapacity: report });
  await page.setViewportSize({ width: 860, height: 760 });
  await page.goto("/");
  const chart = page.locator(".weekly-capacity");
  await expect(chart.locator(".overview-capacity-plan")).toHaveText("plus");
  const model = chart.locator(".relay-option-trigger");
  await expect(model).toHaveCount(1);
  await expect(model).toHaveText("Все модели");
  await chart.scrollIntoViewIfNeeded();
  expect((await model.boundingBox())!.width).toBeLessThan(200);
  await model.click();
  const menu = page.getByRole("listbox", { name: "Модель", exact: true });
  await expect(menu.getByRole("option")).toHaveText(["Все модели", "example-model", "other-model"]);
  expect((await menu.boundingBox())!.width).toBeLessThan(220);
  await page.screenshot({ path: testInfo.outputPath("weekly-capacity-compact-menu.png") });
  await menu.getByRole("option", { name: "example-model", exact: true }).click();
  await expect(model).toHaveAttribute("data-value", "example-model");
  await expect(model).toHaveText("example-model");
  await expect(chart.locator(".overview-chart-summary")).toContainText("120");
  await model.click();
  await menu.getByRole("option", { name: "Все модели", exact: true }).click();
  await expect(chart.locator(".overview-chart-summary")).toContainText("170");
});

test("empty and failed measurements never become a zero-dollar estimate", async ({ page }) => {
  await installTauriMock(page, { locale: "en", weeklyCapacityError: true });
  await page.goto("/");
  const chart = page.locator(".weekly-capacity");
  await expect(chart).toContainText("Measurements could not be loaded");
  await expect(chart.locator(".overview-chart-summary")).toHaveText("—");
  await expect(chart.locator(".overview-chart-dot")).toHaveCount(0);
});

test("unrelated state events and navigation do not reload measured history", async ({ page }) => {
  await page.clock.install();
  await installTauriMock(page, { locale: "en" });
  await page.goto("/");
  await expect(page.locator(".weekly-capacity")).toContainText("No measurements");
  const reads = () => page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string }> }).__TAURI_TEST_INVOKES__.filter((call) => call.command === "get_local_weekly_capacity").length);
  const baseline = await reads();
  await emitTauriEvent(page, "zenith-state-changed", null);
  await page.clock.runFor(1_500);
  expect(await reads()).toBe(baseline);
  await page.getByRole("button", { name: "Settings", exact: true }).click();
  await page.getByRole("button", { name: "Overview", exact: true }).click();
  await expect(page.locator(".weekly-capacity")).toContainText("No measurements");
  expect(await reads()).toBe(baseline);
});
