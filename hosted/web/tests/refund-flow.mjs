import { expect } from "@playwright/test";
import { join } from "node:path";
import { ownerAssertion } from "./owner-session.mjs";

export async function checkRefunds({ a, b, owner, ownerRead, origin, api, acctA, acctB, issue, wait }, original) {
  const customer = a.context().request, peer = b.context().request, admin = owner.context().request;
  const billing = origin + "/app/billing", payment = "payments/" + original.id;
  const request = payment + "/refund-requests";
  const post = (client, path, csrf, data, source = origin) => client.post(api + path, {
    headers: { Origin: source, "X-Relay-CSRF": csrf }, data,
  });
  const read = async (client, path) => {
    const response = await client.get(api + path);
    expect(response.status()).toBe(200);
    return (await response.json()).data;
  };
  const currentPayment = async () => (await read(customer, payment)).payment;
  const expectWallet = async (available, held = "0") => {
    expect((await read(customer, "account")).wallet).toMatchObject({
      available_micro_usd: available, reserved_micro_usd: held,
      refund_reserved_micro_usd: held, inference_reserved_micro_usd: "0",
    });
  };
  const history = a.locator("#top-up-history");
  const refresh = () => history.getByRole("button", { name: "Refresh payments" }).click();
  const customerStatus = (state, amount) => expect(history.getByText(`${state} · ${amount} USD`, { exact: true })).toBeVisible();
  const refundRows = owner.locator(".owner-refunds li");
  const details = id => refundRows.filter({ has: owner.locator(`a[href$="${id}"]`) });
  const adminRefresh = () => owner.getByRole("button", { name: "Refresh refunds" }).click();
  const decision = id => "admin/payments/refunds/" + id;
  const filled = (amount, operation_id = crypto.randomUUID()) => ({ amount_micro_usd: amount, operation_id });
  const capture = process.env.RELAY_CHECKOUT_SCREENSHOTS;
  const fit = async page => expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);

  expect((await post(peer, request, acctB.csrf_token, filled("2000000"))).status()).toBe(404);
  expect((await post(customer, request, acctB.csrf_token, filled("2000000"))).status()).toBe(403);
  expect((await post(customer, request, acctA.csrf_token, filled("2000000"), "https://foreign.invalid")).status()).toBe(403);
  expect((await post(customer, request, acctA.csrf_token, filled("2000001"))).status()).toBe(400);
  expect((await customer.get(api + "admin/payments")).status()).toBe(401);

  await a.setViewportSize({ width: 320, height: 760 });
  await a.goto(billing);
  const open = () => history.getByRole("button", { name: "Request refund", exact: true }).click();
  const amount = history.getByLabel("Refund amount, USD");
  const send = history.getByRole("button", { name: "Send request", exact: true });
  await open();
  await expect(amount).toBeFocused();
  await amount.fill("10.50");
  await expect(send).toBeDisabled();
  await amount.fill("2.00");
  await fit(a);
  if (capture) await a.screenshot({ path: join(capture, "refund-customer-320.png"), fullPage: true });

  let operation;
  await a.route(api + request, async route => {
    operation = route.request().postDataJSON();
    const created = await route.fetch();
    expect(created.status()).toBe(200);
    await route.abort();
  });
  await send.click();
  await expect(history.getByRole("alert")).toHaveText(/Couldn’t confirm the request/);
  await expect(amount).toBeDisabled();
  await a.unroute(api + request);
  const retry = a.waitForRequest(r => r.url() === api + request);
  await history.getByRole("button", { name: "Retry", exact: true }).click();
  expect((await retry).postDataJSON()).toEqual(operation);
  await customerStatus("Refund requested", "2.00");
  await expect(history.locator(".refund-state")).toBeFocused();
  const requested = (await currentPayment()).refund;
  expect(requested).toMatchObject({ state: "requested", amount_micro_usd: "2000000", receipt_id: null });
  expect((await post(customer, request, acctA.csrf_token, { ...operation, amount_micro_usd: "1000000" })).status()).toBe(409);
  expect((await post(customer, request, acctA.csrf_token, filled("2000000"))).status()).toBe(409);
  await expectWallet("10000000");

  await owner.setViewportSize({ width: capture ? 1040 : 320, height: 760 });
  await owner.emulateMedia({ colorScheme: capture ? "dark" : "light" });
  await owner.goto(origin + "/admin/overview");
  await owner.getByRole("link", { name: "Refunds", exact: true }).click();
  await expect(owner).toHaveURL(origin + "/admin/payments");
  const row = details(original.id).first();
  await expect(row).toHaveText(/Refund requested/);
  await expect(owner.getByRole("navigation").getByRole("link", { name: "Refunds", exact: true })).toHaveCount(0);
  const openReview = () => row.getByRole("button", { name: "Approve", exact: true }).click();
  if (capture) {
    await expect(owner.locator(".hosted-entry")).toHaveAttribute("data-theme", "dark");
    await openReview();
    await owner.screenshot({ path: join(capture, "refund-owner-dark.png"), fullPage: true });
    await owner.setViewportSize({ width: 320, height: 760 });
    await owner.emulateMedia({ colorScheme: "light" });
    await owner.reload();
  }
  await expect(owner.locator(".hosted-entry")).toHaveAttribute("data-theme", "light");
  await openReview();
  await expect(row.getByRole("button", { name: "Cancel" })).toBeFocused();
  await expect(row.getByText("Reserve 2.00 USD and refund it to the original payment method?")).toBeVisible();
  await fit(owner); await expectWallet("10000000");
  if (capture) await owner.screenshot({ path: join(capture, "refund-owner-320.png"), fullPage: true });
  issue("refund-requested", requested.id);
  await wait("refund-passkey-aged");
  const old = (await ownerRead()).csrf_token;
  const command = { operation_id: crypto.randomUUID(), expected_state: "requested", action: "approve" };
  const options = decision(requested.id) + "/options";
  const verification = "admin/payments/confirmations/";
  expect((await post(customer, decision(requested.id), old, command)).status()).toBe(401);
  expect((await post(customer, options, old, command)).status()).toBe(401);
  expect((await post(admin, decision(requested.id), "00".repeat(32), command)).status()).toBe(403);
  expect((await post(admin, decision(requested.id), old, { ...command, expected_state: "queued" })).status()).toBe(400);
  const stale = await post(admin, decision(requested.id), old, command);
  expect(stale.status()).toBe(403);
  expect((await stale.json()).error.code).toBe("fresh_passkey_required");

  const prepare = async action => {
    const started = await post(admin, options, old, { ...command, action });
    expect(started.status()).toBe(200);
    const challenge = (await started.json()).data;
    expect(challenge.options.publicKey.userVerification).toBe("required");
    return challenge;
  };
  const approve = await prepare("approve"), decline = await prepare("decline");
  const proof = await ownerAssertion(owner, approve.options);
  const wrong = await post(admin, `${verification}${decline.ceremony_id}/verify`, old, proof);
  expect(wrong.status()).toBe(400);
  expect((await wrong.json()).error.code).toBe("invalid_credential");
  await expectWallet("10000000");
  expect((await currentPayment()).refund.state).toBe("requested");

  let applied;
  await owner.route(api + options, async route => {
    applied = route.request().postDataJSON();
    await route.continue();
  }, { times: 1 });
  await owner.route(api + verification + "*/verify", async route => {
    const assertion = route.request().postDataJSON();
    expect(assertion).toMatchObject({ type: "public-key" });
    expect(assertion).not.toHaveProperty("action");
    const result = await route.fetch();
    expect(result.status()).toBe(200);
    expect((await result.json()).data).toEqual({ applied: true, operation_id: applied.operation_id });
    await result.dispose();
    const spent = await post(admin, route.request().url().slice(api.length), old, assertion);
    expect(spent.status()).toBe(409);
    expect((await spent.json()).error.code).toBe("passkey_ceremony_expired");
    await route.abort();
  }, { times: 1 });
  const confirm = () => row.getByRole("button", { name: "Confirm refund", exact: true }).click();
  await confirm();
  await expect(owner.getByRole("alert")).toHaveText(/Couldn’t confirm the decision/);
  expect(applied).toMatchObject({ action: "approve", expected_state: "requested" });
  await expectWallet("8000000", "2000000");
  const acknowledgment = owner.waitForResponse(r => r.url() === api + options);
  await confirm();
  const replay = await acknowledgment;
  expect(replay.status()).toBe(200);
  expect(replay.request().postDataJSON()).toEqual(applied);
  expect((await replay.json()).data).toEqual({ applied: true, operation_id: applied.operation_id });
  await expect(row).toHaveText(/Approved · awaiting payment provider/);
  expect((await ownerRead()).csrf_token).toBe(old);
  expect((await post(admin, decision(requested.id), old, applied)).status()).toBe(200);
  expect((await post(admin, decision(requested.id), old, { ...applied, action: "decline" })).status()).toBe(409);
  expect((await post(admin, decision(requested.id), old, command)).status()).toBe(409);
  await refresh();
  await customerStatus("Approved · awaiting payment provider", "2.00");
  await expect(a.locator(".entry-wallet dd")).toHaveText(["8.00 USD", "2.000000 USD"]);
  await expect(a.locator(".wallet-reserve-breakdown")).toHaveText(/For refunds.*2\.000000 USD/);
  await expectWallet("8000000", "2000000");
  issue("refund-approved", requested.id);

  await wait("refund-unknown"); await refresh();
  await customerStatus("Awaiting refund confirmation", "2.00");
  await expectWallet("8000000", "2000000");
  issue("refund-read-unknown"); await wait("refund-succeeded");
  await refresh(); await customerStatus("Refunded", "2.00");
  await expectWallet("8000000");
  const refunded = await currentPayment();
  expect(refunded).toMatchObject({ receipt_id: original.receipt_id, credit_micro_usd: original.credit_micro_usd,
    payable_minor: original.payable_minor, refundable_micro_usd: "8000000",
    refund: { id: requested.id, state: "succeeded" } });
  expect((await read(peer, "wallet/entries?id=" + refunded.refund.receipt_id)).items).toEqual([]);
  expect((await post(customer, request, acctA.csrf_token, filled("8010000"))).status()).toBe(409);
  await history.getByRole("link", { name: "Refund receipt" }).click();
  await expect(a).toHaveURL(billing + "?id=" + refunded.refund.receipt_id);
  await expect(a.getByRole("listitem").filter({ hasText: "Payment refund" })).toHaveText(/−2\.000000 USD/);

  await a.goto(billing);
  const openNext = async n => {
    const created = a.waitForResponse(r => r.url() === api + request && r.ok());
    await open(); await amount.fill(n); await send.click(); await created;
    return (await currentPayment()).refund;
  };
  const second = await openNext("1");
  await customerStatus("Refund requested", "1.00"); await adminRefresh();
  await row.getByRole("button", { name: "Approve", exact: true }).click();
  await row.getByRole("button", { name: "Confirm refund", exact: true }).click();
  await expect(row).toHaveText(/Approved · awaiting payment provider/);
  await expectWallet("7000000", "1000000");
  issue("refund-second-approved", second.id); await wait("refund-failed");
  await refresh(); await customerStatus("Refund failed", "1.00");
  await expectWallet("8000000");
  await expect(a.getByRole("listitem").filter({ hasText: "Refund reserve released" })).toHaveText(/1\.000000 USD/);

  const third = await openNext("0.50"); await customerStatus("Refund requested", "0.50"); await adminRefresh();
  await row.getByRole("button", { name: "Decline", exact: true }).click();
  await row.getByRole("button", { name: "Confirm decline", exact: true }).click();
  await expect(row).toHaveText(/Request declined/);
  await refresh(); await customerStatus("Request declined", "0.50");
  await expectWallet("8000000");
  issue("refund-declined", third.id);
  await owner.setViewportSize({ width: 1100, height: 820 });
  await a.setViewportSize({ width: 360, height: 760 });
  const result = await currentPayment();
  await a.goto(origin + "/app/overview");
  return result;
}
