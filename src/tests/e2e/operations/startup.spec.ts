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

test("slow runtime loading does not block navigation, settings or help", async ({ page }) => {
  await page.clock.install();
  await installTauriMock(page, { locale: "ru", theme: "dark", runtimeStateDelayMs: 20_000 });
  await page.goto("/");
  await expect(page.locator("#splash-screen")).toHaveCount(0);
  await expect(page.locator(".relay-content .data-placeholder")).toHaveCount(0);
  await expect(page.locator(".relay-content")).not.toContainText("Загрузка");
  expect(await page.evaluate(() => performance.getEntriesByName("zenith:interactive", "measure"))).toEqual([]);
  await page.getByRole("button", { name: "Настройки", exact: true }).click();
  await expect(page.locator(".settings-page")).toBeVisible();
  await page.getByRole("button", { name: "Помощь", exact: true }).click();
  await expect(page.locator('.relay-shell[data-page="help"] .relay-page')).toBeVisible();
  await page.getByRole("button", { name: "Пул", exact: true }).click();
  await expect(page.locator(".relay-content .data-placeholder")).toHaveCount(0);
  await expect(page.locator(".pool-controls")).toHaveCount(0);
  await page.screenshot({ path: "output/playwright/startup-runtime-pending-ru-dark.png" });
  await page.locator(".window-controls .minimize").click();
  await expect.poll(() => page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string }> }).__TAURI_TEST_INVOKES__.some((call) => call.command === "plugin:window|minimize"))).toBe(true);
  await page.clock.fastForward(20_000);
  await expect(page.locator(".pool-controls")).toBeVisible();
});

test("local snapshot failure is not recorded as successful startup", async ({ page }) => {
  await page.clock.install();
  await installTauriMock(page, { mode: "local", runtimeState: "error", locale: "en" });
  await page.goto("/");
  await expect(page.locator("#splash-screen")).toHaveCount(0);
  await expect(page.locator(".relay-content")).toContainText("Could not refresh the selected runtime.");
  await expect(page.getByRole("button", { name: "Retry", exact: true })).toBeVisible();
  await page.screenshot({ path: "output/playwright/startup-runtime-error-en-light.png" });
  await page.clock.fastForward(12_000);
  expect(await page.evaluate(() => performance.getEntriesByName("zenith:interactive", "measure"))).toEqual([]);
  await expect(page.locator("#root")).not.toHaveAttribute("inert");
  await page.getByRole("button", { name: "Settings", exact: true }).click();
  await expect(page.locator(".settings-page")).toBeVisible();
});

test("a failed initial snapshot can retry without reloading the renderer", async ({ page }) => {
  await installTauriMock(page, { locale: "en" });
  await page.addInitScript(() => {
    const internals = (window as unknown as {
      __TAURI_INTERNALS__: { invoke: (name: string, args?: unknown, options?: unknown) => Promise<unknown> };
    }).__TAURI_INTERNALS__;
    const invoke = internals.invoke.bind(internals);
    let fail = true;
    internals.invoke = (name, args, options) => {
      if (name === "get_local_runtime_state" && fail) {
        fail = false;
        return Promise.reject({ code: "invalid_state", message: "Synthetic initial snapshot failure" });
      }
      return invoke(name, args, options);
    };
  });
  await page.goto("/");
  await expect(page.getByRole("button", { name: "Retry", exact: true })).toBeVisible();
  await page.getByRole("button", { name: "Retry", exact: true }).click();
  await expect(page.getByRole("button", { name: "Stop API", exact: true })).toBeVisible();
  await expect.poll(() => page.evaluate(() => performance.getEntriesByName("zenith:interactive", "measure").length)).toBe(1);
});

test("startup does not retain an unused all-time Usage report", async ({ page }) => {
  await installTauriMock(page, { locale: "en" });
  await page.goto("/");
  await expect(page.getByRole("button", { name: "Stop API", exact: true })).toBeVisible();
  const usageReads = () => page.evaluate(() => (window as unknown as {
    __TAURI_TEST_INVOKES__: Array<{ command: string; args: { input?: { range?: string; includeEvents?: boolean } } }>;
  }).__TAURI_TEST_INVOKES__.filter(({ command }) => command === "get_local_usage_page").map(({ args }) => args.input));
  await expect.poll(async () => (await usageReads()).length).toBeGreaterThan(0);
  expect((await usageReads()).every((input) => input?.range === "custom")).toBe(true);
  await page.getByRole("button", { name: "Usage", exact: true }).click();
  await expect(page.locator(".usage-page")).toBeVisible();
  await expect.poll(async () => (await usageReads()).filter((input) => input?.range === undefined && input.includeEvents === true).length).toBe(1);
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

test("secondary code warms after readiness without mounting pages or preloading reports", async ({ page }) => {
  await page.clock.install();
  const requested: string[] = [];
  page.on("request", (request) => requested.push(request.url()));
  await installTauriMock(page, { locale: "en", runtimeStateDelayMs: 20_000 });
  await page.goto("/");
  await expect(page.locator("#splash-screen")).toHaveCount(0);
  const secondaryPage = /\/(?:ConnectionsPage|ImportDialog|PoolPage|IntegrationsPage|UsagePage|ProfilesPage|SettingsPage|HelpCenter)-/;
  expect(requested.filter((url) => secondaryPage.test(url))).toEqual([]);
  await page.getByRole("button", { name: "Pool", exact: true }).focus();
  await expect.poll(() => requested.some((url) => /\/PoolPage-/.test(url))).toBe(true);
  await page.getByRole("button", { name: "Pool", exact: true }).click();
  await page.clock.fastForward(20_000);
  await expect(page.locator('.relay-shell[data-page="pool"] .relay-page')).toBeVisible();
  await page.clock.fastForward(10_000);
  await expect.poll(() => requested.some((url) => /\/UsagePage-/.test(url))).toBe(true);
  expect(requested.some((url) => /\/ImportDialog-/.test(url))).toBe(false);
  const commands = await page.evaluate(() => (window as unknown as {
    __TAURI_TEST_INVOKES__: Array<{ command: string; args: { input?: { range?: string } } }>;
  }).__TAURI_TEST_INVOKES__);
  expect(commands.some(({ command }) => command === "get_local_proxy_pool")).toBe(false);
  expect(commands.some(({ command, args }) => command === "get_local_usage_page" && args.input?.range !== "custom")).toBe(false);
});

test("first-run setup becomes usable after startup", async ({ page }) => {
  await installTauriMock(page, { onboarding: false, populated: false, locale: "en" });
  await page.goto("/");
  await expect(page.locator("#splash-screen")).toHaveCount(0);
  await expect(page.locator("#root")).not.toHaveAttribute("inert");
  await expect(page.locator(".setup-shell")).toBeVisible();
});
