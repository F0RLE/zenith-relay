import { expect } from "@playwright/test";
import { join } from "node:path";
import { ownerAssertion, signInOwner } from "./owner-session.mjs";
import { setWorkspaceLanguage } from "./customer-shell.mjs";

export async function configureCheckout({ owner, ownerRead, customer, origin, api, issue, wait, getMethods, makeQuote, confirm }) {
  const http = owner.context().request;
  const endpoint = api + "admin/payment-methods";
  const options = endpoint + "/options";
  const verification = api + "admin/payments/confirmations/";
  const verify = challenge => `${verification}${challenge.ceremony_id}/verify`;
  const buttons = name => owner.getByRole("button", { name, exact: true });
  const command = (revision, body) => ({ operation_id: crypto.randomUUID(), expected_revision: revision, command: body });
  const read = async () => {
    const res = await http.get(endpoint);
    expect(res.status()).toBe(200);
    expect(res.headers()["cache-control"]).toBe("no-store");
    return (await res.json()).data;
  };
  const post = async (url, data, { client = http, csrf, src = origin } = {}) => client.post(url,
    { data, headers: { Origin: src, "X-Relay-CSRF": csrf ?? (await ownerRead()).csrf_token } });
  const send = (data, params) => post(endpoint, data, params);
  const reject = async (response, status, code) => {
    const res = await response;
    expect(res.status()).toBe(status);
    if (code) expect((await res.json()).error.code).toBe(code);
  };
  const prepare = async data => {
    const response = await post(options, data);
    expect(response.status()).toBe(200);
    expect(response.headers()["cache-control"]).toBe("no-store");
    const challenge = (await response.json()).data;
    expect(challenge.options.publicKey.userVerification).toBe("required");
    return challenge;
  };
  const check = async () => {
    await buttons("Check draft").click();
    await expect(buttons("Publish")).toBeEnabled();
  };
  const save = async () => {
    await buttons("Save draft").click();
    await expect(owner.getByRole("status")).toHaveText("Draft saved.");
  };
  const publish = async () => {
    await buttons("Publish").click();
    await buttons("Confirm publication").click();
    await expect(owner.getByRole("status")).toHaveText("Checkout updated.");
  };
  const choose = async method => {
    await owner.getByLabel("Payment method", { exact: true }).selectOption({ label: method });
    await buttons("Add method").click();
  };
  const published = owner.getByRole("region", { name: "Published methods" });
  const fits = async width => {
    await owner.setViewportSize({ width, height: 780 });
    expect(await owner.evaluate(() => document.documentElement.scrollWidth <= innerWidth &&
      [...document.querySelectorAll(".payment-builder :is(input,button,select)")].every(el => {
        const box = el.getBoundingClientRect();
        return box.left >= 0 && box.right <= innerWidth;
      }))).toBe(true);
  };
  const dir = process.env.RELAY_CHECKOUT_SCREENSHOTS;

  await owner.emulateMedia({ colorScheme: "light", reducedMotion: "reduce" });
  await owner.getByRole("link", { name: "Payment methods", exact: true }).click();
  await expect(owner).toHaveURL(origin + "/admin/payment-methods");
  await expect(owner.getByText("No payment adapters installed.")).toBeVisible();
  await expect(published.getByText("No published methods.")).toBeVisible();
  await fits(1120);
  if (dir) await owner.screenshot({ path: join(dir, "builder-empty.png"), fullPage: true });
  expect(await read()).toMatchObject({ editable: true, eligible_count: null, adapters: [], published: [],
    draft: { open_limit: 3, connections: [], methods: [], custom_amount: true } });
  expect((await fetch(origin + "/admin/payment-methods", { redirect: "manual" })).headers.get("location")).toBe("/admin/login");
  expect((await customer.get(endpoint)).status()).toBe(401);
  expect((await http.get(api + "payments/methods")).status()).toBe(401);
  const original = await read();
  const probe = command(original.revision, { action: "check" });
  await reject(send(probe, { client: customer }), 401);
  await reject(send(probe, { csrf: "0".repeat(64) }), 403);
  await reject(send(probe, { src: "https://foreign.invalid" }), 403);
  await reject(post(options, probe), 400); // Options is not a way to check or save drafts.

  // The private worker installs a profile, not a draft or a published method.
  issue("checkout-empty"); await wait("checkout-ready");
  await owner.reload();
  const loaded = await read();
  expect(loaded.adapters).toHaveLength(1);
  expect(loaded.draft.connections).toEqual([]);
  const [adapter] = loaded.adapters;
  expect(JSON.stringify(loaded)).not.toContain("fixture-shop");
  expect(await getMethods()).toEqual([]);
  await owner.getByLabel("Installed adapter", { exact: true }).selectOption({ label: "Fixture payments" });
  await buttons("Add connection").click();
  await choose("Bank transfer");
  await choose("Card");
  await choose("Voucher");
  await buttons("Move Card up").click();
  await expect(owner.getByRole("list", { name: "Customer order" }).locator("strong")).toHaveText(["Card", "Bank transfer", "Voucher"]);
  await owner.getByLabel("Quick amounts, USD").fill("20 10 15");
  await owner.getByLabel("Allow custom amounts").uncheck();
  const cents = owner.getByLabel("Minimum credit, USD");
  await cents.fill("0.001");
  await expect(buttons("Save draft")).toBeDisabled();
  await cents.fill("5");
  await save();
  let current = await read();
  expect(current.draft.connections).toEqual([adapter.id]);
  expect(current.draft.quick_amounts).toEqual([2000, 1000, 1500]);
  await reject(send(probe), 409, "revision_conflict");
  await reject(send(command(current.revision, { action: "publish" })), 409, "checkout_check_required");
  const invalid = structuredClone(current.draft);
  invalid.methods.find(m => m.code === "bank").fee_payer = "customer";
  await reject(send(command(current.revision, { action: "save", draft: invalid })), 400);
  await reject(send(command(current.revision, { action: "save", draft: { ...current.draft, endpoint: "https://foreign.invalid" } })), 400);
  expect(await getMethods()).toEqual([]);

  await check();
  await expect(owner.getByRole("status")).toHaveText("Check complete. Methods available to customers: 2.");
  await expect(owner.getByRole("list", { name: "Customer order" }).getByRole("listitem").filter({ hasText: "Voucher" }))
    .toContainText("No eligible amounts");
  expect(await getMethods()).toEqual([]);
  const publishCurrent = () => command(current.revision, { action: "publish" });
  const expiring = await prepare(publishCurrent());
  const expiringProof = await ownerAssertion(owner, expiring.options);
  issue("checkout-check-aged"); await wait("checkout-check-expired");
  await reject(post(verify(expiring), expiringProof), 409, "checkout_check_required");
  await buttons("Publish").click();
  await buttons("Confirm publication").click();
  await expect(owner.getByRole("alert")).toHaveText("The check expired. Check the saved draft again.");
  expect(await getMethods()).toEqual([]);
  await check();

  const foreign = await owner.context().browser().newContext();
  try {
    const oldCsrf = (await ownerRead()).csrf_token;
    await foreign.addCookies(await owner.context().cookies(origin));
    await owner.context().clearCookies({ name: "__Host-relay_admin" });
    await owner.goto(origin + "/admin/login");
    await signInOwner(owner, origin);
    await owner.goto(origin + "/admin/payment-methods");
    expect((await ownerRead()).csrf_token).not.toBe(oldCsrf);
    const publication = publishCurrent();
    await reject(send(publication), 403, "fresh_passkey_required");
    await reject(post(options, publication, { client: customer }), 401);
    await reject(post(options, publication, { csrf: "00".repeat(32) }), 403);
    await reject(post(options, publication, { src: "https://foreign.invalid" }), 403);
    const selected = await prepare(publication), other = await prepare(publishCurrent());
    const proof = await ownerAssertion(owner, selected.options);
    await reject(post(verify(selected), proof, { client: foreign.request, csrf: oldCsrf }), 409, "passkey_ceremony_expired");
    await reject(post(verify(selected), proof, { src: "https://foreign.invalid" }), 403);
    await reject(post(verify(other), proof), 400, "invalid_credential");
    // A real assertion becomes obsolete if the owner edits the draft while its prompt is open.
    await reject(send(command(current.revision, { action: "save", draft: current.draft })), 200);
    await reject(post(verify(selected), proof), 409, "revision_conflict");
    await reject(post(verify(selected), proof), 409, "passkey_ceremony_expired");
  } finally { await foreign.close(); }
  expect(await getMethods()).toEqual([]);
  current = await read();
  await owner.reload();
  await check();

  const csrf = (await ownerRead()).csrf_token;
  const attempts = [];
  const observe = async route => {
    attempts.push(route.request().postDataJSON());
    await route.continue();
  };
  const cut = async route => {
    const proof = route.request().postDataJSON();
    expect(proof).toMatchObject({ type: "public-key" });
    expect(proof).not.toHaveProperty("command");
    const res = await route.fetch();
    expect(res.status()).toBe(200);
    expect((await res.json()).data).toEqual({ applied: true, operation_id: attempts[0].operation_id });
    await res.dispose();
    await route.abort("failed");
  };
  await owner.route(options, observe);
  await owner.route(verification + "*/verify", cut, { times: 1 });
  try {
    await buttons("Publish").click();
    await expect(buttons("Cancel")).toBeFocused();
    await buttons("Cancel").click();
    await expect(buttons("Publish")).toBeFocused();
    expect(await getMethods()).toEqual([]);
    await buttons("Publish").click();
    await buttons("Confirm publication").click();
    await expect(owner.getByRole("alert")).toHaveText("Could not confirm the result. Retry the same action.");
    const receipt = owner.waitForResponse(r => r.url() === options && r.ok());
    await buttons("Retry").click();
    expect((await (await receipt).json()).data).toEqual({ applied: true, operation_id: attempts[0].operation_id });
    await expect(owner.getByRole("status")).toHaveText("Checkout updated.");
  } finally { await owner.unroute(options, observe); }
  expect(attempts).toHaveLength(2);
  expect(attempts[0]).toEqual(attempts[1]);
  expect((await ownerRead()).csrf_token).toBe(csrf);
  await reject(send(attempts[0]), 200);
  await reject(send({ ...attempts[0], command: { action: "check" } }), 409, "revision_conflict");

  const first = await getMethods();
  expect(first.map(m => m.label)).toEqual(["Card", "Bank transfer"]);
  expect(first[0].custom_amount).toBe(false);
  const sellerQuote = await makeQuote(first[0].id, "10000000");
  expect(sellerQuote).toMatchObject({ credit_micro_usd: "10000000", fee_minor: "0", payable_minor: "1000" });
  await expect(published.getByText("Seller pays · Fee: $0.50")).toBeVisible();

  await owner.getByLabel("Who pays the fee for Card?").selectOption("customer");
  await owner.getByLabel("Allow custom amounts").check();
  await save();
  expect(await getMethods()).toEqual(first);
  await check();
  expect(await getMethods()).toEqual(first);
  await publish();
  const methods = await getMethods();
  expect(methods.map(m => m.label)).toEqual(["Card", "Bank transfer"]);
  expect(methods[0].custom_amount).toBe(true);
  expect(methods[1].custom_amount).toBe(false);
  expect(methods[0].quick_credit_micro_usd).toEqual(["20000000", "10000000", "15000000"]);
  expect(methods[0].id).not.toBe(first[0].id);
  expect(methods[1].id).toBe(first[1].id);
  await reject(confirm(sellerQuote), 409, "payment_method_unavailable");

  await fits(1120);
  if (dir) await owner.screenshot({ path: join(dir, "builder-published-desktop.png"), fullPage: true });
  await fits(320);
  await owner.emulateMedia({ colorScheme: "dark", reducedMotion: "reduce" });
  await owner.reload();
  await expect(owner.locator(".hosted-entry")).toHaveAttribute("data-theme", "dark");
  await expect(published.getByText("Customer pays · Fee: $0.50")).toBeVisible();
  await setWorkspaceLanguage(owner, "ru");
  await expect(owner.getByRole("heading", { name: "Способы оплаты", exact: true })).toBeVisible();
  await fits(320);
  if (dir) await owner.screenshot({ path: join(dir, "builder-published-320-ru-dark.png"), fullPage: true });
  await setWorkspaceLanguage(owner, "zh-CN");
  await expect(owner.getByRole("heading", { name: "付款方式", exact: true })).toBeVisible();
  await fits(320);
  await setWorkspaceLanguage(owner, "en");
  await expect(owner.getByRole("heading", { name: "Payment methods", exact: true })).toBeVisible();

  return { methods, lowerLimitAndDisable: async () => {
    await owner.getByLabel("Open invoice limit").fill("2");
    await save();
    expect((await read()).published_open_limit).toBe(3);
    await check(); await publish();
    expect((await read()).published_open_limit).toBe(2);
    expect(await getMethods()).toEqual(methods);
    current = await read();
    await reject(send(command(current.revision, { action: "disable", method_id: first[0].id })), 409, "revision_conflict");
    await reject(send(command(current.revision, { action: "disable", method_id: methods[0].id })), 403, "fresh_passkey_required");
    await buttons("Disable Card").click();
    await expect(buttons("Cancel")).toBeFocused();
    await fits(320);
    if (dir) await owner.screenshot({ path: join(dir, "builder-disable-320-dark.png"), fullPage: true });
    await buttons("Confirm disabling").click();
    await expect(owner.getByRole("status")).toHaveText("Method disabled.");
    expect((await read()).draft.methods.map(m => m.code)).toEqual(["bank", "voucher"]);
    expect(await getMethods()).toEqual([methods[1]]);
  } };
}
