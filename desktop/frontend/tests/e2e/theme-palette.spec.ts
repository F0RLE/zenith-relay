import { expect, test } from "../bun-playwright";
import { installTauriMock } from "./tauri-mock";

const palettes = [
  {
    name: "light",
    theme: "light",
    colorScheme: "light",
    colors: {
      "--relay-bg": "#f4f3ee",
      "--relay-surface": "#fffefa",
      "--relay-sidebar": "#f7f8f2",
      "--relay-border": "#dce1d8",
      "--relay-text": "#202b29",
      "--relay-muted": "#65716c",
      "--relay-accent": "#15644d",
      "--relay-accent-hover": "#104c3b",
      "--relay-accent-soft": "#e8f0e6",
      "--relay-on-accent": "#fffefa",
      "--relay-primary": "#15644d",
      "--relay-success": "#15644d",
      "--relay-danger": "#a52d2a",
    },
    appBackground: "rgb(244, 243, 238)",
    titlebarBackground: "rgb(247, 248, 242)",
  },
  {
    name: "dark",
    theme: "dark",
    colorScheme: "dark",
    colors: {
      "--relay-bg": "#141414",
      "--relay-surface": "#1b1b1b",
      "--relay-sidebar": "#171717",
      "--relay-border": "#333332",
      "--relay-text": "#f2f0eb",
      "--relay-muted": "#aaa9a2",
      "--relay-accent": "#80b49a",
      "--relay-accent-hover": "#a0ccb3",
      "--relay-accent-soft": "#20332a",
      "--relay-on-accent": "#102018",
      "--relay-primary": "#80b49a",
      "--relay-success": "#80b49a",
      "--relay-danger": "#ffa8a4",
    },
    appBackground: "rgb(20, 20, 20)",
    titlebarBackground: "rgb(23, 23, 23)",
  },
  {
    name: "system-dark",
    theme: "system",
    colorScheme: "dark",
    colors: {
      "--relay-bg": "#141414",
      "--relay-surface": "#1b1b1b",
      "--relay-sidebar": "#171717",
      "--relay-border": "#333332",
      "--relay-text": "#f2f0eb",
      "--relay-muted": "#aaa9a2",
      "--relay-accent": "#80b49a",
      "--relay-accent-hover": "#a0ccb3",
      "--relay-accent-soft": "#20332a",
      "--relay-on-accent": "#102018",
      "--relay-primary": "#80b49a",
      "--relay-success": "#80b49a",
      "--relay-danger": "#ffa8a4",
    },
    appBackground: "rgb(20, 20, 20)",
    titlebarBackground: "rgb(23, 23, 23)",
  },
  {
    name: "system-light",
    theme: "system",
    colorScheme: "light",
    colors: {
      "--relay-bg": "#f4f3ee",
      "--relay-surface": "#fffefa",
      "--relay-sidebar": "#f7f8f2",
      "--relay-border": "#dce1d8",
      "--relay-text": "#202b29",
      "--relay-muted": "#65716c",
      "--relay-accent": "#15644d",
      "--relay-accent-hover": "#104c3b",
      "--relay-accent-soft": "#e8f0e6",
      "--relay-on-accent": "#fffefa",
      "--relay-primary": "#15644d",
      "--relay-success": "#15644d",
      "--relay-danger": "#a52d2a",
    },
    appBackground: "rgb(244, 243, 238)",
    titlebarBackground: "rgb(247, 248, 242)",
  },
] as const;

for (const palette of palettes) {
  test(`Relay ${palette.name} theme uses the website palette`, async ({ page }, testInfo) => {
    if (palette.name === "system-dark") await page.emulateMedia({ colorScheme: "dark" });
    if (palette.name === "system-light") await page.emulateMedia({ colorScheme: "light" });
    await installTauriMock(page, {
      locale: "ru",
      mode: "local",
      theme: palette.theme,
      populated: true,
      accountCount: 6,
      quotaAvailable: true,
    });
    await page.setViewportSize({ width: 1160, height: 760 });
    await page.goto("/");
    await page.getByRole("button", { name: "Подключения", exact: true }).click();
    await expect(page.locator(".account-card").first()).toBeVisible();

    const renderedPalette = await page.evaluate(() => {
      const root = getComputedStyle(document.documentElement);
      return {
        colorScheme: root.colorScheme,
        colors: Object.fromEntries([
          "--relay-bg",
          "--relay-surface",
          "--relay-sidebar",
          "--relay-border",
          "--relay-text",
          "--relay-muted",
          "--relay-accent",
          "--relay-accent-hover",
          "--relay-accent-soft",
          "--relay-on-accent",
          "--relay-primary",
          "--relay-success",
          "--relay-danger",
        ].map((name) => [name, root.getPropertyValue(name).trim()])),
        appBackground: getComputedStyle(document.querySelector(".app")!).backgroundColor,
        titlebarBackground: getComputedStyle(document.querySelector(".titlebar")!).backgroundColor,
        primaryButtonText: getComputedStyle(document.querySelector(".relay-button.primary")!).color,
      };
    });

    expect(renderedPalette).toEqual({
      colorScheme: palette.colorScheme,
      colors: palette.colors,
      appBackground: palette.appBackground,
      titlebarBackground: palette.titlebarBackground,
      primaryButtonText: palette.name.includes("dark") ? "rgb(16, 32, 24)" : "rgb(255, 254, 250)",
    });
    await page.screenshot({ path: testInfo.outputPath(`connections-${palette.name}.png`) });
  });
}
