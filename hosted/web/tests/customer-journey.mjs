import { expect } from "@playwright/test";
import { fixtureExists, writeFixture, fixtureOrigin } from "../../test-support/files.mjs";
import { startIssuer } from "./oidc-issuer.mjs";
import { checkOwnKeys } from "./own-keys.mjs";
import { checkPublishedCatalog } from "./published-catalog.mjs";
import { pairDesktop, revokeDesktop } from "./paired-desktop.mjs";
import { checkUsageReport } from "./usage-report-flow.mjs";
import { checkServiceNotices } from "./service-notices.mjs";
import { checkCheckout } from "./checkout-flow.mjs";
import { checkRefunds } from "./refund-flow.mjs";

export async function checkCustomerJourney({ browser, owner, origin, issuer, exchange, funded }) {
  origin = fixtureOrigin(origin);
  const api = origin + "/api/hosted/v1/";
  const issue = (file, value = "") => writeFixture(exchange, file, value);
  const wait = file => expect.poll(() => fixtureExists(exchange, file), { timeout: 25000 }).toBe(true);
  await expect.poll(async () => {
    try { return (await fetch(api + "public/config")).status; } catch { return 0; }
  }, { timeout: 15000 }).toBe(200);
  const cfg = (await (await fetch(api + "public/config")).json()).data;
  expect(cfg.registration_open).toBe(false);
  const base = api + "auth/customer/" + cfg.login_methods[0], callback = base + "/callback";
  const idp = await startIssuer(issuer, callback, "relay-fixture");
  const loginLabel = "Sign in with your identity provider";
  const pageFor = async (subject, width = 1024) => {
    const ctx = await browser.newContext({ viewport: { width, height: 760 } });
    await ctx.addCookies([{ url: issuer, name: "test_subject", value: subject }]);
    return ctx.newPage();
  };
  const login = async page => {
    await page.goto(origin + "/login");
    await page.getByRole("button", { name: loginLabel }).click();
  };
  const candidate = async page => {
    await login(page);
    await expect(page.getByRole("heading", { name: "Your review ID" })).toBeVisible();
    return page.locator(".entry-review-id").innerText();
  };
  const signOwner = async () => {
    await owner.goto(origin + "/admin");
    await owner.getByRole("button", { name: "Sign in with a passkey" }).click();
    await expect(owner.getByRole("button", { name: "Sign out" })).toBeVisible();
    return owner.context().request;
  };
  const account = async page => (await (await page.context().request.get(api + "account")).json()).data;
  const balance = (page, dollars) => expect(page.locator(".entry-wallet dd")).toHaveText(dollars + " USD");
  const fundedBalance = "8.799400"; // $10 minus $1.20 and the $0.000600 net corrected charge.
  const finalBalance = funded ? fundedBalance : "8.00";
  const mobileFits = async page => {
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  };
  try {
    const a = await pageFor("alice", 360), b = await pageFor("bob"), other = await pageFor("outsider");
    const ownerHttp = await signOwner();
    const ownerRead = async () => (await (await ownerHttp.get(api + "admin/overview")).json()).data;
    await owner.getByRole("link", { name:"Connections", exact:true }).click();
    await expect(owner.getByText("No saved connections")).toBeVisible();
    await pairDesktop({owner,ownerRead,origin,exchange,wait,issue});
    await owner.getByRole("link", { name:"Overview", exact:true }).click();
    expect((await ownerHttp.get(api + "account")).status()).toBe(401);
    const idA = await candidate(a), idB = await candidate(b);
    expect(idA).not.toBe(idB); await mobileFits(a);
    expect(await candidate(a)).toBe(idA);
    for (const subject of ["wrong-issuer","wrong-audience","wrong-nonce","expired","not-yet-valid","wrong-signature","wrong-party"]) {
      await other.context().addCookies([{ url: issuer, name: "test_subject", value: subject }]);
      await login(other);
      await expect(other.getByRole("alert")).toHaveText(/Access was not granted/);
    }
    await other.context().addCookies([{ url: issuer, name: "test_subject", value: "outsider" }]);
    expect((await a.context().request.get(api + "account")).status()).toBe(401);
    const request = { operation_id: crypto.randomUUID(), expected_revision: "1", target: "qualification",
      reason_code: "verify_customer_access", candidate_ids: [idA, idB] };
    const change = (client, csrf, body, src = origin) => client.post(api + "admin/operations/service/transitions",
      { headers: { Origin: src, "X-Relay-CSRF": csrf }, data: body });
    let csrf = (await ownerRead()).csrf_token;
    expect((await change(a.context().request, csrf, request)).status()).toBe(401);
    expect((await change(ownerHttp, "00".repeat(32), request)).status()).toBe(403);
    expect((await change(ownerHttp, csrf, request, issuer)).status()).toBe(403);
    expect((await change(ownerHttp, csrf, { ...request, candidate_ids: [crypto.randomUUID()] })).status()).toBe(400);
    expect((await change(ownerHttp, csrf, { ...request, target: "live" })).status()).toBe(400);
    const stale = await a.context().request.post(base + "/begin", { headers: { Origin: origin } });
    const staleLocation = (await a.context().request.get((await stale.json()).data.authorization_url, { maxRedirects: 0 })).headers().location;
    issue("review"); await wait("approve");
    const oldProof = await change(ownerHttp, csrf, request);
    expect(oldProof.status()).toBe(403);
    expect((await oldProof.json()).error.code).toBe("fresh_passkey_required");
    const transition = api + "admin/operations/service/transitions";
    const post = owner.waitForRequest(r => r.url() === transition);
    await owner.getByLabel("Review IDs (one per tester)").fill(`${idA}, ${idB}`);
    await owner.getByRole("button", { name: "Approve 24-hour access" }).click();
    await expect(owner.getByRole("button", { name: "Pause customer access" })).toBeVisible();
    const applied = (await post).postDataJSON();
    csrf = (await ownerRead()).csrf_token;
    expect((await change(ownerHttp, csrf, applied)).status()).toBe(200);
    expect((await change(ownerHttp, csrf, { ...applied, candidate_ids: [idA] })).status()).toBe(409);
    expect((await change(ownerHttp, csrf, request)).status()).toBe(409);
    expect((await a.context().request.get(staleLocation,{ maxRedirects: 0 })).headers().location).toBe("/login#denied");

    // A foreign cookie or wrong state must not use up the legitimate browser attempt.
    const probe = a.context().request;
    expect((await probe.post(base + "/begin")).status()).toBe(403);
    const options = await probe.post(base + "/begin", { headers: { Origin: origin } });
    const authUrl = (await options.json()).data.authorization_url;
    const redirect = (await probe.get(authUrl, { maxRedirects: 0 })).headers().location;
    const before = idp.counts.exchanges;
    const wrong = new URL(redirect); wrong.searchParams.set("state", "00".repeat(32));
    await probe.get(wrong.href,{ maxRedirects: 0 });
    await other.context().request.get(redirect,{ maxRedirects: 0 });
    expect(idp.counts.exchanges).toBe(before);
    const secondA = await pageFor("alice");
    await Promise.all([a.goto(redirect), login(secondA), ...(funded ? [login(b)] : [])]);
    await Promise.all([balance(a,"0.00"), balance(secondA,"0.00"), ...(funded ? [balance(b,"0.00")] : [])]);
    const count = idp.counts.exchanges;
    await probe.get(redirect,{ maxRedirects: 0 });
    expect(idp.counts.exchanges).toBe(count);
    const notices = funded ? null : await checkServiceNotices({ a,b,owner,origin,api,ownerRead,login,issue,wait });
    const acctA = await account(a), acctB = await account(b);
    expect((await account(secondA)).workspace_id).toBe(acctA.workspace_id);
    expect(acctA.workspace_id).not.toBe(acctB.workspace_id);
    expect((await b.context().request.get(api + "account?workspace_id=" + acctA.workspace_id)).status()).toBe(200);
    expect((await (await b.context().request.get(api + "account?workspace_id=" + acctA.workspace_id)).json()).data.workspace_id).toBe(acctB.workspace_id);
    expect((await probe.get(api + "admin/overview")).status()).toBe(401);
    expect((await probe.get(api + "admin/devices")).status()).toBe(401);
    expect((await probe.get(api + "device/connections")).status()).toBe(401);
    expect((await probe.post(api + "auth/customer/logout", { headers: { Origin: origin, "X-Relay-CSRF":acctB.csrf_token } })).status()).toBe(403);
    const session = (await a.context().cookies()).find(v => v.name === "__Host-relay_customer");
    expect(session).toMatchObject({ httpOnly: true, secure: true, sameSite: "Strict" });
    await login(other); await expect(other.getByRole("alert")).toHaveText(/Access was not granted/);
    expect((await other.context().request.get(api + "account")).status()).toBe(401);
    issue("customers",JSON.stringify([acctA.workspace_id,acctB.workspace_id]));
    const checkout = { a,b,secondA,owner,ownerRead,origin,api,acctA,acctB,exchange,issue,wait };
    const payment = funded ? null : await checkRefunds(checkout, await checkCheckout(checkout));
    await wait("funded");
    await a.reload(); await b.reload(); await balance(a,funded ? "10.00" : finalBalance); await balance(b,"0.00"); await mobileFits(a);
    const bKey = await checkOwnKeys({ a,b,ownerHttp,origin,acctA,acctB,checkpoint:issue,wait });
    if (funded) {
      process.stdout.write(bKey); issue("funding-keys");
      await checkPublishedCatalog({ a,b,owner,origin,api,ownerRead,issue,wait,exchange });
      const { current, corrected } = await checkUsageReport({ a,b,owner,origin,api,wait: file => expect.poll(
        () => fixtureExists(exchange,file), {timeout:155000},
      ).toBe(true),issue,exchange });
      const { checkOwnerOperations } = await import("./owner-operations-flow.mjs");
      await checkOwnerOperations({ owner,a,b,origin,api,issue,wait });
      await expect.poll(() => fixtureExists(exchange,"charge-complete"), {timeout:155000}).toBe(true);
      await balance(a,fundedBalance);
      const history = async (p,path) => (await (await p.context().request.get(api+path)).json()).data.items;
      const charged = (await history(a,"usage/requests?report="+current)).find(v=>v.charge_micro_usd==="1200000");
      expect(charged.basis).toBe("provider_final");
      expect(charged.total_tokens).toBe("50000");
      const entries=await history(a,"wallet/entries?id="+charged.wallet_receipt_id);
      expect(entries.length===1 && entries[0].amount_micro_usd==="1200000" && entries[0].request_id===charged.request_id).toBe(true);
      expect(await history(b,"usage/requests?id="+charged.request_id)).toHaveLength(0);
      expect(await history(b,"wallet/entries?id="+charged.wallet_receipt_id)).toHaveLength(0);
      await a.getByRole("link",{name:"Usage",exact:true}).click();
      await expect(a).toHaveURL(origin+"/app/usage");
      await expect(a.getByRole("heading",{name:"Usage",exact:true})).toBeVisible();
      for (const [hours,label] of [[168,"7 days"],[720,"30 days"],[24,"24 hours"]]) {
        const refreshed=a.waitForResponse(r=>r.url()===api+"usage/reports" && r.request().postDataJSON().hours===hours && r.status()===200);
        await a.getByRole("button",{name:label,exact:true}).click();
        await refreshed;
        await expect(a.getByRole("button",{name:label})).toHaveAttribute("aria-pressed","true");
      }
      const detail=a.locator("details").filter({has:a.locator("summary",{hasText:"1.200000 USD"})});
      await detail.locator("summary").click();
      await expect(detail.getByText("50,000",{exact:true})).toBeVisible();
      await expect(detail.getByText("2.000000 USD",{exact:true})).toBeVisible();
      await expect(detail.getByText("1.200000 USD · Verified usage",{exact:true})).toBeVisible();
      for (const width of [320,1024]) {
        await a.setViewportSize({width,height:760}); await mobileFits(a);
        const aligned=await a.locator(".history-requests").evaluate(list=>{
          const edge=list.getBoundingClientRect().right;
          return [...list.querySelectorAll(".receipt-amount")].every(el=>Math.abs(el.getBoundingClientRect().right-edge)<1);
        });
        expect(aligned).toBe(true);
        if (process.env.RELAY_TEST_HISTORY_UI_CAPTURE)
          await a.screenshot({path:join(process.env.RELAY_TEST_HISTORY_UI_CAPTURE,`usage-${width}.png`),fullPage:true});
      }
      await detail.getByRole("link",{name:"Wallet entry"}).click();
      await expect(a).toHaveURL(origin+"/app/billing?id="+charged.wallet_receipt_id);
      const entry=a.getByRole("listitem").filter({hasText:"Request charge"});
      await expect(entry.getByText("1.200000 USD",{exact:true})).toBeVisible();
      await entry.getByRole("link",{name:"Request identifier"}).click();
      await expect(a).toHaveURL(origin+"/app/usage?id="+charged.request_id);
      await expect(a.getByText("50,000",{exact:true})).toBeVisible();
      expect(corrected.charge_micro_usd).toBe("4000");
      expect(corrected.credit.amount_micro_usd).toBe("3400");
      expect(await history(b,"usage/requests?id="+corrected.request_id)).toHaveLength(0);
      expect(await history(b,"wallet/entries?id="+corrected.credit.wallet_receipt_id)).toHaveLength(0);
      await a.goto(origin+"/app/usage?id="+corrected.request_id);
      await expect(a.locator("summary").getByText("0.000600 USD",{exact:true})).toBeVisible();
      await expect(a.getByText("0.004000 USD · Partial usage",{exact:true})).toBeVisible();
      await a.getByRole("link",{name:"+0.003400 USD",exact:true}).click();
      await expect(a).toHaveURL(origin+"/app/billing?id="+corrected.credit.wallet_receipt_id);
      const correction=a.getByRole("listitem").filter({hasText:"Charge correction"});
      await expect(correction.getByText("+0.003400 USD",{exact:true})).toBeVisible();
      await correction.getByRole("link",{name:"Request identifier"}).click();
      await expect(a).toHaveURL(origin+"/app/usage?id="+corrected.request_id);
      await b.getByRole("link",{name:"Usage",exact:true}).click();
      await expect(b.getByText("No requests found")).toBeVisible();
      await a.getByRole("link",{name:"Overview",exact:true}).click();
    }
    await a.getByRole("button",{ name:"Sign out" }).click();
    await expect(a).toHaveURL(origin + "/login");
    expect((await probe.get(api + "account")).status()).toBe(401);
    await login(a); await balance(a,finalBalance);
    issue("restart"); await wait("restarted");
    expect((await probe.get(api + "account")).status()).toBe(401);
    expect((await ownerHttp.get(api + "admin/connections")).status()).toBe(401);
    await login(a); await balance(a,finalBalance);
    const restored = await account(a);
    expect(restored.workspace_id).toBe(acctA.workspace_id);
    if (payment) expect((await (await probe.get(api + "payments/" + payment.id)).json()).data.payment).toEqual(payment);
    expect((await (await probe.get(api + "keys")).json()).data.keys).toHaveLength(3);
    await login(b); await balance(b,"0.00");
    const list = async client => (await (await client.get(api + "notifications")).json()).data.items.map(n => [n.id, n.read]);
    if (notices) {
      expect(await list(probe)).toEqual([[notices.sharedId, true], [notices.privateId, true]]);
      expect(await list(b.context().request)).toEqual([[notices.sharedId, false]]);
    }
    issue("after-restart");
    if (notices) {
      await wait("notice-expired");
      expect(await list(probe)).toEqual([[notices.sharedId, true]]);
      expect((await probe.post(api + "notifications/read", {
        headers: { Origin: origin, "X-Relay-CSRF": restored.csrf_token }, data: { ids: [notices.privateId] },
      })).status()).toBe(404);
      issue("notice-expiry-seen");
    }
    await wait("suspended");
    expect((await probe.get(api + "account")).status()).toBe(401);
    expect((await probe.get(api + "notifications")).status()).toBe(401);
    expect((await probe.post(api + "keys",{headers:{Origin:origin,"X-Relay-CSRF":restored.csrf_token},
      data:{name:"Blocked",operation_id:crypto.randomUUID()}})).status()).toBe(401);
    expect((await b.context().request.get(api + "keys")).status()).toBe(200);
    issue("suspension-checked"); await wait("scope-expired");
    expect((await b.context().request.get(api + "keys")).status()).toBe(401);
    expect((await b.context().request.get(api + "account")).status()).toBe(401);
    expect((await b.context().request.get(api + "models")).status()).toBe(401);
    expect((await b.context().request.get(api + "models/status")).status()).toBe(401);
    expect((await b.context().request.get(api + "notifications")).status()).toBe(401);
    await signOwner();
    await owner.getByRole("button",{ name: "Pause customer access" }).click();
    await expect(owner.getByText("Customer access is paused.")).toBeVisible();
    expect((await probe.get(api + "account")).status()).toBe(401);
    expect((await probe.post(origin + "/v1/responses")).status()).toBe(404);
    await revokeDesktop(owner);
    issue("paused");
  } finally { await idp.close(); }
}
