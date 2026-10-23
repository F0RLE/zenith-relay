import { readFileSync } from "node:fs";
import { expect, test, type Page } from "../../bun-playwright";
import { emitTauriEvent, installTauriMock } from "../tauri-mock";

const driver = readFileSync(new URL("../../../../src-tauri/src/local_pool/commands/oauth/window/automatic_login.js", import.meta.url), "utf8");
const email = "synthetic@example.test";
const password = "synthetic-\"\\-password";

async function formPage(page: Page, html: string, path = "/log-in") {
  await page.route("**/*", (route) => route.fulfill({ contentType: "text/html", body: html }));
  await page.goto(`https://auth.openai.com${path}`);
  await page.evaluate(() => {
    document.querySelectorAll("form").forEach((form) => form.addEventListener("submit", (event) => {
      event.preventDefault();
      form.dataset.submissions = String(Number(form.dataset.submissions ?? 0) + 1);
    }));
  });
}

function runDriver(page: Page, input: Record<string, unknown> = {}) {
  return page.evaluate(`(${driver})(${JSON.stringify({ url: page.url(), expiresAtMs: Date.now() + 2_000, waitUntilMs: Date.now() + 5_000, ...input })})`);
}

test("a hydrating login form waits within the bounded loading grace", async ({ page }) => {
  await formPage(page, '<div id="root"></div><script>setTimeout(() => { document.getElementById("root").innerHTML = \'<form method="post"><input type="email"><button>Continue</button></form>\'; }, 1500)</script>');
  expect(await runDriver(page)).toBe("wait");
  await page.locator("input").waitFor();
  expect(await runDriver(page)).toBe("email");
});

test("empty pages stop waiting after the loading grace and CAPTCHA never waits", async ({ page }) => {
  await formPage(page, '<div>Loading</div>');
  expect(await runDriver(page, { waitUntilMs: Date.now() - 1 })).toBe("manual");
  await formPage(page, '<div data-sitekey="synthetic"></div>');
  expect(await runDriver(page)).toBe("manual");
});

test("an evaluation queued beyond its native deadline cannot fill or submit", async ({ page }) => {
  await formPage(page, '<form method="post"><input type="email"><button>Continue</button></form>');
  const blocking = page.evaluate(() => { const until = Date.now() + 3_600; while (Date.now() < until) { /* Synthetic blocked renderer. */ } });
  await new Promise((resolve) => setTimeout(resolve, 100));
  const pending = runDriver(page, { stage: "email", value: email });
  expect(await Promise.race([pending, new Promise((resolve) => setTimeout(() => resolve("timed_out"), 2_000))])).toBe("timed_out");
  expect(await pending).toBe("manual");
  await blocking;
  await expect(page.locator("input")).toHaveValue("");
  await expect(page.locator("form")).not.toHaveAttribute("data-submissions", /./);
});

test("expiry inside an input handler fences the final submit", async ({ page }) => {
  await formPage(page, '<form method="post"><input type="email"><button>Continue</button></form>');
  const expiresAtMs = Date.now() + 2_000;
  await page.locator("input").evaluate((input, expires) => input.addEventListener("input", () => { Date.now = () => expires; }), expiresAtMs);
  expect(await runDriver(page, { expiresAtMs, stage: "email", value: email })).toBe("manual");
  await expect(page.locator("form")).not.toHaveAttribute("data-submissions", /./);
});

test("saved email and password use normal form events and preserve literal values", async ({ page }) => {
  await formPage(page, '<form method="post"><input type="email" name="email" required><button>Continue</button></form>');
  expect(await runDriver(page)).toBe("email");
  expect(await runDriver(page, { stage: "email", value: email })).toBe("submitted");
  await expect(page.locator("input")).toHaveValue(email);
  await expect(page.locator("form")).toHaveAttribute("data-submissions", "1");
  await formPage(page, '<form method="post"><input type="password" autocomplete="current-password" required><button>Continue</button></form>', "/log-in/password");
  expect(await runDriver(page)).toBe("password");
  expect(await runDriver(page, { stage: "password", value: password })).toBe("submitted");
  await expect(page.locator("input")).toHaveValue(password);
});

