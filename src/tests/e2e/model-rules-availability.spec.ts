import { expect, test } from "../bun-playwright";
import { installTauriMock } from "./tauri-mock";

test("Model Rules preserves editable inventory while its only pool route is cooling down", async ({ page }) => {
  await installTauriMock(page, {
    locale: "en",
    mode: "local",
    populated: true,
    quotaAvailable: true,
    accountModelCooldown: true,
    accountModelErrorCode: "models_unauthorized",
    // The source keeps gpt-5.4-mini available. gpt-5.4 is available only
    // through the account whose exact route is in cooldown.
    serverModelOrder: ["gpt-5.4-mini"],
  });
  await page.setViewportSize({ width: 1160, height: 760 });
  await page.goto("/");
  await page.getByRole("button", { name: "Pool", exact: true }).click();
  await page.getByRole("tab", { name: "Model Rules", exact: true }).click();

  await expect(page.locator(".model-discovery-alert")).toHaveCount(0);
  const coolingModel = page.locator('[data-model-id="gpt-5.4"]');
  await expect(coolingModel).toBeVisible();
  await expect(coolingModel.getByRole("checkbox")).toBeChecked();
  await coolingModel.getByRole("checkbox").click();
  await expect(coolingModel.getByRole("checkbox")).not.toBeChecked();
  await expect(page.locator('[data-model-id="gpt-5.4-mini"]')).toBeVisible();
  await page.screenshot({ path: "output/playwright/model-rules-cooldown-inventory-en-light-1160x760.png" });
});

test("Model Rules shows the discovered catalog for a legacy API binding", async ({ page }) => {
  await installTauriMock(page, {
    locale: "en",
    mode: "local",
    populated: true,
    serverModelOrder: ["provider-only-model"],
    sourceProtocolBindings: [{
      wireApi: "responses",
      adapter: "native",
      reasoningMode: "disabled",
      modelIds: [],
    }],
  });
  await page.setViewportSize({ width: 1160, height: 760 });
  await page.goto("/");
  await page.getByRole("button", { name: "Pool", exact: true }).click();
  await page.getByRole("tab", { name: "Model Rules", exact: true }).click();

  await expect(page.locator('[data-model-id="provider-only-model"]')).toBeVisible();
});
