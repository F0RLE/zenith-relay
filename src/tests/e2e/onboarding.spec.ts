import { expect, test } from "@playwright/test";
import { installTauriMock } from "./tauri-mock";

test("quick setup covers all three runtime choices", async ({ page }) => {
  await installTauriMock(page, { onboarding: false, locale: "en", populated: true });
  await page.goto("/");
  await expect(page.getByRole("heading", { name: "Zenith Relay" })).toBeVisible();
  await page.getByRole("button", { name: "Get started" }).click();
  await expect(page.getByRole("heading", { name: "Where should Zenith Relay run?" })).toBeVisible();
  await page.getByRole("button", { name: /My server/ }).click();
  await page.getByRole("button", { name: "Continue" }).click();
  await page.getByLabel("Server address").fill("https://relay.example.invalid");
  await page.getByLabel("Management token").fill("synthetic-management-token-000000");
  await page.getByRole("button", { name: "Continue" }).click();
  await expect(page.getByRole("heading", { name: "Connection check" })).toBeVisible();
  await page.screenshot({ path: "output/playwright/onboarding-server-1160x760.png" });
});

test("quick setup can switch to Russian without untranslated keys", async ({ page }) => {
  await installTauriMock(page, { onboarding: false, locale: "ru", populated: true });
  await page.goto("/");
  await expect(page.getByRole("button", { name: "Приступить" })).toBeVisible();
  await expect(page.locator("body")).not.toContainText(/(?:common|onboarding|modes)\.[a-z]/);
});