test("authenticator code is limited to explicit TOTP challenge pages", async ({ page }) => {
  const html = '<form method="post"><input name="code" autocomplete="one-time-code"><button>Continue</button></form>';
  await formPage(page, html, "/u/mfa-otp-challenge");
  expect(await runDriver(page)).toBe("totp");
  expect(await runDriver(page, { stage: "totp", value: "123456" })).toBe("submitted");
  await expect(page.locator("input")).toHaveValue("123456");
  for (const path of ["/email-verification", "/log-in", "/phone-verification"]) {
    await formPage(page, html, path);
    expect(await runDriver(page, { stage: "totp", value: "123456" })).toBe("manual");
    await expect(page.locator("input")).toHaveValue("");
  }
});

test("combined login form fills the saved email and password before one submission", async ({ page }) => {
  await formPage(page, '<form method="post"><input type="email" name="email" value="old@example.test" required><input type="password" required><button>Continue</button></form>');
  expect(await runDriver(page)).toBe("password");
  expect(await runDriver(page, { stage: "password", value: password, email })).toBe("submitted");
  await expect(page.locator('input[type="email"]')).toHaveValue(email);
  await expect(page.locator('input[type="password"]')).toHaveValue(password);
  await expect(page.locator("form")).toHaveAttribute("data-submissions", "1");
});

test("a password form bound to a different hidden username is not filled", async ({ page }) => {
  await formPage(page, '<form method="post"><input type="hidden" name="username" value="other@example.test"><input type="password"><button>Continue</button></form>', "/u/login/password");
  expect(await runDriver(page, { stage: "password", value: password, email })).toBe("manual");
  await expect(page.locator('input[type="password"]')).toHaveValue("");
});

test("input sanitization cannot silently submit a changed password", async ({ page }) => {
  await formPage(page, '<form method="post"><input type="password"><button>Continue</button></form>', "/log-in/password");
  expect(await runDriver(page, { stage: "password", value: "synthetic\npassword", email })).toBe("manual");
  await expect(page.locator("form")).not.toHaveAttribute("data-submissions", /./);
});

test("form actions changed by input handlers are checked again before submission", async ({ page }) => {
  await formPage(page, '<form method="post"><input type="email"><button>Continue</button></form>');
  await page.locator("input").evaluate((input) => input.addEventListener("input", () => { input.form!.action = "https://foreign.example.test/"; }));
  expect(await runDriver(page, { stage: "email", value: email })).toBe("manual");
  await expect(page.locator("form")).not.toHaveAttribute("data-submissions", /./);
});

const unsafeForms = [
  '<form method="post" action="https://foreign.example.test/"><input type="email"><button>Continue</button></form>',
  '<form method="get"><input type="email"><button>Continue</button></form>',
  '<form method="post" target="_blank"><input type="email"><button>Continue</button></form>',
  '<form method="post"><input type="email"><button formaction="https://foreign.example.test/">Continue</button></form>',
  '<form method="post"><input type="email"><button formmethod="get">Continue</button></form>',
  '<form method="post"><input type="email"><button formtarget="_blank">Continue</button></form>',
  '<form method="post"><input type="email"><button>Create account</button></form>',
  '<form method="post"><input type="email"><button>Continue with Google</button></form>',
  '<form method="post"><input type="email"><button>Authorize</button></form>',
  '<form method="post"><input type="email"><button>Continue</button><button>Other</button></form>',
  '<form method="post"><input type="email"><input type="email"><button>Continue</button></form>',
  '<form method="post"><input type="password" autocomplete="new-password"><button>Continue</button></form>',
  '<div data-sitekey="synthetic"></div><form method="post"><input type="email"><button>Continue</button></form>',
  '<div role="alert">Synthetic login error</div><form method="post"><input type="email"><button>Continue</button></form>',
  '<form method="post"><input type="email" aria-invalid="true"><button>Continue</button></form>',
];

