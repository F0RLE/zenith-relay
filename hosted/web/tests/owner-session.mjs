import { expect } from "@playwright/test";
import { join } from "node:path";

export async function checkOwnerSession(origin, owner, foreign) {
  const api = origin + "/api/hosted/v1/";
  const verify = api + "auth/admin/sign-in/verify";
  const options = api + "auth/admin/sign-in/options";
  const read = api + "admin/overview";
  const logout = api + "auth/admin/logout";
  const opts = { headers: { Origin: origin } };
  const ctx = owner.context(), request = ctx.request;
  const login = async (page, status) => {
    const reply = page.waitForResponse(r => r.url() === verify);
    await page.getByRole("button", { name: "Sign in with a passkey" }).click();
    expect((await reply).status()).toBe(status);
  };
  await foreign.goto(origin + "/admin/overview");
  await login(foreign, 401);
  await expect(foreign.getByRole("alert")).toBeVisible();
  expect((await foreign.context().request.get(read)).status()).toBe(401);

  await owner.setViewportSize({ width: 320, height: 740 });
  await owner.goto(origin + "/admin");
  await expect(owner).toHaveURL(origin + "/admin/login");
  await expect(owner.getByRole("button", { name: "Sign in with a passkey" })).toBeInViewport();
  const shots = process.env.RELAY_TEST_OWNER_SCREENSHOT_DIR;
  if (shots) await owner.screenshot({ path: join(shots, "owner-login.png") });
  const start = await request.post(options, opts);
  expect(start.status()).toBe(200);
  const attempt = (await ctx.cookies()).find(c => c.name === "__Host-relay_admin_attempt");
  expect(!!attempt && attempt.httpOnly && attempt.secure && attempt.sameSite === "Strict").toBe(true);
  // A failed verification consumes this attempt, not just a valid assertion.
  expect((await request.post(verify, { ...opts, data: {} })).status()).toBe(401);
  expect((await request.post(verify, { ...opts, data: {} })).status()).toBe(401);
  // A signed assertion must not work in a different browser, with a wrong signature,
  // or after that first verification consumed the one-use state.
  let replay, attemptCookie;
  await owner.route(verify, async route => {
    replay = route.request().postDataJSON();
    attemptCookie = (await ctx.cookies()).find(c => c.name === "__Host-relay_admin_attempt");
    const other = await foreign.context().request.post(verify, { ...opts, data: replay });
    expect(other.status()).toBe(401);
    const foreignOrigin = await request.post(verify, { headers: { Origin: "https://foreign.example" }, data: replay });
    expect(foreignOrigin.status()).toBe(403);
    await route.continue({ postData: JSON.stringify({ ...replay, response: { ...replay.response, signature: "AA" } }) });
  }, { times: 1 });
  await login(owner, 401);
  expect((await request.post(verify, { ...opts, data: replay })).status()).toBe(401);
  expect((await request.get(read)).status()).toBe(401);

  const proof = owner.waitForRequest(r => r.url() === verify);
  const cookieAtStart = owner.waitForResponse(r => r.url() === options).then(async () => {
    return (await ctx.cookies()).find(c => c.name === "__Host-relay_admin_attempt");
  });
  await login(owner, 200);
  await expect(owner).toHaveURL(origin + "/admin/overview");
  await expect(owner.getByText("The service is in setup.", { exact: false })).toBeVisible();
  await expect(owner.getByText("No requests found", { exact: true })).toBeVisible();
  await expect(owner.locator(".report-chart-stack")).toHaveCount(0);
  await expect(owner.locator(".relay-shell")).toHaveCount(0);
  expect(await owner.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  await owner.setViewportSize({ width: 1080, height: 720 });
  if (shots) await owner.screenshot({ path: join(shots, "owner-overview.png") });
  const active = (await ctx.cookies()).find(c => c.name === "__Host-relay_admin");
  expect(!!active && active.httpOnly && active.secure && active.sameSite === "Strict" && active.path === "/").toBe(true);
  expect(await owner.evaluate(() => document.cookie.includes("relay_admin"))).toBe(false);
  const used = await cookieAtStart;
  expect(!!used && used.value !== attemptCookie.value).toBe(true);
  await ctx.addCookies([used]);
  const again = await request.post(verify, {
    ...opts, data: (await proof).postDataJSON(),
  });
  expect(again.status()).toBe(401);
  await owner.reload();
  await expect(owner.getByRole("button", { name: "Sign out" })).toBeVisible();
  await owner.goto(origin + "/admin");
  await expect(owner).toHaveURL(origin + "/admin/overview");
  const projection = (await (await request.get(read)).json()).data;
  expect(Object.keys(projection).sort()).toEqual(["csrf_token", "service_mode"]);
  expect(projection.service_mode).toBe("setup");
  for (const headers of [{ Origin: origin }, { Origin: origin, "X-Relay-CSRF": "0".repeat(64) },
    { Origin: "https://foreign.example", "X-Relay-CSRF": projection.csrf_token }]) {
    expect((await request.post(logout, { headers })).status()).toBe(403);
  }
  expect((await request.get(read)).status()).toBe(200);
  expect((await request.post(origin + "/v1/responses", opts)).status()).toBe(404);
  expect((await request.post(api + "payments", opts)).status()).toBe(401);
  await owner.getByRole("button", { name: "Sign out" }).click();
  await expect(owner).toHaveURL(origin + "/admin/login");
  expect((await ctx.cookies()).some(c => c.name === active.name)).toBe(false);
  await ctx.addCookies([active]);
  expect((await request.get(read)).status()).toBe(401);
  await owner.goto(origin + "/admin/overview");
  await expect(owner).toHaveURL(origin + "/admin/login");
  // Keep a new, still-valid session across a real process restart in the Rust harness.
  await login(owner, 200);
  await expect(owner).toHaveURL(origin + "/admin/overview");
  expect((await request.get(read)).status()).toBe(200);
}
