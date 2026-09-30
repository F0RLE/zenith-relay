import { expect, test } from "../../bun-playwright";
import { installTauriMock } from "../tauri-mock";

for (const guide of [
  { locale: "en", help: "Help", contents: "On this page", pool: "Pool", errors: "Errors", quickSetup: "Repeat quick setup", start: "Get started" },
  { locale: "ru", help: "Помощь", contents: "Содержание", pool: "Пул", errors: "Ошибки", quickSetup: "Повторить быструю настройку", start: "Приступить" },
] as const) {
  for (const width of [1160, 840, 430]) {
    test(`Help navigation and explicit quick setup: ${guide.locale}, ${width}px`, async ({ page }, testInfo) => {
      await page.setViewportSize({ width, height: 760 });
      await installTauriMock(page, { mode: "local", locale: guide.locale, populated: true });
      await page.goto("/");
      await page.getByRole("button", { name: guide.help, exact: true }).click();
      await expect(page.getByRole("heading", { name: guide.help, exact: true })).toBeVisible();
      const document = page.locator(".help-document");
      await expect(document).toBeVisible();
      const navigation = page.getByRole("navigation", { name: guide.contents });
      const toggle = navigation.getByRole("button", { name: guide.contents });
      const openContents = async () => {
        if (await toggle.isVisible() && await toggle.getAttribute("aria-expanded") === "false") await toggle.click();
      };
      await openContents();
      const links = navigation.getByRole("link");
      expect(await links.count()).toBeGreaterThan(0);
      expect(await links.evaluateAll((items) => items.every((item) => {
        const href = item.getAttribute("href");
        const target = href?.startsWith("#") ? window.document.getElementById(decodeURIComponent(href.slice(1))) : null;
        return target?.matches(".help-document h2, .help-document h3");
      }))).toBe(true);
      if (await toggle.isVisible()) {
        await toggle.press("Escape");
        await expect(toggle).toHaveAttribute("aria-expanded", "false");
        await expect(toggle).toBeFocused();
      }
      await page.screenshot({ path: testInfo.outputPath("help-start.png") });
      await openContents();
      await navigation.getByRole("link", { name: guide.pool, exact: true }).click();
      const poolHeading = document.getByRole("heading", { name: `3. ${guide.pool}`, exact: true });
      await expect(poolHeading).toBeInViewport();
      await expect(poolHeading).toBeFocused();
      await expect(navigation.locator("a[aria-current]")).toHaveText(guide.pool);
      await openContents();
      await navigation.getByRole("link", { name: guide.errors, exact: true }).focus();
      await page.keyboard.press("Enter");
      const errorsHeading = document.getByRole("heading", { name: `8. ${guide.errors}`, exact: true });
      await expect(errorsHeading).toBeInViewport();
      await expect(errorsHeading).toBeFocused();
      await expect(navigation.locator("a[aria-current]")).toHaveText(guide.errors);
      if (await toggle.isVisible()) await expect(toggle).toHaveAttribute("aria-expanded", "false");
      expect(await page.evaluate(() => window.document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
      expect(await document.locator("table, td").evaluateAll((cells) => cells.every((cell) => cell.scrollWidth <= cell.clientWidth))).toBe(true);
      await page.screenshot({ path: testInfo.outputPath("help-errors.png") });
      const search = document.getByRole("searchbox");
      await search.fill("NO_ELIGIBLE_SOURCE");
      const group = document.locator(".help-error-group");
      await expect(group).toHaveCount(1);
      await expect(group).toHaveAttribute("open", "");
      await expect(group.locator("tbody tr")).toHaveCount(1);
      await expect(group.locator("td").last()).toContainText(guide.locale === "ru" ? "Пуле" : "Pool");
      await search.fill("refresh_token_reused");
      await expect(group).toHaveCount(1);
      await expect(group.locator("tbody tr")).toHaveCount(1);
      await expect(group).toContainText(guide.locale === "ru" ? "гонка" : "concurrent");
      await search.fill("synthetic_unknown_error_9284");
      await expect(group).toHaveCount(0);
      await expect(document.getByRole("status")).toContainText(guide.locale === "ru" ? "Совпадений нет" : "No matches");
      await document.getByRole("button", { name: guide.locale === "ru" ? "Очистить поиск" : "Clear search" }).click();
      await expect(search).toBeFocused();
      await expect(search).toHaveValue("");
      await expect(document.locator(".help-error-group[open]")).toHaveCount(0);
      await document.locator(".help-error-group summary").first().focus();
      await page.keyboard.press("Enter");
      await expect(document.locator(".help-error-group[open]")).toHaveCount(1);
      expect(await document.locator("table, td").evaluateAll((cells) => cells.every((cell) => cell.scrollWidth <= cell.clientWidth))).toBe(true);
      await page.getByRole("button", { name: guide.quickSetup }).click();
      await expect(page.getByRole("button", { name: guide.start, exact: true })).toBeVisible();
    });
  }
}

test("Help retries a failed local guide load", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true });
  let failed = false;
  await page.route(/README.*\.md(?:\?.*)?$/, (route) => {
    if (!failed) { failed = true; return route.fulfill({ status: 503, body: "Unavailable" }); }
    return route.continue();
  });
  await page.goto("/");
  await page.getByRole("button", { name: "Help", exact: true }).click();
  await expect(page.getByRole("alert")).toContainText("Help could not be opened");
  await page.getByRole("button", { name: "Retry", exact: true }).click();
  await expect(page.locator(".help-document")).toBeVisible();
  await expect(page.getByRole("navigation", { name: "On this page" })).toBeVisible();
});

