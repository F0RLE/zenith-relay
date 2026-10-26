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
  await expect(page.getByRole("tooltip")).toContainText("Shared calibration");
  await expect(page.getByRole("tooltip")).toContainText("sampled subscriptions: 3");
  await expect(page.getByRole("tooltip")).toContainText("P10–P90");
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
