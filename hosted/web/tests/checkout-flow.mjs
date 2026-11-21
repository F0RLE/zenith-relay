import { expect } from "@playwright/test";
import { readFixture } from "../../test-support/files.mjs";
import { join } from "node:path";
import { configureCheckout } from "./payment-builder-flow.mjs";

export async function checkCheckout({ a, b, secondA, owner, ownerRead, origin, api, acctA, acctB, exchange, issue, wait }) {
  const path = "/app/billing";
  const secondAccount = (await (await secondA.context().request.get(api + "account")).json()).data;
  const request = (page, method, suffix, data, account = page === b ? acctB : page === secondA ? secondAccount : acctA) => page.context().request.fetch(api + suffix, {
    method, ...(data && { data, headers: { Origin: origin, "X-Relay-CSRF": account.csrf_token } }),
  });
  const get = async (page, suffix) => {
    const r = await request(page, "GET", suffix);
    expect(r.status()).toBe(200);
    return (await r.json()).data;
  };
  const post = async (page, suffix, data) => {
    const r = await request(page, "POST", suffix, data);
    expect(r.status()).toBe(200);
    return (await r.json()).data;
  };
  const makeQuote = async (page, method_id, credit_micro_usd) => (await post(page, "payments/quotes", {
    method_id, credit_micro_usd,
  })).quote;
  const confirmation = q => ({ quote_id: q.id, operation_id: crypto.randomUUID() });
  const wallet = dollars => expect(a.locator(".entry-wallet dd")).toHaveText(dollars + " USD");
  const history = a.locator("#top-up-history");
  const readPayment = async id => (await get(a, "payments/" + id)).payment;
  const refresh = () => history.getByRole("button", { name: "Refresh payments" }).click();
  const review = () => a.getByRole("button", { name: "Review", exact: true }).click();

  await a.setViewportSize({ width: 320, height: 760 });
  await a.goto(origin + path); await wallet("0.00");
  expect((await get(a, "payments/methods")).items).toEqual([]);
  await expect(a.getByRole("heading", { name: "Top up balance" })).toHaveCount(0);
  const builder = await configureCheckout({ owner, ownerRead, customer: a.context().request, origin, api, issue, wait,
    getMethods: async () => (await get(a, "payments/methods")).items,
    makeQuote: (id, amount) => makeQuote(a, id, amount),
    confirm: quote => request(a, "POST", "payments", confirmation(quote)),
  });
  await a.reload();
  await expect(a.getByRole("heading", { name: "Top up balance" })).toBeVisible();
  const [card, bank] = builder.methods;
  const chosenAmount = a.getByLabel("Amount, USD");
  const select = a.getByRole("combobox", { name: "Payment method" });
  await select.click();
  await a.getByRole("option", { name: "Bank transfer" }).click();
  await expect(chosenAmount).toHaveCount(0);
  await select.click();
  await a.getByRole("option", { name: "Card" }).click();
  await chosenAmount.fill("10.005");
  await expect(a.getByRole("button", { name: "Review", exact: true })).toBeDisabled();
  await a.getByRole("button", { name: "10.00 USD", exact: true }).click();
  if (process.env.RELAY_CHECKOUT_SCREENSHOTS)
    await a.screenshot({ path: join(process.env.RELAY_CHECKOUT_SCREENSHOTS, "checkout-form-320.png"), fullPage: true });
  await review();
  const reviewHeading = a.getByRole("heading", { name: "Review top-up" });
  await expect(reviewHeading).toBeFocused();
  const totals = a.locator(".checkout-totals");
  for (const [label, value] of [["Wallet credit", "10.00 USD"], ["Processing fee", "0.50 USD"], ["Total to pay", "10.50 USD"]])
    await expect(totals.locator("div").filter({ hasText: label }).locator("dd")).toHaveText(value);
  expect(await a.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  const actions = await a.locator(".checkout-actions button").evaluateAll(bs => bs.map(b => b.getBoundingClientRect().top));
  expect(actions[0]).toBe(actions[1]);
  if (process.env.RELAY_CHECKOUT_SCREENSHOTS) {
    const dir = process.env.RELAY_CHECKOUT_SCREENSHOTS;
    await a.screenshot({ path: join(dir, "checkout-320.png"), fullPage: true });
    await a.setViewportSize({ width: 1100, height: 820 });
    await a.screenshot({ path: join(dir, "checkout-desktop.png"), fullPage: true });
    await a.setViewportSize({ width: 320, height: 760 });
  }
  await a.getByRole("button", { name: "Cancel", exact: true }).click();
  expect((await get(a, "payments")).items).toEqual([]);
  const rejectedChoice = await request(a, "POST", "payments/quotes", {
    method_id: bank.id, credit_micro_usd: "12500000",
  });
  expect(rejectedChoice.status()).toBe(400);
  const invalidCsrf = await request(a, "POST", "payments/quotes", {
    method_id: card.id, credit_micro_usd: "10000000",
  }, acctB);
  expect(invalidCsrf.status()).toBe(403);
  const expiry = confirmation(await makeQuote(a, card.id, "12500000"));
  expect((await request(b, "POST", "payments", expiry)).status()).toBe(404);
  issue("checkout-reviewed", expiry.quote_id); await wait("checkout-expired");
  expect([404, 410]).toContain((await request(a, "POST", "payments", expiry)).status());

  const quoteReply = a.waitForResponse(r => r.url() === api + "payments/quotes" && r.ok());
  await review();
  const shortQuote = (await (await quoteReply).json()).data.quote;
  issue("checkout-shorten-quote", shortQuote.id); await wait("checkout-short-quote");
  const quoteDeadline = Number(readFixture(exchange, "checkout-short-quote"));
  let ack, submitted;
  const opRoute = url => url.pathname.endsWith("/payments") && url.searchParams.has("operation_id");
  await a.route(opRoute, route => route.abort());
  await a.route(api + "payments", async route => {
    if (route.request().method() !== "POST") return route.continue();
    submitted = route.request().postDataJSON();
    const r = await route.fetch();
    expect(r.status()).toBe(200);
    ack = (await r.json()).data.payment;
    await route.abort();
  });
  await a.getByRole("button", { name: "Create invoice" }).click();
  await expect(a.getByRole("button", { name: "Retry confirmation" })).toBeEnabled();
  expect(ack.state).toBe("processing");
  await a.unroute(api + "payments"); await a.unroute(opRoute);
  await a.reload();
  await expect(history.getByText("Checking invoice")).toBeVisible();
  await expect(a.getByRole("button", { name: "Retry confirmation" })).toHaveCount(0);
  issue("checkout-confirmed", ack.id); await wait("checkout-uncertain");
  const same = (await post(a, "payments", submitted)).payment;
  expect(same.id).toBe(ack.id);
  expect(await readPayment(ack.id)).toEqual(same);
  const reopen = "payments/" + ack.id + "/resume";
  expect(await post(a, reopen, {})).toMatchObject({ handoff_url: null, payment: { can_resume: false } });
  const expiresUnpaid = (await post(b, "payments", confirmation(await makeQuote(b, card.id, "10000000")))).payment;
  expect((await request(b, "GET", "payments/" + ack.id)).status()).toBe(404);
  expect((await get(b, "payments?operation_id=" + submitted.operation_id)).items).toEqual([]);
  const bankQuote = await makeQuote(a, bank.id, "10000000");
  expect((await request(a, "POST", "payments", { ...submitted, quote_id: bankQuote.id })).status()).toBe(409);
  expect((await request(a, "POST", "payments", { ...submitted, operation_id: crypto.randomUUID() })).status()).toBe(409);
  expect((await post(a, "payments", confirmation(bankQuote))).payment.id).not.toBe(ack.id);
  const candidates = await Promise.all([makeQuote(a, bank.id, "20000000"), makeQuote(secondA, bank.id, "15000000")]);
  const x = candidates.map(confirmation);
  const results = await Promise.all([request(a, "POST", "payments", x[0]), request(secondA, "POST", "payments", x[1])]);
  expect(results.map(r => r.status()).sort()).toEqual([200, 429]);
  const limited = results.findIndex(r => r.status() === 429);
  expect((await results[limited].json()).error).toMatchObject({ code: "open_invoice_limit", details: { open_count: 3, open_limit: 3 } });
  expect((await request(a, "POST", "payments", x[limited])).status()).toBe(429);
  const independent = await makeQuote(b, bank.id, "10000000");
  expect((await post(b, "payments", confirmation(independent))).payment.state).toBe("processing");
  const retiring = confirmation(await makeQuote(a, card.id, "14000000"));
  await builder.lowerLimitAndDisable();
  const reduced = await request(a, "POST", "payments", x[limited]);
  expect(reduced.status()).toBe(429);
  expect((await reduced.json()).error.details).toEqual({ open_count: 3, open_limit: 2 });
  expect((await get(a, "payments")).items).toHaveLength(3);
  issue("checkout-cap", expiresUnpaid.id); await wait("checkout-found");

  await refresh();
  await expect(history.getByText("Awaiting payment")).toHaveCount(1);
  const continuePayment = history.getByRole("button", { name: "Continue payment" });
  await expect(continuePayment).toHaveCount(1);
  expect(await readPayment(ack.id)).toMatchObject({ id: ack.id, can_resume: true });
  const destination = "https://checkout.merchant.invalid/invoice/" + ack.id;
  expect(JSON.stringify(await get(a, "payments"))).not.toContain("checkout.merchant.invalid");
  await expect(history.locator("a[href^='https://']")).toHaveCount(0);
  expect((await request(b, "POST", reopen, {})).status()).toBe(404);
  expect((await request(a, "POST", reopen, {}, acctB)).status()).toBe(403);
  expect((await a.request.post(api + reopen)).status()).toBe(403);
  expect((await get(a, "payments/methods")).items.map(m => m.id)).toEqual([bank.id]);
  const unavailable = await request(a, "POST", "payments", retiring);
  expect(unavailable.status()).toBe(409);
  expect((await unavailable.json()).error.code).toBe("payment_method_unavailable");

  // Leave the original quote behind. Both attempts must go to the same invoice,
  // including after returning to the cabinet and loading fresh history.
  await expect.poll(() => Math.floor(Date.now() / 1000), { timeout: 20000 }).toBeGreaterThanOrEqual(quoteDeadline);
  await a.route("https://checkout.merchant.invalid/**", route => {
    expect(route.request().url()).toBe(destination);
    expect(route.request().headers()["referer"]).toBeUndefined();
    return route.fulfill({ contentType: "text/html", body: "<h1>Original invoice</h1>" });
  });
  if (process.env.RELAY_CHECKOUT_SCREENSHOTS) {
    const dir = process.env.RELAY_CHECKOUT_SCREENSHOTS;
    await a.screenshot({ path: join(dir, "checkout-resume-320.png"), fullPage: true });
    await a.setViewportSize({ width: 1120, height: 820 });
    await a.emulateMedia({ colorScheme: "dark" });
    await a.screenshot({ path: join(dir, "checkout-resume-dark.png"), fullPage: true });
    await a.emulateMedia({ colorScheme: "light" });
    await a.setViewportSize({ width: 320, height: 760 });
  }
  expect(await a.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  const [button, section] = await Promise.all([continuePayment.boundingBox(), history.boundingBox()]);
  expect(button.height).toBeGreaterThanOrEqual(44);
  expect(button.x + button.width).toBeLessThanOrEqual(section.x + section.width);
  for (let i = 0; i < 2; i++) {
    await continuePayment.click();
    await expect(a).toHaveURL(destination);
    await expect(a.getByRole("heading", { name: "Original invoice" })).toBeVisible();
    await a.goto(origin + path); await wallet("0.00");
  }
  issue("checkout-resumed-twice");
  await wait("checkout-mismatch-ready");
  await continuePayment.click();
  await expect(a.getByRole("alert")).toHaveText(/Couldn’t reopen the invoice/);
  await expect(continuePayment).toHaveCount(0);
  await expect(history.getByRole("heading", { name: "Payment history" })).toBeFocused();
  expect(await readPayment(ack.id)).toMatchObject({ can_resume: false, receipt_id: null });
  await expect(a).toHaveURL(origin + path);
  issue("checkout-mismatch-checked"); await wait("checkout-mismatch-cleared");

  await b.goto(origin + path);
  await expect(b.locator("#top-up-history").getByText("Checking invoice").first()).toBeVisible();
  issue("checkout-start-expiry"); await wait("checkout-expiring");
  await b.getByRole("button", { name: "Refresh payments" }).click();
  const staleExpiry = b.getByRole("button", { name: "Continue payment" });
  await expect(staleExpiry).toBeVisible();
  issue("checkout-expiring-visible"); await wait("checkout-invoice-expired");
  await staleExpiry.click();
  await expect(staleExpiry).toHaveCount(0);
  await expect(b).toHaveURL(origin + path);
  expect((await get(b, "payments/" + expiresUnpaid.id)).payment).toMatchObject({ can_resume: false, receipt_id: null });

  await a.goto(origin + path + "?paid=1&payment_id=" + ack.id);
  await wallet("0.00");
  await expect(continuePayment).toBeVisible();
  expect((await request(a, "POST", "payments/webhooks/fixture", { payment_id: ack.id, status: "paid" })).status()).toBe(404);
  expect((await readPayment(ack.id)).state).toBe("awaiting_payment");
  issue("checkout-fake-return"); await wait("checkout-pay-during-resume");
  const concurrent = a.waitForResponse(r => r.url() === api + reopen);
  await continuePayment.click();
  expect((await (await concurrent).json()).data).toMatchObject({ handoff_url: null, payment: { state: "paid", can_resume: false } });
  await wait("funded");
  await wallet("10.00");
  await expect(continuePayment).toHaveCount(0);
  expect(await post(a, reopen, {})).toMatchObject({ handoff_url: null, payment: { state: "paid", can_resume: false } });
  await refresh();
  await expect(history.getByText("Closed", { exact: true })).toHaveCount(1);
  const paid = await readPayment(ack.id);
  expect(paid).toMatchObject({ state: "paid", credit_micro_usd: "10000000", payable_minor: "1050" });
  const released = (await post(a, "payments", x[limited])).payment;
  expect((await post(a, "payments", x[limited])).payment.id).toBe(released.id);
  expect((await get(a, "payments")).items).toHaveLength(4);
  expect((await get(b, "account")).wallet.available_micro_usd).toBe("0");
  await history.getByRole("link", { name: "Wallet entry" }).click();
  await expect(a).toHaveURL(origin + path + "?id=" + paid.receipt_id);
  await expect(a.getByRole("listitem").filter({ hasText: "Credited" })).toHaveText(/10\.000000 USD/);
  await a.goto(origin + "/app/overview");
  await a.setViewportSize({ width: 360, height: 760 });
  if (process.env.RELAY_CHECKOUT_SCREENSHOTS) {
    await b.emulateMedia({ colorScheme: "dark" });
    await b.goto(origin + path);
    await expect(b.locator('.hosted-app[data-workspace="customer"]')).toHaveAttribute("data-theme", "dark");
    await expect(b.locator("#top-up-history").getByText("Checking invoice").first()).toBeVisible();
    await b.screenshot({ path: join(process.env.RELAY_CHECKOUT_SCREENSHOTS, "checkout-history-dark.png"), fullPage: true });
    await b.emulateMedia({ colorScheme: "light" });
    await b.goto(origin + "/app/overview");
  }
  return paid;
}
