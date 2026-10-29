import { expect, test } from "../bun-playwright";
import { installTauriMock } from "./tauri-mock";

test("shared calibration crosses the snapshot boundary and linked routes count once", async ({ page }) => {
  await installTauriMock(page, { accountRouteKinds: ["codex", "excel_bps", "siwc"] });
  await page.goto("/");
  await page.getByRole("button", { name: "Pool", exact: true }).click();
  const cards = page.locator('.pool-member-card[data-member-kind="account"]');
  await expect(cards).toHaveCount(3);
  for (const card of await cards.all()) {
    await expect(card.locator(".account-value-strip dd").nth(1)).toHaveText("≈$6.4");
  }
  const total = page.locator('[data-metric="api-remaining"]');
  await expect(total.locator("strong")).toHaveText("≈$6.4");
  await expect(total).toHaveAttribute("data-relay-tooltip", /1 of 1 independent pool limits/);
  const remaining = cards.first().locator(".account-value-strip > div").nth(1);
  await remaining.focus();
  await expect(page.getByRole("tooltip")).toContainText("Price coverage 100%");
  await expect(page.getByRole("tooltip")).toContainText("subscriptions: 3");
  await expect(page.getByRole("tooltip")).toContainText("Median estimate from weekly quota");
});

test("missing calibration is an explicit unknown, not the old per-account extrapolation", async ({ page }) => {
  await installTauriMock(page, { accountCalibrationMissing: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Pool", exact: true }).click();
  const strip = page.locator('.pool-member-card[data-member-kind="account"] .account-value-strip').first();
  await expect(strip.locator("dd").nth(0)).toHaveText("$14.1");
  await expect(strip.locator("dd").nth(1)).toHaveText("—");
  await expect(strip.locator("dd").nth(2)).toHaveText("78%");
  await expect(page.locator('[data-metric="api-remaining"] strong')).toHaveText("—");
  await strip.locator(":scope > div").nth(1).focus();
  await expect(page.getByRole("tooltip")).toContainText("unknown, not zero");
});

test("mixed workloads display one observed estimate instead of a range or midpoint", async ({ page }) => {
  await installTauriMock(page, { accountCalibrationMixed: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Pool", exact: true }).click();
  const strip = page.locator('.pool-member-card[data-member-kind="account"] .account-value-strip').first();
  await expect(strip.locator("dd").nth(1)).toHaveText("≈$5.44");
  await expect(page.locator('[data-metric="api-remaining"] strong')).toHaveText("≈$5.44");
  await strip.locator(":scope > div").nth(1).focus();
  await expect(page.getByRole("tooltip")).toContainText("2 workload profiles");
  await expect(page.getByRole("tooltip")).toContainText("80%");
});

for (const locale of ["en", "ru"] as const) {
  test(`pool dollar toggle controls totals and cards across reloads (${locale})`, async ({ page }) => {
    await installTauriMock(page, { locale, theme: "dark" });
    await page.setViewportSize({ width: 920, height: 760 });
    await page.goto("/");
    const openPool = () => page.getByRole("button", { name: locale === "ru" ? "Пул" : "Pool", exact: true }).click();
    await openPool();
    const toggle = page.locator(".pool-quota-actions .account-calculation-toggle");
    const totals = page.locator(".pool-economics");
    const cards = page.locator('.pool-member-card[data-member-kind="account"] .account-value-strip');
    await expect(totals).toBeVisible();
    await expect(cards.first()).toBeVisible();
    const amounts = await totals.locator("strong").allTextContents();
    const accountAmounts = await cards.first().locator("dd").allTextContents();

    await toggle.click();
    await expect(toggle).toHaveAttribute("aria-pressed", "false");
    await expect(totals).toHaveCount(0);
    await expect(cards).toHaveCount(0);
    const unusedWidth = await page.locator(".pool-summary-band").evaluate((band) =>
      band.clientWidth - band.querySelector(".pool-summary")!.getBoundingClientRect().width);
    expect(Math.abs(unusedWidth)).toBeLessThanOrEqual(2);

    await page.reload();
    await openPool();
    await expect(toggle).toHaveAttribute("aria-pressed", "false");
    await expect(totals).toHaveCount(0);
    await expect(cards).toHaveCount(0);
    await toggle.click();
    await expect(totals.locator("strong")).toHaveText(amounts);
    await expect(cards.first().locator("dd")).toHaveText(accountAmounts);
  });
}

test("purchase-cost changes update card and pool payback without changing usage or quota estimates", async ({ page }) => {
  await installTauriMock(page);
  await page.goto("/");
  await page.getByRole("button", { name: "Pool", exact: true }).click();
  const card = page.locator('.pool-member-card[data-member-kind="account"]').first();
  const values = card.locator(".account-value-strip dd");
  await expect(values).toHaveText(["$14.1", "≈$6.4", "78%"]);
  await card.getByRole("button", { name: "Pool member policy: Personal Plus", exact: true }).click();
  const dialog = page.getByRole("dialog");
  await dialog.getByText("Settings", { exact: true }).click();
  await dialog.getByRole("spinbutton", { name: "Purchase cost, USD" }).fill("28.20");
  await dialog.getByRole("button", { name: /Save/ }).click();
  await expect(dialog).toHaveCount(0);
  await expect(values).toHaveText(["$14.1", "≈$6.4", "50%"]);
  await expect(page.locator('[data-metric="payback"] strong')).toHaveText("50%");
  const toggle = page.locator(".pool-quota-actions .account-calculation-toggle");
  await toggle.click();
  await expect(page.locator(".pool-economics")).toHaveCount(0);
  await toggle.click();
  await expect(values.nth(2)).toHaveText("50%");
  await expect(page.locator('[data-metric="payback"] strong')).toHaveText("50%");
});

test("source-only pools retain a way to restore hidden monetary totals", async ({ page }) => {
  await installTauriMock(page);
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  const account = page.locator(".account-card").first();
  await account.getByRole("button", { name: "Remove from pool", exact: true }).click();
  await expect(account.getByRole("button", { name: /^Add to pool/ })).toBeVisible();
  await page.getByRole("button", { name: "Pool", exact: true }).click();
  await expect(page.locator('.pool-member-card[data-member-kind="account"]')).toHaveCount(0);
  const toggle = page.locator(".pool-quota-actions .account-calculation-toggle");
  await expect(toggle).toBeVisible();
  await toggle.click();
  await expect(page.locator(".pool-economics")).toHaveCount(0);
  await toggle.click();
  await expect(page.locator(".pool-economics")).toBeVisible();
});
