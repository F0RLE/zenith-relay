import { expect, test } from "../bun-playwright";
import type { RuntimeSnapshot } from "../../src/features/relay/api/types";
import { installTauriMock } from "./tauri-mock";
import { openPoolRotation } from "./pool-rotation";

for (const theme of ["light", "dark"] as const) {
  test(`${theme} selected rotation modes use the preset fill with only a header close button`, async ({ page }) => {
    await installTauriMock(page, { theme, locale: "en", quotaAvailable: true });
    await page.goto("/");
    const dialog = await openPoolRotation(page);
    await expect(dialog.locator("footer")).toHaveCount(0);
    await expect(dialog.getByRole("button", { name: "Close", exact: true })).toHaveCount(1);
    await expect(dialog.locator("header").getByRole("button", { name: "Close", exact: true })).toBeVisible();
    const selectedFill = await dialog.getByRole("button", { name: "Recommended", exact: true }).evaluate(async (button) => {
      await Promise.all(button.getAnimations().map((animation) => animation.finished));
      return getComputedStyle(button).backgroundColor;
    });
    const automatic = dialog.getByRole("radio", { name: "Automatic", exact: true });
    await expect(automatic).toHaveAttribute("aria-checked", "true");
    await expect(automatic).toHaveCSS("background-color", selectedFill);
    for (const name of ["Round robin", "Primary & reserve", "Automatic"]) {
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

  test(`${theme} rotation presets keep neutral borders on hover and selection`, async ({ page }) => {
    await installTauriMock(page, { theme, locale: "en", quotaAvailable: true });
    await page.goto("/");
    const dialog = await openPoolRotation(page);
    const preset = dialog.getByRole("button", { name: "Even distribution", exact: true });
    const neutralBorder = await dialog.locator(".pool-routing-presets").evaluate((section) => getComputedStyle(section).borderRightColor);
    const before = await preset.evaluate((button) => {
      const style = getComputedStyle(button);
      return { border: style.borderTopColor, background: style.backgroundColor };
    });
    expect([neutralBorder, "rgba(0, 0, 0, 0)"]).toContain(before.border);
    await preset.hover();
    await expect(preset).toHaveCSS("border-top-color", neutralBorder);
    await preset.click();
    await expect(preset).toHaveAttribute("aria-pressed", "true");
    await expect(preset).not.toHaveCSS("background-color", before.background);
    const selectedBackground = await preset.evaluate(async (button) => {
      await Promise.all(button.getAnimations().map((animation) => animation.finished));
      return getComputedStyle(button).backgroundColor;
    });
    for (const hovered of [true, false]) {
      if (!hovered) await dialog.getByRole("heading", { name: "Pool rotation", exact: true }).hover();
      await expect(preset).toHaveCSS("border-top-color", "rgba(0, 0, 0, 0)");
      await expect(preset).toHaveCSS("background-color", selectedBackground);
    }
  });
}

for (const mode of ["local"] as const) {
  test(`${mode} recommended preset restores defaults without changing membership or restarting the gateway`, async ({ page }) => {
    await installTauriMock(page, { mode, locale: "en", accountCount: 2, sourceCount: 2, quotaAvailable: true });
    await page.goto("/");
    const dialog = await openPoolRotation(page);
    const recommended = dialog.getByRole("button", { name: "Recommended", exact: true });
    await expect(recommended).toHaveAttribute("aria-pressed", "true");
    await expect(dialog.getByRole("button", { name: "Reset to defaults", exact: true })).toHaveCount(0);
    await dialog.getByLabel("Request share: Personal Plus", { exact: true }).fill("17");
    await dialog.getByLabel("Concurrent requests: Personal Plus", { exact: true }).fill("2");
    await dialog.getByRole("radio", { name: "Primary & reserve", exact: true }).click();
    await expect(dialog.locator(".pool-routing-editor")).toHaveAttribute("aria-busy", "false");
    const readRuntime = () => page.evaluate(async () => (window as unknown as { __TAURI_INTERNALS__: { invoke: (command: string) => Promise<RuntimeSnapshot> } }).__TAURI_INTERNALS__.invoke("get_local_runtime_state"));
    const before = await readRuntime();
    expect(before.gateway.poolRouting?.members.find((member) => member.id === "account_synthetic")).toMatchObject({ weight: 17, maxConcurrency: 2 });
    await recommended.click();
    await expect(dialog.getByRole("radio", { name: "Automatic", exact: true })).toHaveAttribute("aria-checked", "true");
    await expect(dialog.getByRole("button", { name: "Recommended", exact: true })).toHaveAttribute("aria-pressed", "true");
    await expect(dialog.locator(".pool-routing-custom")).toHaveCount(0);
    await expect(dialog.getByLabel("Request share: Personal Plus", { exact: true })).toHaveValue("1");
    await expect(dialog.getByLabel("Concurrent requests: Personal Plus", { exact: true })).toHaveValue("");
    await expect(dialog.locator(".pool-routing-editor")).toHaveAttribute("aria-busy", "false");
    const after = await readRuntime();
    expect(after.gateway.poolRouting).toEqual({ ...before.gateway.poolRouting, mode: "automatic", members: before.gateway.poolRouting!.members.map((member) => ({ ...member, weight: 1, maxConcurrency: 0 })) });
    expect(after.accounts).toEqual(before.accounts);
    expect(after.sources).toEqual(before.sources);
    expect(after.gateway.maxRetryCandidates).toBe(before.gateway.maxRetryCandidates);
    expect(after.gateway.defaultServiceTier).toBe(before.gateway.defaultServiceTier);
    expect(after.gateway.running).toBe(true);
    await dialog.getByRole("button", { name: "Close", exact: true }).click();
    await page.getByRole("button", { name: "Pool rotation settings", exact: true }).click();
    await expect(dialog.getByRole("button", { name: "Recommended", exact: true })).toHaveAttribute("aria-pressed", "true");
  });

  test(`${mode} accounts-first preset restores controls and keeps API as an explicit reserve`, async ({ page }) => {
    await installTauriMock(page, { mode, locale: "en", accountCount: 2, sourceCount: 2, quotaAvailable: true });
    await page.goto("/");
    const dialog = await openPoolRotation(page);
    await expect(dialog.getByRole("radio")).toHaveCount(3);
    const account = dialog.locator('[data-member-id="account:account_synthetic"]');
    await account.getByLabel("Request share: Personal Plus", { exact: true }).fill("17");
    await account.getByLabel("Concurrent requests: Personal Plus", { exact: true }).fill("2");
    await expect(dialog.locator(".pool-routing-custom")).toHaveText("Custom");
    const presets = dialog.getByRole("group", { name: "Preset", exact: true });
    await expect(presets.getByRole("button")).toHaveCount(3);
    await presets.getByRole("button", { name: "Accounts → API", exact: true }).click();
    await expect(dialog.getByRole("radio", { name: "Primary & reserve", exact: true })).toHaveAttribute("aria-checked", "true");
    await expect(account.getByLabel("Concurrent requests: Personal Plus", { exact: true })).toHaveValue("");
    await expect(presets.getByRole("button", { name: "Accounts → API", exact: true })).toHaveAttribute("aria-pressed", "true");
    await expect(dialog.locator(".pool-routing-custom")).toHaveCount(0);
    await expect(dialog.getByLabel("Request share: Personal Plus", { exact: true })).toHaveCount(0);
    await expect(dialog.locator(".pool-routing-description")).toHaveCount(0);
    await dialog.getByRole("button", { name: "Close", exact: true }).last().click();
    const runtime = await page.evaluate(async () => (window as unknown as { __TAURI_INTERNALS__: { invoke: (command: string) => Promise<RuntimeSnapshot> } }).__TAURI_INTERNALS__.invoke("get_local_runtime_state"));
    expect(runtime.gateway.poolRouting).toMatchObject({ version: 3, mode: "failover" });
    expect(runtime.gateway.poolRouting?.members.map((member) => member.kind)).toEqual(["account", "account", "source", "source"]);
    expect(runtime.gateway.poolRouting?.members.every((member) => member.weight === 1 && member.maxConcurrency === 0)).toBe(true);
    expect(runtime.gateway.running).toBe(true);
    await expect(page.getByRole("group", { name: "Connection priority", exact: true })).toBeVisible();
  });

  test(`${mode} round robin can enable request shares without losing them when disabled`, async ({ page }) => {
    await installTauriMock(page, { mode, locale: "en", quotaAvailable: true });
    await page.goto("/");
    const dialog = await openPoolRotation(page);
    await dialog.getByRole("radio", { name: "Round robin", exact: true }).click();
    const weighted = dialog.getByRole("checkbox", { name: "Use custom request shares", exact: true });
    await expect(weighted).not.toBeChecked();
    await weighted.check();
    const share = dialog.getByLabel("Request share: Personal Plus", { exact: true });
    await share.fill("23");
    const limit = dialog.getByLabel("Concurrent requests: Personal Plus", { exact: true });
    await limit.fill("2");
    await weighted.uncheck();
    await expect(share).toHaveCount(0);
    await weighted.check();
    await expect(share).toHaveValue("23");
    await expect(limit).toHaveValue("2");
    await dialog.getByRole("button", { name: "Close", exact: true }).last().click();
    await page.getByRole("button", { name: "Pool rotation settings", exact: true }).click();
    await expect(weighted).toBeChecked();
    await expect(share).toHaveValue("23");
    await expect(limit).toHaveValue("2");
    await dialog.getByRole("button", { name: "Even distribution", exact: true }).click();
    await expect(weighted).not.toBeChecked();
    await expect(limit).toHaveValue("");
    await expect(dialog.getByRole("button", { name: "Even distribution", exact: true })).toHaveAttribute("aria-pressed", "true");
    await dialog.getByRole("button", { name: "Close", exact: true }).click();
    const runtime = await page.evaluate(async () => (window as unknown as { __TAURI_INTERNALS__: { invoke: (command: string) => Promise<RuntimeSnapshot> } }).__TAURI_INTERNALS__.invoke("get_local_runtime_state"));
    expect(runtime.gateway.poolRouting?.members.every((member) => member.weight === 1 && member.maxConcurrency === 0)).toBe(true);
  });
}


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

test("Russian preset selection restores defaults without an extra reset action", async ({ page }) => {
  await page.setViewportSize({ width: 840, height: 560 });
  await installTauriMock(page, { locale: "ru", theme: "dark", quotaAvailable: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Пул", exact: true }).click();
  await page.getByRole("button", { name: "Настройки ротации пула", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "Ротация пула", exact: true });
  const recommended = dialog.getByRole("button", { name: "Рекомендуемый", exact: true });
  await expect(dialog.getByRole("button", { name: "По умолчанию", exact: true })).toHaveCount(0);
  await dialog.getByLabel("Доля запросов: Personal Plus", { exact: true }).fill("17");
  await dialog.getByLabel("Одновременных запросов: Personal Plus", { exact: true }).fill("2");
  await expect(dialog.locator(".pool-routing-custom")).toHaveText("Свой");
  await recommended.click();
  await expect(recommended).toHaveAttribute("aria-pressed", "true");
  await expect(dialog.locator(".pool-routing-custom")).toHaveCount(0);
  await expect(dialog.getByLabel("Доля запросов: Personal Plus", { exact: true })).toHaveValue("1");
  await expect(dialog.getByLabel("Одновременных запросов: Personal Plus", { exact: true })).toHaveValue("");
  await page.screenshot({ path: "output/playwright/pool-rotation-presets-ru-dark-840x560.png" });
});
