import { expect, test } from "../bun-playwright";
import type { RuntimeSnapshot } from "../../src/features/relay/api/types";
import { installTauriMock } from "./tauri-mock";
import { openPoolRotation } from "./pool-rotation";

for (const theme of ["light", "dark"] as const) {
  test(`${theme} rotation modes share the selected fill in the strategy sidebar`, async ({ page }) => {
    await installTauriMock(page, { theme, locale: "en", quotaAvailable: true });
    await page.goto("/");
    const dialog = await openPoolRotation(page);
    await expect(dialog.locator("footer")).toHaveCount(0);
    await expect(dialog.locator("aside")).toHaveCount(1);
    await expect(dialog.getByRole("radiogroup")).toHaveAttribute("aria-orientation", "vertical");
    await expect(dialog.getByRole("group", { name: "Preset", exact: true })).toHaveCount(0);
    await expect(dialog.getByRole("button", { name: "Close", exact: true })).toHaveCount(1);
    await expect(dialog.locator("header").getByRole("button", { name: "Close", exact: true })).toBeVisible();
    const automatic = dialog.getByRole("radio", { name: "Automatic", exact: true });
    await expect(automatic).toHaveAttribute("aria-checked", "true");
    const selectedFill = await automatic.evaluate(async (button) => {
      await Promise.all(button.getAnimations().map((animation) => animation.finished));
      return getComputedStyle(button).backgroundColor;
    });
    expect(selectedFill).not.toBe("rgba(0, 0, 0, 0)");
    for (const name of ["Expiring first", "Round robin", "Primary & reserve", "Automatic"]) {
      const option = dialog.getByRole("radio", { name, exact: true });
      await option.click();
      await expect(option).toHaveAttribute("aria-checked", "true");
      await expect(option).toHaveCSS("background-color", selectedFill);
      await dialog.getByRole("heading", { name: "Pool rotation", exact: true }).hover();
      await expect(option).toHaveCSS("background-color", selectedFill);
      await expect(option).toHaveCSS("border-top-color", "rgba(0, 0, 0, 0)");
    }
    await dialog.getByRole("button", { name: "Close", exact: true }).click();
    await expect(dialog).toBeHidden();
  });

}

