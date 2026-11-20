import { expect, test, type Page } from "../../bun-playwright";
import type { OAuthViewBounds, ProxyPoolSummary } from "../../../src/features/relay/api/types";
import { emitTauriEvent, installTauriMock } from "../tauri-mock";

function invokes(page: Page) {
  return page.evaluate(() => (window as unknown as {
    __TAURI_TEST_INVOKES__: Array<{ command: string; args: Record<string, unknown> }>;
  }).__TAURI_TEST_INVOKES__);
}

async function openSignIn(page: Page, locale: "en" | "ru" = "en") {
  const connections = locale === "ru" ? "Подключения" : "Connections";
  await page.goto("/");
  await page.getByRole("button", { name: connections, exact: true }).click();
  return showSignIn(page, locale);
}

async function showSignIn(page: Page, locale: "en" | "ru") {
  const signIn = locale === "ru" ? "Войти" : "Sign in";
  await page.getByRole("button", { name: signIn, exact: true }).first().click();
  return page.getByRole("dialog", { name: signIn, exact: true });
}

test.use({ deviceScaleFactor: 1.5 });

test("the native pane follows resize, yields to the proxy picker and hides on completion", async ({ page }) => {
  await installTauriMock(page, { populated: true, proxyCount: 2 });
  const dialog = await openSignIn(page);
  const panel = dialog.getByRole("region", { name: "Sign-in page" });
  const placement = async () => (await invokes(page)).findLast(({ command }) => command === "update_codex_oauth_view")?.args.bounds as OAuthViewBounds | null | undefined;
  await expect(panel).toHaveText("The sign-in page will appear here.");
  expect(await placement()).toBeUndefined();
  await dialog.getByRole("button", { name: "Open sign-in page" }).click();
  await expect(dialog.getByRole("button", { name: "Go to sign-in page" })).toBeVisible();
  const first = await panel.locator(".oauth-page-slot").boundingBox();
  expect(Math.abs((await placement())!.width - first!.width * 1.5)).toBeLessThan(2);
  expect(first!.x).toBeGreaterThan((await dialog.locator(".oauth-waiting").boundingBox())!.x);
  await page.setViewportSize({ width: 840, height: 560 });
  await expect.poll(async () => (await placement())?.width).toBeLessThan(first!.width * 1.5);
  const proxy = dialog.getByRole("button", { name: "Sign in with a proxy" });
  await proxy.hover();
  await expect(page.getByRole("tooltip")).toHaveText("Sign in with a proxy");
  await proxy.click();
  const picker = page.getByRole("dialog", { name: "Sign in with a proxy" });
  await expect(picker).toBeVisible();
  await expect.poll(placement).toBeNull();
  await picker.getByRole("button", { name: "Close", exact: true }).click();
  await expect.poll(placement).not.toBeNull();
  await emitTauriEvent(page, "relay-oauth-status", { loginId: "oauth_synthetic", status: "callback_received" });
  await expect(panel).toHaveCount(0);
  await expect.poll(placement).toBeNull();
});

test("changing the method after opening the form gives the new attempt a closed page", async ({ page }) => {
  await installTauriMock(page, { populated: true });
  await page.addInitScript(() => {
    const host = window as unknown as { __TAURI_INTERNALS__: { invoke: (command: string, args?: Record<string, unknown>, options?: unknown) => Promise<unknown> } };
    const invoke = host.__TAURI_INTERNALS__.invoke.bind(host.__TAURI_INTERNALS__);
    let attempt = 0;
    host.__TAURI_INTERNALS__.invoke = async (command, args, options) => {
      const result = await invoke(command, args, options);
      return command === "start_codex_oauth" ? { ...result as object, loginId: `oauth_synthetic_${++attempt}` } : result;
    };
  });
  const dialog = await openSignIn(page);
  await dialog.getByRole("button", { name: "Open sign-in page" }).click();
  await expect(dialog.getByRole("button", { name: "Go to sign-in page" })).toBeEnabled();
  await dialog.getByText("ChatGPT (SIWC)", { exact: true }).click();
  await expect(dialog.getByRole("region", { name: "Sign-in page" })).toHaveText("The sign-in page will appear here.");
  await expect(dialog.getByRole("button", { name: "Open sign-in page" })).toBeEnabled();
  const calls = await invokes(page);
  expect(calls.findLast(({ command }) => command === "update_codex_oauth_view")?.args).toMatchObject({
    loginId: "oauth_synthetic_1", bounds: null,
  });
  expect(calls.some(({ command, args }) => command === "update_codex_oauth_view" && args.loginId === "oauth_synthetic_2")).toBe(false);
});

