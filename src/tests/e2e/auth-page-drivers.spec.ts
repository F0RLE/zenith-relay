import { expect, test } from "@playwright/test";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

const root = new URL("../../../src-tauri/src/local_pool/", import.meta.url);
const patterns = ["en", "ru"].map((locale) => JSON.parse(
  readFileSync(new URL(`auth_page_patterns/${locale}.json`, root), "utf8"),
));
const driver = (path: string) => fileURLToPath(new URL(path, root));
type DriverWindow = Window & {
  module: { exports: (...args: unknown[]) => unknown };
  syntheticSubmissions: number;
};

test.beforeEach(async ({ page }) => {
  await page.route("**/*", (route) => route.fulfill({
    contentType: "text/html; charset=utf-8",
    body: '<h1>Войти в ChatGPT</h1><form action="/log-in" method="post"><label>Email<input type="email" name="email"></label><button type="submit">Continue to sign in</button></form>',
  }));
  await page.goto("https://auth.openai.com/log-in");
  await page.evaluate(() => {
    Object.assign(window, { module: { exports: null }, syntheticSubmissions: 0 });
    document.querySelector("form")!.addEventListener("submit", (event) => {
      event.preventDefault();
      (window as unknown as DriverWindow).syntheticSubmissions++;
    });
  });
});

test("login driver uses the real browser environment with literal input data", async ({ page }) => {
  await page.addScriptTag({ path: driver("commands/oauth/window/automatic_login.cjs") });
  const observation = await page.evaluate((patterns) => {
    const client = window as unknown as DriverWindow;
    const result = client.module.exports(patterns, {
      url: location.href, expiresAtMs: Date.now() + 10_000,
      waitUntilMs: Date.now() + 10_000, stage: "email", value: "synthetic@example.test",
    });
    return { result, value: document.querySelector("input")!.value, submissions: client.syntheticSubmissions };
  }, patterns);
  expect(observation).toEqual({ result: "submitted", value: "synthetic@example.test", submissions: 1 });
});

test("login driver blocks an unsafe accessible submit label before filling", async ({ page }) => {
  await page.getByRole("button").evaluate((button) => button.setAttribute("aria-label", "Разрешить доступ"));
  await page.addScriptTag({ path: driver("commands/oauth/window/automatic_login.cjs") });
  const observation = await page.evaluate((patterns) => {
    const client = window as unknown as DriverWindow;
    const result = client.module.exports(patterns, {
      url: location.href, expiresAtMs: Date.now() + 10_000,
      waitUntilMs: Date.now() + 10_000, stage: "email", value: "synthetic@example.test",
    });
    return { result, value: document.querySelector("input")!.value, submissions: client.syntheticSubmissions };
  }, patterns);
  expect(observation).toEqual({ result: "manual", value: "", submissions: 0 });
});

test("snapshot driver recognizes mixed-language markers in the real document", async ({ page }) => {
  await page.addScriptTag({ path: driver("client_auth_watchdog/auth_snapshot.cjs") });
  const snapshot = await page.evaluate((patterns) =>
    (window as unknown as DriverWindow).module.exports(patterns), patterns);
  expect(snapshot).toEqual({ route: "/log-in", loginUiSignal: true, loginUiMarkers: ["login_title", "login_primary_action"] });
});
