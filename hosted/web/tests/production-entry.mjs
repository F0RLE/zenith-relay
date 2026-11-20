// Called only by the disposable PostgreSQL bootstrap test with the built site.
import { chromium, expect } from "@playwright/test";

const origin = process.argv[2];
if (!/^http:\/\/localhost:\d+$/.test(origin)) throw new Error("Expected the isolated test origin");
const browser = await chromium.launch({ executablePath: process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH });
try {
  const page = await browser.newPage();
  const config = "/api/hosted/v1/public/config";
  const errors = [], external = [];
  page.on("pageerror", e => errors.push(e.message));
  page.on("request", r => { if (new URL(r.url()).origin !== origin) external.push(r.url()); });
  const response = page.waitForResponse(r => r.url() === origin + config);
  await page.goto(origin);
  expect((await response).status()).toBe(200);
  await expect(page.locator(".landing-header .landing-brand")).toHaveText("Fixture Site");
  await expect(page.locator(".weave-labels strong")).toHaveText("Fixture Site");
  await expect(page.getByText("This service is not open yet.", { exact: true })).toBeVisible();
  await expect(page.locator("a[href^='/app'], a[href^='/admin']")).toHaveCount(0);
  await expect(page).toHaveTitle("Fixture Site");
  await page.getByRole("link", { name: "API connection guide" }).click();
  await expect(page).toHaveURL(origin + "/docs");
  await expect(page.locator("main ol > li")).toHaveCount(4);
  await expect(page.getByRole("heading", { name: "Configure your client" })).toBeVisible();
  await page.getByRole("link", { name: "Policies and support" }).click();
  await expect(page.getByRole("link", { name: "Terms of service" })).toHaveAttribute("href", "https://docs.example.org/terms");
  await expect(page.getByRole("link", { name: "Contact support" })).toHaveCount(0);
  await page.route(origin + config, async route => {
    const upstream = await route.fetch();
    const body = await upstream.json();
    delete body.data.policy_links;
    await route.fulfill({ json: body });
  }, { times: 1 });
  await page.reload();
  await expect(page.getByText("The operator has not published policies or a support contact yet.")).toBeVisible();
  expect(await page.context().cookies()).toEqual([]);
  for (const path of ["/app/keys", "/login"]) {
    await page.goto(origin + path);
    await expect(page.getByRole("heading", { name: "This service is not open yet." })).toBeVisible();
    await expect(page.locator(".relay-shell")).toHaveCount(0);
  }
  for (const path of ["/admin", "/admin/overview"]) {
    await page.goto(origin + path);
    await expect(page).toHaveURL(origin + "/admin/login");
    await expect(page.getByRole("button", { name: "Sign in with a passkey" })).toBeVisible();
  }
  for (const reply of [
    { status: 503 }, { contentType: "text/html", body: "<html></html>" },
    { json: { data: {}, meta: { api_version: "2" } } },
  ]) {
    await page.route(origin + config, route => route.fulfill(reply), { times: 1 });
    await page.goto(origin);
    await expect(page.getByRole("heading", { name: "Cannot connect to a compatible Relay server." })).toBeVisible();
    await expect(page.locator(".landing-home, .relay-shell")).toHaveCount(0);
  }
  expect(errors).toEqual([]);
  expect(external).toEqual([]);
} finally {
  await browser.close();
}