test("left-click uses the only compatible proxy from the fresh pool", async ({ page }) => {
  await installTauriMock(page, { populated: true, proxyCount: 2 });
  const dialog = await openSignIn(page);
  await page.evaluate(() => {
    const host = window as unknown as { __TAURI_INTERNALS__: { invoke: (command: string, args?: Record<string, unknown>, options?: unknown) => Promise<unknown> } };
    const original = host.__TAURI_INTERNALS__.invoke.bind(host.__TAURI_INTERNALS__);
    host.__TAURI_INTERNALS__.invoke = async (command, args, options) => {
      const result = await original(command, args, options);
      if (command !== "get_local_proxy_pool") return result;
      const pool = result as ProxyPoolSummary;
      return { ...pool, entries: pool.entries.map((entry, i) =>
        i === 0 ? { ...entry, endpoint: entry.endpoint.replace("http://", "https://") } : entry) };
    };
  });
  await dialog.getByRole("button", { name: "Sign in with a proxy" }).click();
  await expect.poll(async () => (await invokes(page)).findLast(({ command }) => command === "start_codex_oauth")?.args)
    .toMatchObject({ proxyId: "proxy_synthetic_2", presentation: { embedded: true, openWindow: true } });
  await expect(page.getByRole("dialog", { name: "Sign in with a proxy" })).toHaveCount(0);
  await expect(dialog.getByRole("button", { name: "Go to sign-in page" })).toBeEnabled();
});

test("right-click still uses a remembered compatible proxy without a picker", async ({ page }) => {
  await installTauriMock(page, { populated: true, proxyCount: 2 });
  const dialog = await openSignIn(page);
  await page.evaluate(() => localStorage.setItem("relay.signInProxyId", "proxy_synthetic_2"));
  await dialog.getByRole("button", { name: "Sign in with a proxy" }).click({ button: "right" });
  await expect.poll(async () => (await invokes(page)).findLast(({ command }) => command === "start_codex_oauth")?.args)
    .toMatchObject({ proxyId: "proxy_synthetic_2", presentation: { embedded: true, openWindow: true } });
  await expect(page.getByRole("dialog", { name: "Sign in with a proxy" })).toHaveCount(0);
  await expect(page.getByRole("menu")).toHaveCount(0);
});

test("a failed native placement can retry without replacing the issued attempt", async ({ page }) => {
  await installTauriMock(page, { populated: true });
  await page.addInitScript(() => {
    const tauri = (window as unknown as { __TAURI_INTERNALS__: { invoke: (command: string, args?: unknown) => Promise<unknown> } }).__TAURI_INTERNALS__;
    const original = tauri.invoke.bind(tauri);
    let opens = 0;
    tauri.invoke = async (command, args) => {
      const result = await original(command, args);
      if (command === "resume_codex_oauth") opens++;
      if (command === "update_codex_oauth_view") return opens >= 2;
      return result;
    };
  });
  const dialog = await openSignIn(page);
  const open = dialog.getByRole("button", { name: "Open sign-in page" });
  await open.click();
  await expect(dialog.getByRole("alert")).toContainText("Could not open the sign-in page");
  await expect(dialog.getByRole("button", { name: "Copy sign-in link" })).toBeEnabled();
  await open.click();
  await expect(dialog.getByRole("button", { name: "Go to sign-in page" })).toBeEnabled();
  expect((await invokes(page)).filter(({ command }) => command === "start_codex_oauth")).toHaveLength(1);
});

test("a rejected first open reports the error without trying to show a native pane", async ({ page }) => {
  await installTauriMock(page, { populated: true });
  await page.addInitScript(() => {
    const tauri = (window as unknown as { __TAURI_INTERNALS__: { invoke: (command: string, args?: unknown) => Promise<unknown> } }).__TAURI_INTERNALS__;
    const invoke = tauri.invoke.bind(tauri);
    let rejected = false;
    tauri.invoke = async (command, args) => {
      const result = await invoke(command, args);
      if (command === "resume_codex_oauth" && !rejected) {
        rejected = true;
        throw { code: "invalid_state", message: "Synthetic sign-in error" };
      }
      return result;
    };
  });
  const dialog = await openSignIn(page);
  const open = dialog.getByRole("button", { name: "Open sign-in page" });
  await open.click();
  await expect(dialog.getByRole("alert")).toHaveText("Could not open the sign-in page");
  expect((await invokes(page)).some(({ command }) => command === "update_codex_oauth_view")).toBe(false);
  await open.click();
  await expect(dialog.getByRole("button", { name: "Go to sign-in page" })).toBeEnabled();
  expect((await invokes(page)).filter(({ command }) => command === "start_codex_oauth")).toHaveLength(1);
});

