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

for (const mode of ["local", "remote"] as const) {
  test(`${mode} rotation presets preserve controls and keep API as an explicit reserve`, async ({ page }) => {
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
    await expect(account.getByLabel("Concurrent requests: Personal Plus", { exact: true })).toHaveValue("2");
    await expect(dialog.getByLabel("Request share: Personal Plus", { exact: true })).toHaveCount(0);
    await expect(dialog.locator(".pool-routing-description")).toHaveCount(0);
    await dialog.getByRole("button", { name: "Close", exact: true }).last().click();
    const runtime = await page.evaluate(async (mode) => (window as unknown as { __TAURI_INTERNALS__: { invoke: (command: string) => Promise<RuntimeSnapshot> } }).__TAURI_INTERNALS__.invoke(mode === "local" ? "get_local_runtime_state" : "get_remote_server_state"), mode);
    expect(runtime.gateway.poolRouting).toMatchObject({ version: 3, mode: "failover" });
    expect(runtime.gateway.poolRouting?.members.map((member) => member.kind)).toEqual(["account", "account", "source", "source"]);
    expect(runtime.gateway.poolRouting?.members.find((member) => member.id === "account_synthetic")).toMatchObject({ weight: 17, maxConcurrency: 2 });
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
    await weighted.uncheck();
    await expect(share).toHaveCount(0);
    await weighted.check();
    await expect(share).toHaveValue("23");
    await dialog.getByRole("button", { name: "Close", exact: true }).last().click();
    await page.getByRole("button", { name: "Pool rotation settings", exact: true }).click();
    await expect(weighted).toBeChecked();
    await expect(share).toHaveValue("23");
  });
}

test("version two servers keep cycling controls without receiving version three choices", async ({ page }) => {
  await installTauriMock(page, { mode: "remote", locale: "en", rotationVersion: 2, remoteFeatures: ["sources", "accounts", "runtime_routing", "rotation_v2"] });
  await page.goto("/");
  const dialog = await openPoolRotation(page);
  await expect(dialog.getByRole("radio")).toHaveCount(2);
  await expect(dialog.getByRole("group", { name: "Preset", exact: true })).toHaveCount(0);
  await dialog.getByRole("radio", { name: "Round robin", exact: true }).click();
  await expect(dialog.getByRole("checkbox")).toHaveCount(0);
  await dialog.getByRole("button", { name: "Close", exact: true }).last().click();
  const runtime = await page.evaluate(async () => (window as unknown as { __TAURI_INTERNALS__: { invoke: (command: string) => Promise<RuntimeSnapshot> } }).__TAURI_INTERNALS__.invoke("get_remote_server_state"));
  expect(runtime.gateway.poolRouting).toMatchObject({ version: 2, mode: "round_robin" });
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
