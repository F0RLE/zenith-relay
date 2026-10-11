import { expect, test } from "../bun-playwright";
import { installTauriMock } from "./tauri-mock";
import { openGatewayApi } from "./operations/helpers";

for (const mode of ["local", "remote"] as const) {
  test(`${mode} timezone settings save per account and retain choices when disabled`, async ({ page }, testInfo) => {
    await installTauriMock(page, { mode, locale: "en", populated: true, accountCount: 2 });
    await page.goto("/");
    await openGatewayApi(page);
    const toggle = page.getByRole("checkbox", { name: "Bind request timezone" });
    await expect(toggle).not.toBeChecked();
    await toggle.check();
    await expect(toggle).toBeChecked();
    await page.getByRole("button", { name: "Account timezones", exact: true }).click();
    const dialog = page.getByRole("dialog", { name: "Account timezones" });
    const fields = dialog.getByRole("combobox");
    await fields.first().fill("Europe/Minsk");
    await fields.nth(1).fill("Invalid/Zone");
    await expect(dialog.getByRole("button", { name: "Save", exact: true })).toBeDisabled();
    await fields.nth(1).fill("");
    await dialog.getByRole("button", { name: "Save", exact: true }).click();
    await expect(dialog).not.toBeVisible();
    await toggle.uncheck();
    await expect(toggle).not.toBeChecked();
    await page.getByRole("button", { name: "Account timezones", exact: true }).click();
    await expect(dialog.getByRole("combobox").first()).toHaveValue("Europe/Minsk");
    await expect(dialog.getByRole("combobox").nth(1)).toHaveValue("");
    await page.screenshot({ path: testInfo.outputPath("timezone-dialog.png") });
  });
}

test("older servers do not offer unsupported timezone commands", async ({ page }) => {
  await installTauriMock(page, { mode: "remote", locale: "en", populated: true, remoteFeatures: ["route_recovery_v1"] });
  await page.goto("/");
  await openGatewayApi(page);
  await expect(page.getByRole("checkbox", { name: "Bind request timezone" })).toHaveCount(0);
});

test("Russian timezone settings fit a narrow dark window", async ({ page }, testInfo) => {
  await page.setViewportSize({ width: 640, height: 900 });
  await installTauriMock(page, { mode: "local", locale: "ru", theme: "dark", populated: true, accountCount: 2 });
  await page.goto("/");
  await page.getByRole("button", { name: "Доступ", exact: true }).click();
  await page.getByRole("tab", { name: "Локальный API", exact: true }).click();
  await expect(page.getByRole("checkbox", { name: "Привязка часового пояса" })).not.toBeChecked();
  await page.getByRole("button", { name: "Часовые пояса аккаунтов", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "Часовые пояса аккаунтов" });
  await expect(dialog.getByRole("combobox")).toHaveCount(2);
  await dialog.getByRole("combobox").first().fill("Europe/Minsk");
  await expect(dialog.getByRole("button", { name: "Сохранить", exact: true })).toBeEnabled();
  const width = await dialog.evaluate((element) => ({
    scroll: element.scrollWidth, client: element.clientWidth,
  }));
  expect(width.scroll).toBeLessThanOrEqual(width.client + 1);
  await page.screenshot({ path: testInfo.outputPath("timezone-dialog-ru-narrow.png") });
});

test("Excel credentials show local expiry and recovery without changing pool membership", async ({ page }) => {
  await installTauriMock(page, {
    mode: "local", locale: "en", populated: true, accountCount: 1,
    accountOAuthClientKind: "excel_bps",
    accountCredentialDiagnostics: {
      status: "expired", expiresAtMs: 1, hasRefreshToken: true, recovery: "refresh_next_request",
    },
  });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await expect(page.getByText("BPS authorization: Expired or rejected", { exact: true })).toBeVisible();
  await expect(page.getByText("A refresh token is saved. Relay will refresh before the next request.", { exact: true })).toBeVisible();
  await expect(page.getByText("Local token state. Expiry does not confirm upstream authentication.", { exact: true })).toBeVisible();
});
