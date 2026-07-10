import { expect, test } from "@playwright/test";
import { installTauriMock } from "./tauri-mock";

test("local commands are reachable from the operational UI", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await page.getByRole("tab", { name: "Sources" }).click();
  await page.getByRole("button", { name: "Edit" }).click();
  await expect(page.getByRole("dialog", { name: "Edit source" })).toBeVisible();
  await page.getByRole("button", { name: "Cancel" }).click();
  await page.getByRole("tab", { name: "Accounts" }).click();
  await page.getByRole("button", { name: "Sign in" }).first().click();
  await page.getByRole("button", { name: "Open sign-in" }).click();
  await expect(page.getByLabel(/Callback URL/)).toBeVisible();
  await page.getByRole("button", { name: "Cancel" }).click();

  await page.getByRole("button", { name: "Pool", exact: true }).click();
  await page.getByRole("button", { name: "Personal Plus" }).click();
  await page.getByRole("button", { name: "Save policy" }).click();
  await expect(page.getByText("Saved.")).toBeVisible();
  await page.getByRole("tab", { name: "Keys" }).click();
  await page.getByRole("button", { name: "Edit policy" }).click();
  await expect(page.getByRole("dialog", { name: "Edit policy" })).toBeVisible();
  await page.getByRole("button", { name: "Cancel" }).click();
  page.once("dialog", (dialog) => dialog.accept());
  await page.getByRole("button", { name: "Rotate" }).click();
  await expect(page.getByText("zlr_synthetic_rotated_key")).toBeVisible();

  await page.getByRole("button", { name: "Gateway", exact: true }).click();
  await page.getByRole("tab", { name: "Client Setup" }).click();
  await page.getByRole("button", { name: "Attach current endpoint" }).click();
  await expect(page.getByText(/backup was preserved/i)).toBeVisible();
});

test("Ready API top-up uses the stored-key backend command", async ({ page }) => {
  await installTauriMock(page, { mode: "zenith", locale: "en", populated: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await page.locator(".relay-page-header").getByRole("button", { name: "Top up" }).click();
  await expect(page.getByRole("dialog", { name: "Top up" })).toBeVisible();
  await page.getByLabel("Top-up amount, USD").fill("10");
  await page.getByRole("button", { name: "Open top-up" }).click();
  await expect(page.getByText("Top-up opened in Telegram.")).toBeVisible();
});