for (const mode of ["local"] as const) {
  test(`${mode} strategy changes preserve controls and membership without restarting the gateway`, async ({ page }) => {
    await installTauriMock(page, { mode, locale: "en", accountCount: 2, sourceCount: 2, quotaAvailable: true });
    await page.goto("/");
    const dialog = await openPoolRotation(page);
    const automatic = dialog.getByRole("radio", { name: "Automatic", exact: true });
    await expect(automatic).toHaveAttribute("aria-checked", "true");
    await expect(dialog.getByRole("button", { name: "Reset to defaults", exact: true })).toHaveCount(0);
    await dialog.getByLabel("Request share: Personal Plus", { exact: true }).fill("17");
    await dialog.getByLabel("Concurrent requests: Personal Plus", { exact: true }).fill("2");
    await dialog.getByRole("radio", { name: "Expiring first", exact: true }).click();
    await expect(dialog.getByRole("radio", { name: "Expiring first", exact: true })).toHaveAttribute("aria-checked", "true");
    await expect(dialog.getByRole("button", { name: /Move .* (up|down)/ })).toHaveCount(0);
    await expect(dialog.getByLabel("Request share: Personal Plus", { exact: true })).toHaveValue("17");
    await dialog.getByRole("radio", { name: "Primary & reserve", exact: true }).click();
    await expect(dialog.locator(".pool-routing-editor")).toHaveAttribute("aria-busy", "false");
    const readRuntime = () => page.evaluate(async () => (window as unknown as { __TAURI_INTERNALS__: { invoke: (command: string) => Promise<RuntimeSnapshot> } }).__TAURI_INTERNALS__.invoke("get_local_runtime_state"));
    const before = await readRuntime();
    expect(before.gateway.poolRouting?.members.find((member) => member.id === "account_synthetic")).toMatchObject({ weight: 17, maxConcurrency: 2 });
    await automatic.click();
    await expect(dialog.getByRole("radio", { name: "Automatic", exact: true })).toHaveAttribute("aria-checked", "true");
    await expect(dialog.getByLabel("Request share: Personal Plus", { exact: true })).toHaveValue("17");
    await expect(dialog.getByLabel("Concurrent requests: Personal Plus", { exact: true })).toHaveValue("2");
    await expect(dialog.locator(".pool-routing-editor")).toHaveAttribute("aria-busy", "false");
    const after = await readRuntime();
    expect(after.gateway.poolRouting).toEqual({ ...before.gateway.poolRouting, mode: "automatic" });
    expect(after.accounts).toEqual(before.accounts);
    expect(after.sources).toEqual(before.sources);
    expect(after.gateway.maxRetryCandidates).toBe(before.gateway.maxRetryCandidates);
    expect(after.gateway.defaultServiceTier).toBe(before.gateway.defaultServiceTier);
    expect(after.gateway.running).toBe(true);
    await dialog.getByRole("button", { name: "Close", exact: true }).click();
    await page.getByRole("button", { name: "Pool rotation settings", exact: true }).click();
    await expect(automatic).toHaveAttribute("aria-checked", "true");
    await expect(dialog.getByLabel("Request share: Personal Plus", { exact: true })).toHaveValue("17");
    await expect(dialog.getByLabel("Concurrent requests: Personal Plus", { exact: true })).toHaveValue("2");
  });

  test(`${mode} round robin can enable request shares without losing them when disabled`, async ({ page }) => {
    await installTauriMock(page, { mode, locale: "en", quotaAvailable: true });
    await page.goto("/");
    const dialog = await openPoolRotation(page);
    await dialog.getByRole("radio", { name: "Round robin", exact: true }).click();
    const weighted = dialog.getByRole("checkbox", { name: "Use custom request shares", exact: true });
    await expect(weighted).not.toBeChecked();
    await weighted.click();
    await expect(weighted).toBeChecked();
    const share = dialog.getByLabel("Request share: Personal Plus", { exact: true });
    await share.fill("23");
    const limit = dialog.getByLabel("Concurrent requests: Personal Plus", { exact: true });
    await limit.fill("2");
    await weighted.click();
    await expect(weighted).not.toBeChecked();
    await expect(share).toHaveCount(0);
    await weighted.click();
    await expect(weighted).toBeChecked();
    await expect(share).toHaveValue("23");
    await expect(limit).toHaveValue("2");
    await dialog.getByRole("button", { name: "Close", exact: true }).last().click();
    await page.getByRole("button", { name: "Pool rotation settings", exact: true }).click();
    await expect(weighted).toBeChecked();
    await expect(share).toHaveValue("23");
    await expect(limit).toHaveValue("2");
    await weighted.click();
    await expect(weighted).not.toBeChecked();
    await expect(limit).toHaveValue("2");
    await dialog.getByRole("button", { name: "Close", exact: true }).click();
    const runtime = await page.evaluate(async () => (window as unknown as { __TAURI_INTERNALS__: { invoke: (command: string) => Promise<RuntimeSnapshot> } }).__TAURI_INTERNALS__.invoke("get_local_runtime_state"));
    expect(runtime.gateway.poolRouting?.mode).toBe("round_robin");
    expect(runtime.gateway.poolRouting?.members.find((member) => member.id === "account_synthetic")).toMatchObject({ weight: 23, maxConcurrency: 2 });
  });
}