test("sign-in notes remember both states after closing the dialog and reloading", async ({ page }) => {
  await installTauriMock(page, { populated: true, locale: "ru" });
  const dialog = await openSignIn(page, "ru");
  const notes = dialog.getByRole("button", { name: "Заметки", exact: true });
  const email = dialog.getByLabel("Почта", { exact: true });
  const close = dialog.getByRole("button", { name: "Закрыть", exact: true });
  await expect(notes).toHaveAttribute("aria-expanded", "false");
  await notes.click();
  await expect(email).toBeVisible();
  await close.click();
  await expect(dialog).toHaveCount(0);
  await showSignIn(page, "ru");
  await expect(notes).toHaveAttribute("aria-expanded", "true");
  await expect(email).not.toBeFocused();
  expect((await invokes(page)).filter(({ command }) => command === "update_codex_oauth_view")).toHaveLength(0);
  await openSignIn(page, "ru");
  await expect(notes).toHaveAttribute("aria-expanded", "true");
  await expect(email).toBeVisible();
  await notes.click();
  await openSignIn(page, "ru");
  await expect(notes).toHaveAttribute("aria-expanded", "false");
  await expect(email).toHaveCount(0);
});

for (const [width, height] of [[1160, 760], [840, 560], [390, 620]] as const) {
  test(`notes and the sign-in pane stay separate at ${width}px`, async ({ page }, testInfo) => {
    const locale = width === 840 ? "ru" : "en";
    const labels = locale === "ru"
      ? { notes: "Заметки", email: "Почта", codeSeed: "Секрет 2FA", open: "Открыть форму" }
      : { notes: "Notes", email: "Email", codeSeed: "2FA secret", open: "Open sign-in page" };
    await page.setViewportSize({ width, height });
    await installTauriMock(page, { populated: true, locale, theme: width === 840 ? "light" : "dark" });
    const dialog = await openSignIn(page, locale);
    const openLabel = dialog.getByRole("button", { name: labels.open, exact: true }).locator("span");
    expect(await openLabel.evaluate((element) => element.scrollWidth <= element.clientWidth)).toBe(true);
    const notes = dialog.getByRole("button", { name: labels.notes, exact: true });
    await notes.click();
    const email = dialog.getByLabel(labels.email, { exact: true });
    await expect(email).not.toBeFocused();
    await page.keyboard.press("Tab");
    await expect(email).toBeFocused();
    await expect(email).toHaveCSS("outline-style", "solid");
    await email.click();
    await expect(email).toHaveCSS("outline-style", "none");
    await email.fill("synthetic@example.test");
    expect(await email.evaluate((input: HTMLInputElement) => {
      const style = getComputedStyle(input);
      const canvas = document.createElement("canvas");
      const context = canvas.getContext("2d")!;
      context.font = style.font;
      const available = input.clientWidth - parseFloat(style.paddingLeft) - parseFloat(style.paddingRight);
      return available >= context.measureText(input.value).width;
    })).toBe(true);
    const slot = await dialog.locator(".oauth-page-slot").boundingBox();
    const column = await dialog.locator(".oauth-waiting").boundingBox();
    expect(width > 700 ? slot!.x >= column!.x + column!.width : slot!.y >= column!.y + column!.height).toBe(true);
    expect(slot!.width).toBeGreaterThan(260);
    expect(slot!.height).toBeGreaterThan(140);
    await page.mouse.move(0, 0);
    await dialog.screenshot({ path: testInfo.outputPath("oauth-layout.png") });
    const seed = dialog.getByLabel(labels.codeSeed, { exact: true });
    await seed.scrollIntoViewIfNeeded();
    await expect(seed).toBeInViewport({ ratio: 1 });
    expect(await dialog.locator(".oauth-page-slot").boundingBox()).toEqual(slot);
  });
}
