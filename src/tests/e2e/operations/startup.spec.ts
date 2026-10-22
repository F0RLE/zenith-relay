import { expect, test } from "../../bun-playwright";
import { installTauriMock } from "../tauri-mock";

const mainBundle = /\/assets\/main-[^/]+\.js$/;

for (const appearance of [
  { locale: "ru", theme: "dark", width: 840, height: 560 },
  { locale: "en", theme: "light", width: 1160, height: 760 },
] as const) {
  test(`loader paints before React in ${appearance.locale}/${appearance.theme}`, async ({ page }) => {
    await installTauriMock(page, { ...appearance, populated: true });
    await page.addInitScript((locale) => localStorage.setItem("relay.language", locale), appearance.locale);
    await page.setViewportSize({ width: appearance.width, height: appearance.height });
    let release!: () => void;
    const waiting = new Promise<void>((resolve) => { release = resolve; });
    await page.route(mainBundle, async (route) => { await waiting; await route.continue(); });
    try {
      await page.goto("/", { waitUntil: "domcontentloaded" });
      await expect(page.locator("#splash-message")).toHaveText(appearance.locale === "ru" ? "Подготовка интерфейса…" : "Preparing the interface…");
      await expect(page.locator("#root")).toHaveAttribute("inert", "");
      await expect(page.locator(".splash-logo")).toHaveJSProperty("naturalWidth", 512);
      await expect.poll(() => page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string }> }).__TAURI_TEST_INVOKES__.some((call) => call.command === "plugin:window|show"))).toBe(true);
      const box = await page.locator(".splash-content").boundingBox();
      expect(box).not.toBeNull();
      expect(Math.abs(box!.x + box!.width / 2 - appearance.width / 2)).toBeLessThan(1);
      expect(Math.abs(box!.y + box!.height / 2 - appearance.height / 2)).toBeLessThan(1);
      await page.screenshot({ path: `output/playwright/startup-${appearance.locale}-${appearance.theme}.png` });
    } finally {
      release();
    }
    await expect(page.locator("#splash-screen")).toHaveCount(0);
    await expect(page.locator("#root")).not.toHaveAttribute("inert");
    await expect(page.locator(".relay-shell")).toBeVisible();
  });
}

test("slow startup remains recoverable instead of revealing an unfinished workspace", async ({ page }) => {
  await page.clock.install();
  await installTauriMock(page, { locale: "ru", theme: "dark", runtimeStateDelayMs: 20_000 });
  await page.goto("/");
  await expect(page.locator("html")).toHaveAttribute("data-startup-state", "runtime");
  await page.clock.fastForward(11_000);
  await expect(page.locator("#splash-message")).toHaveText("Запуск занимает больше времени");
  await expect(page.getByRole("button", { name: "Перезагрузить", exact: true })).toBeVisible();
  await expect(page.locator(".app-workspace")).toHaveAttribute("inert", "");
  await page.locator(".window-controls .minimize").click();
  await expect.poll(() => page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string }> }).__TAURI_TEST_INVOKES__.some((call) => call.command === "plugin:window|minimize"))).toBe(true);
  await expect(page.locator("html")).not.toHaveAttribute("data-startup-ready", "true");
  await page.screenshot({ path: "output/playwright/startup-slow-ru-dark.png" });
  await page.clock.fastForward(10_000);
  await expect(page.locator("#splash-screen")).toHaveCount(0);
  await expect(page.locator(".relay-shell")).toBeVisible();
});

test("local snapshot failure is not recorded as successful startup", async ({ page }) => {
  await page.clock.install();
  await installTauriMock(page, { mode: "local", runtimeState: "error", locale: "en" });
  await page.goto("/");
  await expect(page.locator("#splash-message")).toHaveText("Relay could not start");
  await expect(page.getByRole("button", { name: "Reload", exact: true })).toBeVisible();
  await page.clock.fastForward(12_000);
  expect(await page.evaluate(() => performance.getEntriesByName("zenith:interactive", "measure"))).toEqual([]);
  await expect(page.locator(".app-workspace")).toHaveAttribute("inert", "");
});

test("a failed UI bundle can be reloaded without losing preferences", async ({ page }) => {
  await installTauriMock(page, { theme: "light", locale: "en" });
  await page.route(mainBundle, (route) => route.abort("failed"));
  await page.goto("/");
  await expect(page.locator("#splash-message")).toHaveText("Relay could not start");
  await page.screenshot({ path: "output/playwright/startup-error-en-light.png" });
  await page.evaluate(() => localStorage.setItem("relay.startup-test", "keep"));
  await page.unroute(mainBundle);
  await page.getByRole("button", { name: "Reload", exact: true }).click();
  await expect(page.locator("#splash-screen")).toHaveCount(0);
  expect(await page.evaluate(() => localStorage.getItem("relay.startup-test"))).toBe("keep");
});

test("render failure reveals the error boundary instead of hiding it behind the loader", async ({ page }) => {
  await installTauriMock(page, { locale: "en", runtimeState: "invalid" });
  await page.goto("/");
  await expect(page.locator(".relay-render-error")).toBeVisible();
  await expect(page.locator("#splash-screen")).toHaveCount(0);
  await expect(page.locator("#root")).not.toHaveAttribute("inert");
  await expect(page.getByRole("button", { name: "Reload", exact: true })).toBeEnabled();
});

test("migrating a saved legacy mode does not leave a stuck loader", async ({ page }) => {
  await page.emulateMedia({ reducedMotion: "reduce", colorScheme: "dark" });
  await installTauriMock(page, { storedMode: "remote", theme: "system" });
  await page.goto("/");
  await expect(page.locator("#splash-screen")).toHaveCount(0);
  await expect(page.locator("html")).toHaveAttribute("data-startup-ready", "true");
  await expect(page.locator("#root")).not.toHaveAttribute("inert");
});

test("secondary pages are loaded on navigation intent, not during startup", async ({ page }) => {
  const requested: string[] = [];
  page.on("request", (request) => requested.push(request.url()));
  await installTauriMock(page, { locale: "en" });
  await page.goto("/");
  await expect(page.locator("#splash-screen")).toHaveCount(0);
  const secondaryPage = /\/(?:ConnectionsPage|ImportDialog|PoolPage|IntegrationsPage|UsagePage|ProfilesPage|SettingsPage|HelpCenter)-/;
  expect(requested.filter((url) => secondaryPage.test(url))).toEqual([]);
  await page.getByRole("button", { name: "Pool", exact: true }).focus();
  await expect.poll(() => requested.some((url) => /\/PoolPage-/.test(url))).toBe(true);
  await page.getByRole("button", { name: "Pool", exact: true }).click();
  await expect(page.locator('.relay-shell[data-page="pool"] .relay-page')).toBeVisible();
});

test("first-run setup becomes usable after startup", async ({ page }) => {
  await installTauriMock(page, { onboarding: false, populated: false, locale: "en" });
  await page.goto("/");
  await expect(page.locator("#splash-screen")).toHaveCount(0);
  await expect(page.locator("#root")).not.toHaveAttribute("inert");
  await expect(page.locator(".setup-shell")).toBeVisible();
});