test("expiry strategy persists and status icons replace duplicated status text", async ({ page }) => {
  await installTauriMock(page, { locale: "en", quotaAvailable: true });
  await page.goto("/");
  const dialog = await openPoolRotation(page);
  await dialog.getByRole("radio", { name: "Expiring first", exact: true }).click();
  await expect(dialog.getByRole("radio", { name: "Expiring first", exact: true })).toHaveAttribute("aria-checked", "true");
  const row = dialog.locator('[data-member-id="account:account_synthetic"]');
  await expect(row.getByRole("img", { name: "In rotation", exact: true })).toHaveAttribute("data-status", "ready");
  await expect(row.getByText("In rotation", { exact: true })).toHaveCount(0);
  await expect(row.getByRole("spinbutton").first()).toHaveCSS("border-top-width", "0px");
  await dialog.getByRole("button", { name: "Close", exact: true }).click();
  const state = await page.evaluate(async () => (window as unknown as { __TAURI_INTERNALS__: { invoke: (command: string) => Promise<RuntimeSnapshot> } }).__TAURI_INTERNALS__.invoke("get_local_runtime_state"));
  expect(state.gateway.poolRouting?.mode).toBe("subscription_expiry");
  await page.getByRole("button", { name: "Pool rotation settings", exact: true }).click();
  await expect(dialog.getByRole("radio", { name: "Expiring first", exact: true })).toHaveAttribute("aria-checked", "true");
});

for (const locale of ["en", "ru"] as const) {
  test(`${locale} rotation keeps explanations in contextual Help and restores focus`, async ({ page }) => {
    await installTauriMock(page, { locale, quotaAvailable: true });
    await page.goto("/");
    await page.getByRole("button", { name: locale === "ru" ? "Пул" : "Pool", exact: true }).click();
    await page.getByRole("button", { name: locale === "ru" ? "Настройки ротации пула" : "Pool rotation settings", exact: true }).click();
    const dialog = page.getByRole("dialog", { name: locale === "ru" ? "Ротация пула" : "Pool rotation", exact: true });
    const title = locale === "ru" ? "Как работает ротация" : "How pool rotation works";
    const helpButton = dialog.getByRole("button", { name: title, exact: true });
    await expect(dialog.locator(".pool-routing-description")).toHaveCount(0);
    await helpButton.click();
    const help = page.getByRole("dialog", { name: title, exact: true });
    await expect(help).toContainText(locale === "ru" ? "могут расходовать его баланс" : "Reserve API requests may spend");
    await expect(help).toContainText(locale === "ru" ? "Доли задают относительное число запросов" : "Shares target relative request counts");
    await expect(help).not.toContainText("relay:topic");
    await page.keyboard.press("Escape");
    await expect(help).toHaveCount(0);
    await expect(dialog).toBeVisible();
    await expect(helpButton).toBeFocused();
  });
}

test("Russian strategy selection keeps member settings without preset controls", async ({ page }, testInfo) => {
  await page.setViewportSize({ width: 840, height: 560 });
  await installTauriMock(page, { locale: "ru", theme: "dark", quotaAvailable: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Пул", exact: true }).click();
  await page.getByRole("button", { name: "Настройки ротации пула", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "Ротация пула", exact: true });
  const automatic = dialog.getByRole("radio", { name: "Автоматически", exact: true });
  await expect(dialog.getByRole("group", { name: "Пресет", exact: true })).toHaveCount(0);
  await expect(dialog.getByRole("button", { name: "По умолчанию", exact: true })).toHaveCount(0);
  await dialog.getByLabel("Доля запросов: Personal Plus", { exact: true }).fill("17");
  await dialog.getByLabel("Одновременных запросов: Personal Plus", { exact: true }).fill("2");
  await dialog.getByRole("radio", { name: "По сроку подписки", exact: true }).click();
  await expect(dialog.getByRole("radio", { name: "По сроку подписки", exact: true })).toHaveAttribute("aria-checked", "true");
  await dialog.getByRole("radio", { name: "По кругу", exact: true }).click();
  await automatic.click();
  await expect(automatic).toHaveAttribute("aria-checked", "true");
  await expect(dialog.getByLabel("Доля запросов: Personal Plus", { exact: true })).toHaveValue("17");
  await expect(dialog.getByLabel("Одновременных запросов: Personal Plus", { exact: true })).toHaveValue("2");
  await page.screenshot({ path: testInfo.outputPath("rotation-settings-ru.png"), animations: "disabled" });
});
