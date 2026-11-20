import { expect } from "@playwright/test";
import { join } from "node:path";

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
  const change = async (data, client = ownerHttp, token = null, site = origin) =>
    mutation(client, path, token ?? (await ownerRead()).csrf_token, data, site);
  const rawPublish = command(1, { action: "publish" });
  expect((await change(rawPublish, customer)).status()).toBe(401);
  expect((await change(rawPublish, ownerHttp, "0".repeat(64))).status()).toBe(403);
  expect((await change(rawPublish, ownerHttp, null, "https://foreign.invalid")).status()).toBe(403);
  await row.getByRole("button", { name: "Edit", exact: true }).click();
  await owner.getByLabel("Message", { exact: true }).fill("Only the first tester can see this notice.");
  await owner.getByRole("button", { name: "Save draft", exact: true }).click();
  await expect(row).toContainText("Only the first tester can see this notice.");
  expect((await change(rawPublish)).status()).toBe(409);
  await row.getByRole("button", { name: "Send", exact: true }).click();
  await row.getByRole("button", { name: "Cancel", exact: true }).click();
  await expect(row.getByRole("button", { name: "Send", exact: true })).toBeFocused();
  expect(await items(customer)).toEqual([]);
  issue("notice-draft", privateNotice.id);
  await wait("notice-confirm");

  const stale = await change(command(2, { action: "publish" }));
  expect(stale.status()).toBe(403);
  expect((await stale.json()).error.code).toBe("fresh_passkey_required");
  const writes = [];
  let dropped = false;
  const cut = async route => {
    writes.push(route.request().postDataJSON());
    if (dropped) return route.continue();
    const result = await route.fetch();
    if (result.status() === 200) {
      dropped = true;
      await result.dispose();
      await route.abort("failed");
    } else {
      expect(result.status()).toBe(403);
      await route.fulfill({ response: result });
      await result.dispose();
    }
  };
  await owner.route(path, cut);
  try {
    await row.getByRole("button", { name: "Send", exact: true }).click();
    await row.getByRole("button", { name: "Confirm sending", exact: true }).click();
    await expect(owner.getByRole("alert")).toHaveText(/No confirmation received/);
    await row.getByRole("button", { name: "Retry", exact: true }).click();
    await expect(row).toContainText("Recipients: 1");
  } finally { await owner.unroute(path, cut); }
  expect(writes).toHaveLength(3);
  expect(writes.every(w => JSON.stringify(w) === JSON.stringify(writes[0]))).toBe(true);
  const repeat = await Promise.all([change(writes[0]), change(writes[0])]);
  expect(repeat.map(r => r.status())).toEqual([200, 200]);
  expect((await change({ ...writes[0], command: { action: "discard" } })).status()).toBe(409);
  expect((await change(command(3, { action: "save", title: "No rewrite", message: "Conflict." }))).status()).toBe(409);
  expect((await items(customer)).map(n => n.id)).toEqual([privateNotice.id]);

  await login(b);
  await expect(b.locator(".entry-wallet dd")).toBeVisible();
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

  const shared = await create("Service update", "We have scheduled work.\n<b>Plain text, not markup.</b>");
  const broadcast = owner.getByRole("article", { name: shared.title });
  await broadcast.getByRole("button", { name: "Send", exact: true }).click();
  await broadcast.getByRole("button", { name: "Confirm sending", exact: true }).click();
  await expect(broadcast).toContainText("Recipients: 2");
  expect((await mark(later, bToken, [shared.id, privateNotice.id])).status()).toBe(404);
  expect((await items(later)).map(n => [n.id, n.read])).toEqual([[shared.id, false]]);

  const bell = a.locator(".entry-notification-button");
  const panel = a.getByRole("dialog", { name: "Notifications" });
  await a.setViewportSize({ width: 320, height: 740 });
  await a.emulateMedia({ colorScheme: "dark", reducedMotion: "reduce" });
  await a.reload();
  await expect(bell).toHaveAttribute("data-unread", "true");
  await bell.click();
  await expect(panel.getByRole("heading", { name: shared.title, exact: true })).toBeVisible();
  await expect(panel.getByText("<b>Plain text, not markup.</b>", { exact: false })).toBeVisible();
  await expect(panel.locator("b")).toHaveCount(0);
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
