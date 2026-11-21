import { expect } from "@playwright/test";
import { setWorkspaceLanguage } from "./customer-shell.mjs";
import { join } from "node:path";

export async function checkCustomerSessions({ a, b, secondA, ownerHttp, origin, api, login, pageFor, issue, wait }) {
  const url = api + "account/sessions";
  const read = async (client, path) => {
    const response = await client.get(path);
    expect(response.status()).toBe(200);
    return (await response.json()).data;
  };
  const list = async client => (await read(client, url)).sessions;
  const csrf = async client => (await read(client, api + "account")).csrf_token;
  const end = (client, id, token, from = origin) =>
    client.delete(`${url}/${id}`, { headers: { Origin: from, "X-Relay-CSRF": token } });
  const [alice, remote, bob] = [a.request, secondA.request, b.request];
  const aliceToken = await csrf(alice);
  const [here, idle] = await list(alice);
  const [foreign] = await list(bob);
  expect(here.current && !idle.current && foreign.current).toBe(true);
  expect(new Set([here.id, idle.id, foreign.id]).size).toBe(3);
  expect((await list(remote)).map(row => [row.id, row.current])).toEqual([[idle.id, true], [here.id, false]]);
  expect((await read(alice, url + "?workspace_id=" + (await read(bob, api + "account")).workspace_id)).sessions
    .map(row => row.id)).toEqual([here.id, idle.id]);
  expect(Object.keys(here).sort()).toEqual(["created_at_s", "current", "id", "last_seen_at_s"]);
  expect(here.created_at_s).toBeLessThanOrEqual(here.last_seen_at_s);
  expect((await alice.get(url)).headers()["cache-control"]).toBe("no-store");
  expect((await ownerHttp.get(url)).status()).toBe(401);
  expect((await end(ownerHttp, idle.id, aliceToken)).status()).toBe(401);
  expect((await end(alice, idle.id, await csrf(remote))).status()).toBe(403);
  expect((await end(alice, idle.id, aliceToken, "https://elsewhere.invalid")).status()).toBe(403);
  expect((await end(alice, "invalid", aliceToken)).status()).toBe(400);
  const unavailableTargets = await Promise.all([foreign.id, crypto.randomUUID()].map(id => end(alice, id, aliceToken)));
  for (const response of unavailableTargets) {
    expect(response.status()).toBe(404);
    expect((await response.json()).error.code).toBe("not_found");
  }

  issue("age-customer-session", idle.id);
  await wait("customer-session-aged");
  expect((await list(alice)).map(row => row.id)).toEqual([here.id]);
  expect((await remote.get(url)).status()).toBe(401);
  expect((await end(alice, idle.id, aliceToken)).status()).toBe(404);
  await login(secondA);
  await expect(secondA.locator(".hosted-tools .hosted-balance")).toBeVisible();

  const thirdA = await pageFor("alice");
  try {
    await login(thirdA);
    await expect(thirdA.locator(".hosted-tools .hosted-balance")).toBeVisible();
    const [second, third] = await Promise.all([list(remote), list(thirdA.request)]);
    const [secondToken, thirdToken] = await Promise.all([csrf(remote), csrf(thirdA.request)]);
    const pair = await Promise.all([
      end(remote, third[0].id, secondToken),
      end(thirdA.request, second[0].id, thirdToken),
    ]);
    expect(pair.map(response => response.status()).sort()).toEqual([200, 401]);
    const winning = pair[0].ok() ? 0 : 1;
    const survivor = winning === 0 ? remote : thirdA.request;
    const survivorId = winning === 0 ? second[0].id : third[0].id;
    expect((await pair[winning].json()).data).toEqual({
      id: winning === 0 ? third[0].id : second[0].id, revoked: true, signed_out: false,
    });
    expect((await list(alice)).map(row => row.id)).toEqual([here.id, survivorId]);

    const wallet = (await read(alice, api + "account")).wallet;
    const keys = await read(alice, api + "keys");
    const panel = a.getByRole("region", { name: "Browser sessions", exact: true });
    const otherRow = panel.getByRole("listitem").filter({ hasText: /Browser session/ });
    const button = otherRow.getByRole("button", { name: /^End session started / });
    const open = a.getByRole("link", { name: "Account security" });
    await a.route(url, route => route.fulfill({ status: 503, json: { error: { code: "customer_unavailable" } } }), { times: 1 });
    await open.press("Enter");
    await expect(a).toHaveURL(origin + "/app/overview?panel=account");
    await expect(panel.getByRole("alert")).toBeVisible();
    await expect(panel.getByRole("listitem")).toHaveCount(0);
    const refresh = panel.getByRole("button", { name: "Refresh sessions" });
    await refresh.click();
    await expect(panel.getByText("Current session", { exact: true })).toBeVisible();
    await expect(panel.getByRole("listitem")).toHaveCount(2);

    for (const [width, theme] of [[1024, "light"], [320, "dark"]]) {
      await a.setViewportSize({ width, height: 760 });
      await a.emulateMedia({ colorScheme: theme });
      await a.reload();
      await expect(button).toBeVisible();
      if (width === 320) {
        for (const [locale, title] of [["ru", "Безопасность аккаунта"], ["zh-CN", "账户安全"], ["en", "Account security"]]) {
          await setWorkspaceLanguage(a, locale);
          await expect(a.getByRole("heading", { name: title, exact: true })).toBeVisible();
          expect(await a.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
        }
      }
      await button.press("Enter");
      await expect(otherRow.getByRole("button", { name: "Cancel", exact: true })).toBeFocused();
      expect(await a.evaluate(() => document.documentElement.scrollWidth <= innerWidth
        && [...document.querySelectorAll(".account-sessions button,.hosted-tools .icon-button")].every(element => {
          const rect = element.getBoundingClientRect();
          return element.getClientRects().length === 0 || (rect.left >= 0 && rect.right <= innerWidth && rect.height >= 40);
        }))).toBe(true);
      const dir = process.env.RELAY_TEST_SESSION_UI_CAPTURE;
      if (dir) await a.screenshot({ path: join(dir, `account-sessions-${width}-${theme}.png`), fullPage: true });
      await a.keyboard.press("Escape");
      await expect(button).toBeFocused();
      await expect(otherRow.getByRole("button", { name: "Yes, end session" })).toHaveCount(0);
    }
    expect((await list(survivor))[0].id).toBe(survivorId);

    // Losing the acknowledgement must initiate a read, not a second DELETE.
    let sends = 0;
    const cut = async route => {
      if (++sends > 1) return route.continue();
      const committed = await route.fetch();
      expect(committed.status()).toBe(200);
      await committed.dispose();
      await route.abort("failed");
    };
    await a.route(url + "/" + survivorId, cut);
    try {
      await button.click();
      await otherRow.getByRole("button", { name: "Yes, end session" }).click();
      await expect(panel.getByRole("alert")).toHaveText("Could not confirm the change. Refresh the list to check.");
      await refresh.click();
      await expect(panel.getByText("Current session", { exact: true })).toBeVisible();
      await expect(panel.getByRole("listitem")).toHaveCount(1);
    } finally {
      await a.unroute(url + "/" + survivorId, cut);
    }
    expect(sends).toBe(1);
    expect((await survivor.get(api + "account")).status()).toBe(401);
    expect((await survivor.get(api + "keys")).status()).toBe(401);
    expect((await alice.get(api + "account")).status()).toBe(200);

    // Ending this session clears the cookie and preserves unrelated state.
    await panel.getByRole("button", { name: "Sign out", exact: true }).click();
    await panel.getByRole("button", { name: "Confirm sign-out" }).click();
    await expect(a).toHaveURL(origin + "/login");
    expect((await alice.get(url)).status()).toBe(401);
    expect((await a.context().cookies()).some(cookie => cookie.name === "__Host-relay_customer")).toBe(false);
    await login(a);
    await expect(a.locator(".hosted-tools .hosted-balance")).toBeVisible();
    expect((await list(alice)).map(row => row.id)).not.toContain(here.id);
    expect((await read(alice, api + "account")).wallet).toEqual(wallet);
    expect(await read(alice, api + "keys")).toEqual(keys);
    expect((await list(bob)).map(row => row.id)).toEqual([foreign.id]);
    expect((await ownerHttp.get(api + "admin/overview")).status()).toBe(200);
    await a.setViewportSize({ width: 1024, height: 760 });
    await a.emulateMedia({ colorScheme: null });
  } finally {
    await thirdA.context().close();
  }
}
