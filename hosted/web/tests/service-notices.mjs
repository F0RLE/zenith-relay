import { expect } from "@playwright/test";
import { join } from "node:path";
import { forkOwnerSession, ownerAssertion } from "./owner-session.mjs";

export async function checkServiceNotices({ a, b, owner, origin, api, ownerRead, login, issue, wait }) {
  const ownerHttp = owner.context().request;
  const customer = a.context().request;
  const later = b.context().request;
  const endpoint = api + "admin/notifications";
  const inbox = api + "notifications";
  const items = async (client, url = inbox) => {
    const r = await client.get(url);
    expect(r.status()).toBe(200);
    expect(r.headers()["cache-control"]).toBe("no-store");
    return (await r.json()).data.items;
  };
  const csrf = async client => (await (await client.get(api + "account")).json()).data.csrf_token;
  const mutation = (client, target, token, data, site = origin) => client.post(target, {
    headers: { Origin: site, "X-Relay-CSRF": token }, data,
  });
  const command = (revision, action) => ({ operation_id: crypto.randomUUID(), expected_revision: revision, command: action });
  const create = async (title, message) => {
    await owner.getByRole("button", { name: "New notification", exact: true }).click();
    await expect(owner.getByLabel("Title", { exact: true })).toBeFocused();
    await owner.getByLabel("Title", { exact: true }).fill(title);
    await owner.getByLabel("Message", { exact: true }).fill(message);
    await owner.getByRole("button", { name: "Save draft", exact: true }).click();
    await expect(owner.getByRole("article", { name: title, exact: true })).toBeVisible();
    return (await items(ownerHttp, endpoint)).find(n => n.title === title);
  };
  const privateTitle = "Initial service notice";
  const previous = await forkOwnerSession(owner, origin);
  await owner.getByRole("link", { name: "Notifications", exact: true }).click();
  await expect(owner).toHaveURL(origin + "/admin/notifications");
  await expect(owner.getByRole("heading", { name: "Notifications", exact: true })).toBeVisible();
  expect((await fetch(origin + "/admin/notifications", { redirect: "manual" })).headers.get("location")).toBe("/admin/login");
  expect((await fetch(inbox)).status).toBe(401);
  expect((await ownerHttp.get(inbox)).status()).toBe(401);
  expect((await customer.get(endpoint)).status()).toBe(401);
  const privateNotice = await create(privateTitle, "Only the first registered tester can see this.");
  expect(privateNotice).toMatchObject({ revision: 1, published_at_ms: null, recipient_count: null });
  await owner.reload();
  const row = owner.getByRole("article", { name: privateTitle });
  await expect(row).toBeVisible();
  expect(await items(customer)).toEqual([]);

  const path = endpoint + "/" + privateNotice.id;
  const options = path + "/options";
  const verification = endpoint + "/confirmations/";
  const ownerCsrf = (await ownerRead()).csrf_token;
  const post = (url, data, client = ownerHttp, token = ownerCsrf, site = origin) =>
    mutation(client, url, token, data, site);
  const change = (...args) => post(path, ...args);
  const rawPublish = command(1, { action: "publish" });
  let proof, verify;
  try {
    for (const url of [path, options]) {
      expect((await post(url, rawPublish, customer)).status()).toBe(401);
      expect((await post(url, rawPublish, ownerHttp, "0".repeat(64))).status()).toBe(403);
      expect((await post(url, rawPublish, ownerHttp, ownerCsrf, "https://foreign.invalid")).status()).toBe(403);
    }
    const withoutProof = await change(rawPublish);
    expect(withoutProof.status()).toBe(403);
    expect((await withoutProof.json()).error.code).toBe("fresh_passkey_required");
    expect((await post(options, command(1, { action: "discard" }))).status()).toBe(400);
    const prepared = await post(options, rawPublish);
    expect(prepared.status()).toBe(200);
    const challenge = (await prepared.json()).data;
    verify = `${verification}${challenge.ceremony_id}/verify`;
    proof = await ownerAssertion(owner, challenge.options);
    const earlier = await previous.request.get(api + "admin/overview");
    expect(earlier.status()).toBe(200);
    const wrong = await post(verify, proof, previous.request, (await earlier.json()).data.csrf_token);
    expect(wrong.status()).toBe(409);
    expect((await wrong.json()).error.code).toBe("passkey_ceremony_expired");
    expect(await items(customer)).toEqual([]);
  } finally { await previous.close(); }
  await row.getByRole("button", { name: "Edit", exact: true }).click();
  await owner.getByLabel("Message", { exact: true }).fill("Only the first tester can see this notice.");
  await owner.getByRole("button", { name: "Save draft", exact: true }).click();
  await expect(row).toContainText("Only the first tester can see this notice.");
  const stale = await post(verify, proof);
  expect(stale.status()).toBe(409);
  expect((await stale.json()).error.code).toBe("notice_changed");
  expect((await post(verify, proof)).status()).toBe(409);
  await row.getByRole("button", { name: "Send", exact: true }).click();
  await row.getByRole("button", { name: "Cancel", exact: true }).click();
  await expect(row.getByRole("button", { name: "Send", exact: true })).toBeFocused();
  expect(await items(customer)).toEqual([]);
  issue("notice-draft", privateNotice.id);
  await wait("notice-confirm");

  const writes = [];
  const record = async route => { writes.push(route.request().postDataJSON()); await route.continue(); };
  const cut = async route => {
    const result = await route.fetch();
    expect(result.status()).toBe(200);
    expect((await result.json()).data).toEqual({ operation_id: writes[0].operation_id, applied: true });
    await result.dispose();
    const replay = await post(route.request().url(), route.request().postDataJSON());
    expect(replay.status()).toBe(409);
    expect((await replay.json()).error.code).toBe("passkey_ceremony_expired");
    await route.abort("failed");
  };
  const verifyRoute = `${verification}*/verify`;
  await owner.route(verifyRoute, cut, { times: 1 });
  await owner.route(options, record);
  try {
    await row.getByRole("button", { name: "Send", exact: true }).click();
    await row.getByRole("button", { name: "Confirm sending", exact: true }).click();
    await expect(owner.getByRole("alert")).toHaveText(/No confirmation received/);
    await row.getByRole("button", { name: "Retry", exact: true }).click();
    await expect(row).toContainText("Recipients: 1");
  } finally { await owner.unroute(options, record); await owner.unroute(verifyRoute, cut); }
  expect(writes).toHaveLength(2);
  expect(writes[1]).toEqual(writes[0]);
  expect((await ownerRead()).csrf_token).toBe(ownerCsrf);
  const repeat = await Promise.all([change(writes[0]), change(writes[0])]);
  expect(repeat.map(r => r.status())).toEqual([200, 200]);
  expect((await change({ ...writes[0], command: { action: "discard" } })).status()).toBe(409);
  expect((await change(command(3, { action: "save", title: "No rewrite", message: "Conflict." }))).status()).toBe(409);
  expect((await items(customer)).map(n => n.id)).toEqual([privateNotice.id]);

  await login(b);
  await expect(b.locator(".hosted-tools .hosted-balance")).toBeVisible();
  expect(await items(later, inbox + "?workspace_id=" + (await (await customer.get(api + "account")).json()).data.workspace_id)).toEqual([]);
  const mark = (client, token, ids) => mutation(client, inbox + "/read", token, { ids });
  const bToken = await csrf(later);
  expect((await mark(later, bToken, [privateNotice.id])).status()).toBe(404);
  expect((await mark(later, await csrf(customer), [privateNotice.id])).status()).toBe(403);

  const deleted = await create("Discarded draft", "Do not send.");
  const discarded = owner.getByRole("article", { name: deleted.title });
  await discarded.getByRole("button", { name: "Delete", exact: true }).click();
  await expect(discarded).toContainText("Delete draft?");
  await discarded.getByRole("button", { name: "Delete", exact: true }).click();
  await expect(discarded).toHaveCount(0);
  expect((await post(endpoint + "/" + deleted.id, command(0, {
    action: "save", title: "Replacement draft", message: "Same ID is no longer available.",
  }))).status()).toBe(409);

  const shared = await create("Service update", "We have scheduled work.\n<b>Plain text, not markup.</b>");
  const broadcast = owner.getByRole("article", { name: shared.title });
  await broadcast.getByRole("button", { name: "Send", exact: true }).click();
  await broadcast.getByRole("button", { name: "Confirm sending", exact: true }).click();
  await expect(broadcast).toContainText("Recipients: 2");
  expect((await mark(later, bToken, [shared.id, privateNotice.id])).status()).toBe(404);
  expect((await items(later)).map(n => [n.id, n.read])).toEqual([[shared.id, false]]);

  const bell = a.locator(".hosted-tools .notification-button");
  const panel = a.getByRole("dialog", { name: "Notifications" });
  await a.setViewportSize({ width: 320, height: 740 });
  await a.emulateMedia({ colorScheme: "dark", reducedMotion: "reduce" });
  await a.reload();
  await expect(bell).toHaveAttribute("data-unread", "true");
  await bell.click();
  await expect(panel.getByRole("heading", { name: shared.title, exact: true })).toBeVisible();
  await expect(panel.getByText("<b>Plain text, not markup.</b>", { exact: false })).toBeVisible();
  await expect(panel.locator("b")).toHaveCount(0);
  await a.getByRole("combobox", { name: /^Language:/ }).click();
  await expect(panel).toHaveCount(0);
  await expect(bell).toHaveAttribute("inert", "");
  await a.getByRole("listbox", { name: "Language" }).press("Escape");
  await expect(bell).toHaveAttribute("data-unread", "true");
  await bell.click();
  await expect(panel.getByRole("heading", { name: shared.title, exact: true })).toBeVisible();
  const fits = await panel.evaluate(el => {
    const box = el.getBoundingClientRect();
    return box.left >= 11 && box.right <= innerWidth - 11 && box.bottom <= innerHeight - 11
      && document.documentElement.scrollWidth <= innerWidth;
  });
  expect(fits).toBe(true);
  if (process.env.RELAY_TEST_NOTICE_CAPTURE) {
    await a.screenshot({ path: join(process.env.RELAY_TEST_NOTICE_CAPTURE, "customer-320-dark.png") });
    for (const [width, theme] of [[1120, "light"], [320, "dark"]]) {
      await owner.setViewportSize({ width, height: 760 });
      await owner.emulateMedia({ colorScheme: theme, reducedMotion: "reduce" });
      expect(await owner.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
      await owner.screenshot({ path: join(process.env.RELAY_TEST_NOTICE_CAPTURE, `owner-${width}-${theme}.png`), fullPage: true });
    }
  }
  await panel.getByRole("button", { name: "Mark as read" }).click();
  await expect(bell).toHaveAttribute("data-unread", "false");
  await expect(panel.getByRole("button", { name: "Mark as read" })).toHaveCount(0);
  await expect(panel.getByRole("heading", { name: privateTitle, exact: true })).toBeVisible();
  const aToken = await csrf(customer);
  expect((await mark(customer, aToken, [shared.id, privateNotice.id])).status()).toBe(200);
  expect((await items(customer)).every(n => n.read)).toBe(true);
  expect((await items(later)).map(n => [n.id, n.read])).toEqual([[shared.id, false]]);
  await a.keyboard.press("Escape");
  await expect(panel).toHaveCount(0);
  await expect(bell).toBeFocused();
  await a.route(inbox, route => route.fulfill({ status: 503, json: { error: { code: "unavailable" } } }));
  try {
    await bell.press("Enter");
    await expect(panel.getByRole("alert")).toHaveText("Could not refresh notifications.");
    await expect(panel.getByRole("heading", { name: shared.title, exact: true })).toHaveCount(0);
  } finally { await a.unroute(inbox); }
  await panel.getByRole("button", { name: "Retry", exact: true }).click();
  await expect(panel.getByRole("heading", { name: shared.title, exact: true })).toBeVisible();
  await a.keyboard.press("Escape");
  await a.setViewportSize({ width: 360, height: 760 });
  await a.emulateMedia({ colorScheme: null, reducedMotion: null });
  await owner.setViewportSize({ width: 1024, height: 760 });
  await owner.emulateMedia({ colorScheme: null, reducedMotion: null });
  return { privateId: privateNotice.id, sharedId: shared.id };
}