for (const [index, html] of unsafeForms.entries()) {
  test(`unsafe or ambiguous form ${index + 1} requires manual continuation without filling`, async ({ page }) => {
    await formPage(page, html);
    expect(await runDriver(page, { stage: "email", value: email })).toBe("manual");
    expect(await page.locator("input").evaluateAll((fields) => fields.every((field) => !field.value))).toBe(true);
    await expect(page.locator("form")).not.toHaveAttribute("data-submissions", /./);
  });
}

test("navigation races, foreign origins and subframes cannot receive notes", async ({ page }) => {
  const html = '<form method="post"><input type="email"><button>Continue</button></form>';
  await formPage(page, html);
  expect(await runDriver(page, { url: "https://auth.openai.com/log-in?old-page", stage: "email", value: email })).toBe("manual");
  await page.goto("https://foreign.example.test/log-in");
  expect(await runDriver(page, { stage: "email", value: email })).toBe("manual");
  await page.goto("http://auth.openai.com/log-in");
  expect(await runDriver(page, { stage: "email", value: email })).toBe("manual");
  await page.goto("https://auth.openai.com/log-in");
  await page.evaluate(() => { const frame = document.createElement("iframe"); frame.src = "/log-in"; document.body.append(frame); });
  await expect.poll(() => page.frames().length).toBe(2);
  const frame = page.frames()[1];
  await frame.waitForLoadState();
  expect(await frame.evaluate(`(${driver})(${JSON.stringify({ url: "https://auth.openai.com/log-in", expiresAtMs: Date.now() + 2_000, stage: "email", value: email })})`)).toBe("manual");
});

test("detector rechecks the stage immediately before filling", async ({ page }) => {
  await formPage(page, '<form method="post"><input type="email"><button>Continue</button></form>');
  expect(await runDriver(page, { stage: "password", value: password })).toBe("manual");
  await expect(page.locator("input")).toHaveValue("");
});

for (const platform of ["windows", "macos", "linux"] as const) {
test(`BPS reauthentication keeps manual and automatic actions on ${platform}`, async ({ page }) => {
  await installTauriMock(page, { platform, mode: "local", populated: true, locale: "ru", accountOAuthClientKind: "excel_bps", accountAuthReason: "invalid_grant", quotaRefreshStatus: "requires_reauth" });
  await page.goto("/");
  await page.getByRole("button", { name: "Подключения", exact: true }).click();
  const login = page.getByRole("button", { name: "Требуется вход", exact: true }).first();
  await login.click();
  await expect(page.getByRole("dialog", { name: "Войти", exact: true })).toBeVisible();
  const calls = () => page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string; args: Record<string, unknown> }> }).__TAURI_TEST_INVOKES__.filter((call) => call.command === "start_codex_oauth").map((call) => call.args));
  expect((await calls())[0]).toMatchObject({ openBrowser: false });
  expect((await calls())[0]).not.toHaveProperty("automaticLogin");
  await page.getByRole("dialog", { name: "Войти", exact: true }).getByRole("button", { name: "Закрыть", exact: true }).click();
  await login.click({ button: "right" });
  await expect(page.locator(".app-context-menu")).toHaveCount(0);
  await expect.poll(async () => (await calls()).length).toBe(2);
  const started = await calls();
  expect(started[1]).toMatchObject({ openBrowser: true, accountId: started[0].accountId, automaticLogin: true });
  await expect(page.getByText("Пробуем сохранённые почту", { exact: false })).toHaveCount(0);
  await page.getByRole("button", { name: "Открыть окно входа", exact: true }).click();
  await expect.poll(() => page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string }> }).__TAURI_TEST_INVOKES__.some((call) => call.command === "resume_codex_oauth"))).toBe(true);
  await expect(page.getByText("Пробуем сохранённые почту", { exact: false })).toHaveCount(0);
});
}