test("updates are checked without downloading and require an explicit action", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, updateVersion: "1.1.0", updateBody: "Faster parallel routing\nUpdated settings" });
  await page.goto("/");

  const updateButton = page.getByRole("button", { name: "Open update 1.1.0" });
  await expect(updateButton).toBeVisible();
  let commands = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string }> }).__TAURI_TEST_INVOKES__.map((call) => call.command));
  expect(commands).toContain("plugin:updater|check");
  expect(commands).not.toContain("plugin:updater|download_and_install");

  await updateButton.click();
  let dialog = page.getByRole("dialog", { name: "Update 1.1.0" });
  await expect(dialog).toContainText("Faster parallel routing");
  await dialog.getByRole("button", { name: "Skip 1.1.0" }).click();
  await expect(updateButton).toHaveCount(0);
  await expect.poll(() => page.evaluate(() => localStorage.getItem("relay.skippedUpdate"))).toBe("1.1.0");

  await page.getByRole("button", { name: "Settings", exact: true }).click();
  await page.locator(".settings-group").filter({ hasText: "Application" }).getByRole("button", { name: "Check" }).click();
  dialog = page.getByRole("dialog", { name: "Update 1.1.0" });
  await expect(dialog).toBeVisible();
  await dialog.getByRole("button", { name: "Update", exact: true }).click();
  await expect.poll(() => page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string }> }).__TAURI_TEST_INVOKES__.map((call) => call.command))).toEqual(expect.arrayContaining(["plugin:updater|download_and_install", "plugin:process|restart"]));
  commands = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string }> }).__TAURI_TEST_INVOKES__.map((call) => call.command));
  expect(commands.filter((command) => command === "plugin:updater|download_and_install")).toHaveLength(1);
});

test("updates are checked again when the window returns to the foreground", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, updateVersion: "1.1.1" });
  await page.goto("/");

  await expect(page.getByRole("button", { name: "Open update 1.1.1" })).toBeVisible();
  const before = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string }> }).__TAURI_TEST_INVOKES__.filter((call) => call.command === "plugin:updater|check").length);
  await page.evaluate(() => document.dispatchEvent(new Event("visibilitychange")));
  await expect.poll(() => page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string }> }).__TAURI_TEST_INVOKES__.filter((call) => call.command === "plugin:updater|check").length)).toBe(before + 1);
});

test("startup performs one update check without a deferred duplicate", async ({ page }) => {
  await installTauriMock(page, {
    mode: "local",
    locale: "en",
    populated: true,
    updateVersion: "1.1.1",
  });
  await page.goto("/");

  await expect(page.getByRole("button", { name: "Overview", exact: true })).toBeVisible();
  await expect.poll(() => page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string }> }).__TAURI_TEST_INVOKES__.filter((call) => call.command === "plugin:updater|check").length)).toBe(1);
  const checks = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string }> }).__TAURI_TEST_INVOKES__.filter((call) => call.command === "plugin:updater|check"));
  expect(checks).toHaveLength(1);
  await expect(page.getByRole("button", { name: "Open update 1.1.1" })).toBeVisible();
});

test("portable updates replace the same executable through the verified helper path", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, bundleType: null, updateVersion: "1.1.1", updateBody: "<!-- relay-notes:en -->\nPortable self-update\n<!-- relay-notes:ru -->\nСамообновление portable-версии" });
  await page.goto("/");

  await page.getByRole("button", { name: "Open update 1.1.1" }).click();
  const dialog = page.getByRole("dialog", { name: "Update 1.1.1" });
  await expect(dialog).toContainText("Portable self-update");
  await expect(dialog).not.toContainText("Самообновление portable-версии");
  await expect(dialog.getByRole("button", { name: "Skip 1.1.1", exact: true })).toBeVisible();
  await expect(dialog.getByRole("button", { name: "Update", exact: true })).toBeVisible();
  await expect(dialog.getByRole("button", { name: "Later", exact: true })).toHaveCount(0);
  await dialog.getByRole("button", { name: "Update", exact: true }).click();

  const calls = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: Record<string, unknown> }> }).__TAURI_TEST_INVOKES__);
  expect(calls.find((call) => call.command === "plugin:updater|check")?.args).toMatchObject({ target: "windows-x86_64-portable" });
  expect(calls.some((call) => call.command === "install_portable_update")).toBe(true);
  expect(calls.some((call) => call.command === "plugin:updater|download_and_install")).toBe(false);
  expect(calls.some((call) => call.command === "plugin:process|restart")).toBe(false);
});

test("a portable executable ignores a valid manifest without its portable target", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, bundleType: null, portableUpdateTargetMissing: true, updateVersion: "1.1.1" });
  await page.goto("/");

  await expect.poll(() => page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string }> }).__TAURI_TEST_INVOKES__.filter((call) => call.command === "plugin:updater|check").length)).toBe(2);
  await expect(page.getByRole("button", { name: "Open update 1.1.1" })).toHaveCount(0);

  const checks = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: Record<string, unknown> }> }).__TAURI_TEST_INVOKES__.filter((call) => call.command === "plugin:updater|check").map((call) => call.args));
  expect(checks).toEqual([{ target: "windows-x86_64-portable" }, {}]);
});

test("a portable updater check keeps a verification failure visible", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, bundleType: null, updateCheckError: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Settings", exact: true }).click();

  await expect(page.getByText("Update failed", { exact: true })).toBeVisible();
  const checks = await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string }> }).__TAURI_TEST_INVOKES__.filter((call) => call.command === "plugin:updater|check"));
  expect(checks).toHaveLength(1);
});