test("native fallback keeps manual sign-in available and ignores unrelated status events", async ({ page }) => {
  await installTauriMock(page, { mode: "local", populated: true, locale: "ru", accountOAuthClientKind: "excel_bps", accountAuthReason: "invalid_grant", quotaRefreshStatus: "requires_reauth" });
  await page.goto("/");
  await page.getByRole("button", { name: "Подключения", exact: true }).click();
  await page.getByRole("button", { name: "Требуется вход", exact: true }).first().click({ button: "right" });
  const dialog = page.getByRole("dialog", { name: "Войти", exact: true });
  await expect(dialog).toBeVisible();
  await emitTauriEvent(page, "relay-oauth-status", { loginId: "another_synthetic_login", status: "expired", automaticLogin: false });
  await expect(dialog.getByRole("alert")).toHaveCount(0);
  await emitTauriEvent(page, "relay-oauth-status", { loginId: "oauth_synthetic", status: "pending", automaticLogin: false });
  await expect(page.getByText("Пробуем сохранённые почту", { exact: false })).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Открыть окно входа", exact: true })).toBeEnabled();
});

test("a native stop arriving before start resolves leaves manual sign-in available without a description", async ({ page }) => {
  await installTauriMock(page, { mode: "local", populated: true, locale: "ru", accountOAuthClientKind: "excel_bps", accountAuthReason: "invalid_grant", quotaRefreshStatus: "requires_reauth" });
  await page.addInitScript(() => {
    const testWindow = window as unknown as {
      __TAURI_INTERNALS__: { invoke: (command: string, args?: Record<string, unknown>) => Promise<unknown> };
      __TAURI_TEST_EMIT__: (event: string, payload: unknown) => void;
    };
    const invoke = testWindow.__TAURI_INTERNALS__.invoke.bind(testWindow.__TAURI_INTERNALS__);
    testWindow.__TAURI_INTERNALS__.invoke = async (command, args) => {
      const result = await invoke(command, args);
      if (command === "start_codex_oauth") testWindow.__TAURI_TEST_EMIT__("relay-oauth-status", { loginId: "oauth_synthetic", status: "pending", automaticLogin: false });
      return result;
    };
  });
  await page.goto("/");
  await page.getByRole("button", { name: "Подключения", exact: true }).click();
  await page.getByRole("button", { name: "Требуется вход", exact: true }).first().click({ button: "right" });
  await expect(page.getByRole("dialog", { name: "Войти", exact: true })).toBeVisible();
  await expect(page.getByText("Пробуем сохранённые почту", { exact: false })).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Открыть окно входа", exact: true })).toBeEnabled();
});

for (const options of [{ platform: "macos", accountOAuthClientKind: "codex" }, { platform: "linux", accountOAuthClientKind: "siwc" }, { platform: "windows", accountOAuthClientKind: "codex" }, { platform: "windows", accountOAuthClientKind: "siwc" }] as const) {
  test(`right click cannot start automatic login for ${options.platform}/${options.accountOAuthClientKind}`, async ({ page }) => {
    await installTauriMock(page, { ...options, mode: "local", populated: true, locale: "ru", accountAuthReason: "invalid_grant", quotaRefreshStatus: "requires_reauth" });
    await page.goto("/");
    await page.getByRole("button", { name: "Подключения", exact: true }).click();
    await page.getByRole("button", { name: "Требуется вход", exact: true }).first().click({ button: "right" });
    await expect(page.getByRole("menuitem", { name: "Войти автоматически из заметок" })).toHaveCount(0);
    expect(await page.evaluate(() => (window as unknown as { __TAURI_TEST_INVOKES__: Array<{ command: string }> }).__TAURI_TEST_INVOKES__.filter((call) => call.command === "start_codex_oauth").length)).toBe(0);
  });
}
