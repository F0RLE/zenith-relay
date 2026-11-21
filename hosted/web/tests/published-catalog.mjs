import { expect } from "@playwright/test";
import { join } from "node:path";
import { checkPurchaseLimit } from "./purchase-limit.mjs";
import { checkSellingReview } from "./selling-review.mjs";
import { checkModelStatus } from "./model-status-flow.mjs";
import { checkCapture } from "./diagnostic-capture-flow.mjs";
import { signInOwner } from "./owner-session.mjs";
import { supplyConfirmation } from "./supply-confirmation.mjs";
import { checkSourceLifecycle, checkSourceRemoval, checkSourceServing } from "./source-serving-flow.mjs";
import { navigateCustomer } from "./customer-shell.mjs";
import { addPoolModel, checkPoolActivity } from "./pool-members.mjs";

export async function checkPublishedCatalog({ a,b,owner,origin,api,ownerRead,issue,wait,exchange,reopenSessions }) {
  const ah=a.context().request, oh=owner.context().request, endpoint=api+"admin/pools/model-rules";
  const read=async (client,url)=>(await (await client.get(url)).json()).data.items;
  const rules=()=>read(oh,endpoint), list=page=>read(page.context().request,api+"models");
  const send=(client,id,csrf,body)=>client.post(`${endpoint}/${id}`,{headers:{Origin:origin,"X-Relay-CSRF":csrf},data:body});
  const op=(rev,action,rates)=>({expected_revision:rev,operation_id:crypto.randomUUID(),action,...(rates&&{rates})});
  const csrf=async ()=>(await ownerRead()).csrf_token;
  const proof=supplyConfirmation(owner,origin,api,csrf);
  await wait("inventory-ready");
  const sources = api + "admin/connections";
  expect((await fetch(sources)).status).toBe(401);
  expect((await ah.get(sources)).status()).toBe(401);
  expect((await ah.get(origin+"/admin/connections",{maxRedirects:0})).headers().location).toBe("/admin/login");
  expect((await ah.get(sources+"?view=trash")).status()).toBe(401);
  for (const q of ["?after=invalid","?view=bogus","?view=trash&view=api","?workspace_id="+crypto.randomUUID(),"?after="+crypto.randomUUID()+"&after="+crypto.randomUUID()])
    expect((await oh.get(sources+q)).status()).toBe(400);
  await owner.getByRole("link",{name:"Connections",exact:true}).click();
  await expect(owner.getByRole("heading",{name:"Connections"})).toBeVisible();
  const saved = owner.getByRole("article", { name: "Test connection", exact: true });
  await expect(saved.getByText("Proxy assigned")).toBeVisible();
  const verified = saved.getByRole("button", { name: /Verified models/ });
  await verified.click();
  const checks = owner.getByRole("dialog", { name: "Verified models" });
  await expect(checks.getByText("fixture-text", { exact: true })).toBeVisible();
  await owner.keyboard.press("Escape");
  await expect(verified).toBeFocused();
  const response = await oh.get(sources);
  expect(response.headers()["cache-control"]).toBe("no-store");
  const inventory = (await response.json()).data, sourceId = inventory.items[0].id;
  expect(inventory).toEqual({items:[{id:sourceId,label:"Test connection",credential_state:"stored",
    serving:{revision:"0",state:"enabled",in_pool:false,draining:false},
    proxy_bound:true,verified_models:["fixture-text"]}],next_after:null});
  for (const width of [320,1024]) {
    await owner.setViewportSize({width,height:760});
    expect(await owner.evaluate(()=>document.documentElement.scrollWidth<=innerWidth)).toBe(true);
    if (process.env.RELAY_TEST_CATALOG_CAPTURE) {
      await owner.getByRole("heading", { name: "Connections", exact: true }).click();
      await owner.screenshot({path:join(process.env.RELAY_TEST_CATALOG_CAPTURE,`connections-${width}.png`),fullPage:true});
    }
  }
  await checkSourceServing({ owner, customer: a, saved, origin, api, sourceId, ownerRead });
  expect((await ah.get(endpoint)).status()).toBe(401);
  expect((await oh.get(api+"models")).status()).toBe(401);
  expect((await fetch(api+"models")).status).toBe(401);
  await a.route(api+"models", route => route.fulfill({ status: 503 }), { times: 1 });
  await navigateCustomer(a, "Models");
  const catalog = a.locator(".catalog-table");
  await expect(catalog.getByRole("alert")).toHaveText("Cannot connect to a compatible Relay server.");
  await expect(catalog.locator(".catalog-row")).toHaveCount(0);
  await catalog.getByRole("button", { name: "Retry", exact: true }).click();
  await expect(catalog.getByText("No published models yet", { exact: true })).toBeVisible();
  await owner.getByRole("link",{name:"Pool",exact:true}).click();
  await expect(owner.getByText("No members in the pool", { exact: true })).toBeVisible();
  const [parked]=await rules();
  expect([parked.admitted,parked.enabled,parked.ready,parked.proxy,parked.effective,parked.purchase])
    .toEqual([false,false,true,true,null,null]);
  const mutation=endpoint+"/"+parked.id, options=mutation+"/options", admission=op("0","admit");
  const reject=async (pending,status,code)=>{
    const r=await pending; expect(r.status()).toBe(status);
    if(code) expect((await r.json()).error.code).toBe(code);
  };
  for (const path of [mutation,options]) {
    await reject(proof.post(path,admission,{client:ah}),401);
    await reject(proof.post(path,admission,{token:"0".repeat(64)}),403);
    await reject(proof.post(path,admission,{site:"https://foreign.invalid"}),403);
  }
  const foreign=await owner.context().browser().newContext();
  try {
    const oldToken=await csrf();
    await foreign.addCookies(await owner.context().cookies(origin));
    await owner.context().clearCookies({name:"__Host-relay_admin"});
    await owner.goto(origin+"/admin/login");
    await signInOwner(owner,origin);
    await expect(owner).toHaveURL(origin+"/admin/overview");
    expect(await csrf()).not.toBe(oldToken);
    // Even a fresh login cannot change admission or publish a rule.
    await reject(proof.post(mutation,admission),403,"fresh_passkey_required");
    await reject(proof.post(mutation,op("0","publish")),400,"invalid_model_rule");
    const selected=await proof.prepare(mutation,admission);
    const other=await proof.prepare(mutation,op("0","admit"));
    const signed=await proof.assertion(selected);
    await reject(proof.post(proof.path(selected),signed,{client:foreign.request,token:oldToken}),409,"passkey_ceremony_expired");
    await reject(proof.post(proof.path(selected),signed,{site:"https://foreign.invalid"}),403);
    await reject(proof.post(proof.path(other),signed),400,"invalid_credential");
    await reject(proof.post(proof.path(selected),admission),400,"invalid_credential");
    await reject(proof.post(proof.path(selected),signed),409,"passkey_ceremony_expired");
    expect((await rules())[0]).toEqual(parked);
  } finally { await foreign.close(); }
  await owner.getByRole("link",{name:"Pool",exact:true}).click();
  const sessionToken=await csrf();
  await addPoolModel(owner, "Test connection", "fixture-text");
  expect(await csrf()).toBe(sessionToken);
  await owner.getByRole("button",{name:"Model rules"}).click();
  const publish=owner.getByRole("button",{name:"Publish to testers"});
  const save=owner.getByRole("button",{name:"Save prices"});
  const buy=owner.getByRole("group",{name:"Purchase prices"}), sell=owner.getByRole("group",{name:"Selling prices"});
  const price=(group,name)=>group.getByLabel(name,{exact:true});
  await expect(publish).toBeDisabled();
  await price(sell,"Input").fill("20");
  await price(sell,"Cache read").fill("20");
  await expect(save).toBeDisabled();
  await price(sell,"Output").fill("39");
  await save.click();
  await expect(owner.getByText("Set purchase prices to publish.")).toBeVisible();
  await expect(publish).toBeDisabled();
  const [draft]=await rules();
  expect(draft.purchase).toBeNull();
  const noPurchase=await send(oh,parked.id,await csrf(),op(draft.revision,"publish"));
  expect(noPurchase.status()).toBe(409);
  expect((await noPurchase.json()).error.code).toBe("purchase_prices_required");
  await price(buy,"Input").fill("5");
  await price(buy,"Cache read").fill("0");
  await expect(save).toBeDisabled();
  await price(buy,"Output").fill("11");
  await price(sell,"Output").fill("40");
  await save.click(); await expect(publish).toBeEnabled();
  expect((await rules())[0].purchase)
    .toEqual({input:"5.000000000000",cached:"0.000000000000",output:"11.000000000000"});
  expect(await list(a)).toEqual([]);
  const ready=(await rules())[0];
  await reject(proof.post(mutation,op(ready.revision,"publish")),403,"fresh_passkey_required");
  await reject(proof.post(options,op(ready.revision,"save",ready.draft)),400,"invalid_model_rule");
  const publications=[];
  const track=async route=>{ publications.push(route.request().postDataJSON()); await route.continue(); };
  await owner.route(options,track);
  await owner.route(proof.verification+"*/verify",async route=>{
    const signed=route.request().postDataJSON();
    expect(signed).toMatchObject({type:"public-key"});
    expect(signed).not.toHaveProperty("action");
    const committed=await route.fetch();
    expect(committed.status()).toBe(200);
    expect((await committed.json()).data).toEqual({applied:true,operation_id:publications[0].operation_id});
    await committed.dispose();
    await reject(proof.post(route.request().url(),signed),409,"passkey_ceremony_expired");
    await route.abort("failed");
  },{times:1});
  try {
    await publish.click();
    await expect(owner.getByRole("alert")).toHaveText(/Could not confirm the change/);
    // A re-read can hide Publish before the acknowledgement is recovered.
    await owner.evaluate(()=>window.dispatchEvent(new Event("focus")));
    await expect(owner.getByText("Published",{exact:true})).toBeVisible();
    const receipt=owner.waitForResponse(r=>r.url()===options && r.ok());
    await owner.getByRole("button",{name:"Retry",exact:true}).click();
    expect((await (await receipt).json()).data).toEqual({applied:true,operation_id:publications[0].operation_id});
    await expect(owner.getByRole("alert")).toHaveCount(0);
  } finally { await owner.unroute(options,track); }
  expect(publications).toHaveLength(2);
  const [published,replayed]=publications;
  expect(replayed).toEqual(published);
  expect((await send(oh,parked.id,await csrf(),published)).status()).toBe(200);
  expect((await send(oh,parked.id,await csrf(),{...published,operation_id:crypto.randomUUID()})).status()).toBe(409);
  await reject(proof.post(options,{...published,action:"suspend"}),409,"rule_changed");
  expect(await csrf()).toBe(sessionToken);
  const [row]=await list(a);
  expect(row.model).toBe("fixture-text"); expect(row.protocol).toBe("responses");
  expect(row.revision).toBe("1"); expect(row.rates.input).toBe("20.000000000000");
  expect(await list(b)).toEqual([row]);
  expect(row).not.toHaveProperty("source"); expect(row).not.toHaveProperty("draft");
  expect(row).not.toHaveProperty("purchase"); expect(row).not.toHaveProperty("purchase_rates");
  const refreshed=a.waitForResponse(r=>r.url()===api+"models" && r.status()===200);
  await a.bringToFront();
  // Headless contexts do not consistently send OS window activation events.
  await a.evaluate(()=>window.dispatchEvent(new Event("focus")));
  await refreshed; await expect(a.getByText(row.model,{exact:true})).toBeVisible();
  const publicRow = catalog.getByRole("row").filter({ has: a.getByRole("rowheader", { name: row.model, exact: true }) });
  await expect(publicRow.locator(".catalog-protocols")).toHaveText("Responses");
  await expect(publicRow.locator(".catalog-price-main > strong")).toHaveText(["$20", "$40", "$20", "—"]);
  await expect(publicRow.locator(".catalog-context-value")).toHaveText("—");
  await expect(catalog.locator(".catalog-company,.catalog-official,.catalog-saving")).toHaveCount(0);
  const copyId = publicRow.getByRole("button", { name: `Copy ${row.model} model ID`, exact: true });
  await a.context().grantPermissions(["clipboard-read", "clipboard-write"]);
  await copyId.click();
  await expect.poll(() => a.evaluate(() => navigator.clipboard.readText())).toBe(row.model);
  for (const theme of ["light","dark"]) {
    for (const [view,page] of [["pool",owner],["models",a]]) {
      await page.emulateMedia({colorScheme:theme,reducedMotion:"reduce"});
      await page.reload();
      await expect(page.locator(".hosted-app")).toHaveAttribute("data-theme",theme);
      if (page===owner) {
        await owner.getByRole("button",{name:"Model rules"}).click();
        await expect(buy).toBeVisible();
      } else await expect(a.getByText(row.model,{exact:true})).toBeVisible();
      for (const width of page===a ? [320,1024,1440] : [320,1024]) {
        await page.setViewportSize({width,height:760});
        expect(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth &&
          [...document.querySelectorAll(".entry-catalog :is(button,input,legend),.catalog-prices dd,.catalog-table button,.hosted-tools > :is(a, button)")].every(el=>{
            if (el.getClientRects().length === 0) return true;
            const box=el.getBoundingClientRect();return box.left>=0&&box.right<=innerWidth;
          }))).toBe(true);
        if (page===a && width===1440) expect(await publicRow.evaluate(el => {
          const middle = node => { const box = node.getBoundingClientRect(); return box.top + box.height / 2; };
          return [...el.querySelectorAll(".catalog-price-main > strong, .catalog-protocols, .catalog-model-name")]
            .every(node => Math.abs(middle(node) - middle(el)) <= 1);
        })).toBe(true);
        if (process.env.RELAY_TEST_CATALOG_CAPTURE)
          await page.screenshot({path:join(process.env.RELAY_TEST_CATALOG_CAPTURE,`${view}-${width}-${theme}.png`),fullPage:true});
      }
    }
  }
  await expect(a.getByRole("link", { name: "Status", exact: true })).toBeVisible();
  const status = await a.context().newPage();
  await checkModelStatus({ page: status, foreign: b, owner, origin, api, model: row.model });
  issue("rule-published"); await wait("first-hold");
  await checkPoolActivity({ owner, customer: a, api, sourceId, model: row.model });
  // Lose the acknowledgement after the price revision commits. Retry the same edit and operation.
  const writes=[];
  const interrupt=async route=>{
    const body=route.request().postDataJSON();
    writes.push(body);
    if (writes.length===1) {
      expect((await route.fetch()).status()).toBe(200);
      await route.abort();
    } else await route.continue();
  };
  await owner.route(mutation,interrupt);
  try {
    await price(buy,"Input").fill("8");
    await price(buy,"Cache read").fill("2");
    await price(buy,"Output").fill("10");
    await save.click();
    await expect(owner.getByRole("alert")).toHaveText(/Could not confirm the change/);
    await save.click(); await expect(save).toBeHidden();
  } finally { await owner.unroute(mutation,interrupt); }
  expect(writes).toHaveLength(2);
  expect(writes[1]).toEqual(writes[0]);
  expect(writes[0]).not.toHaveProperty("rates");
  expect(await list(a)).toEqual([row]);
  await price(sell,"Input").fill("21");
  await save.click(); await expect(publish).toBeEnabled(); await publish.click();
  await expect(owner.getByRole("alert")).toHaveText(/increase requires advance notice/);
  expect(await list(a)).toEqual([row]);
  await price(sell,"Input").fill("19.000000000001");
  await save.click(); await expect(save).toBeHidden(); await publish.click();
  await expect.poll(async ()=>(await list(a))[0].rates.input).toBe("19.000000000001");
  await a.evaluate(() => window.dispatchEvent(new Event("focus")));
  await expect(publicRow.locator('[data-component="input"] strong')).toHaveText("$19.000000000001");
  issue("lower-price");
  await checkCapture({ owner, a, b, origin, api, ownerRead, issue, wait, exchange });
  await navigateCustomer(a, "Overview");
  await checkPurchaseLimit({ owner, customer: a, origin, api, sourceId, ownerRead, issue, wait, exchange });
  await owner.getByRole("link",{name:"Pool",exact:true}).click();
  await owner.getByRole("button",{name:"Model rules"}).click();
  await status.getByRole("button", { name: "Refresh model status" }).click();
  const requestTotal = status.locator(".model-status-summary dd").nth(1).locator(":scope > span").first();
  await expect(requestTotal).toHaveText(/^[1-9][0-9]*$/);
  const latency = status.locator('.model-status-row [data-metric="latency"]');
  await expect(latency.locator("span[title]")).toHaveText(/^\d[\d,]*\s*ms$/);
  await expect(latency.locator("span[title]")).toHaveAttribute("title", /^Average time to completion · [1-9]/);
  await expect(status.locator('.model-status-row[data-state="unknown"]')).toHaveCount(1);
  if (process.env.RELAY_TEST_CATALOG_CAPTURE)
    await status.screenshot({ path: join(process.env.RELAY_TEST_CATALOG_CAPTURE, "live-status-timing.png"), fullPage: true });
  await owner.getByRole("button",{name:"Withdraw model"}).click();
  expect(await list(a)).toHaveLength(1);
  await owner.getByRole("button",{name:"Confirm withdrawal"}).click();
  await expect.poll(()=>list(a)).toEqual([]);
  await status.bringToFront();
  await status.evaluate(() => window.dispatchEvent(new Event("focus")));
  await expect(status.getByText("No models available yet")).toBeVisible();
  await expect(status.getByRole("group", { name: "Availability history: " + row.model })).toHaveCount(0);
  issue("withdrawn"); await wait("withdrawal-seen");
  await publish.click(); await expect.poll(async ()=>(await list(a))[0]?.revision).toBe("3");
  await status.getByRole("button", { name: "Refresh model status" }).click();
  const restored = status.getByRole("group", { name: "Availability history: " + row.model });
  await expect(restored).toBeVisible();
  await expect(restored.locator(':scope > span:not([data-state="unknown"])')).toHaveCount(0);
  await expect(latency.locator("span[aria-hidden=true]").last()).toHaveText("—");
  await status.close();
  issue("republished");
  await wait("selling-review-ready");
  await reopenSessions();
  await owner.getByRole("link", { name: "Pool", exact: true }).click();
  await checkSellingReview({ owner, customer: a, rules, list: () => list(a), send, csrf, endpoint, proof, issue, wait });
  await wait("source-lifecycle-ready");
  await reopenSessions();
  await checkSourceLifecycle({ owner, saved, origin, api, sourceId, ownerRead, models: () => list(a) });
  issue("source-lifecycle-checked");
  await wait("connections-changed");
  await owner.getByRole("link",{name:"Connections",exact:true}).click();
  const first=(await (await oh.get(sources)).json()).data;
  expect(first.items).toHaveLength(32);
  expect(first.next_after).toBe(first.items.at(-1).id);
  expect(first.items.filter(r=>r.label!==null).map(r=>r.label)).toEqual(["Second connection"]);
  expect(first.items.every(r=>!r.proxy_bound && r.verified_models.length===0)).toBe(true);
  const revoked = owner.getByRole("article").filter({hasText:"Provider key revoked"});
  await expect(revoked.getByRole("button", { name: "Pause", exact: true })).toHaveCount(0);
  await revoked.first().getByRole("button", { name: "Purchase limit", exact: true }).click();
  const limit = owner.getByRole("dialog", { name: "Purchase limit" });
  await expect(limit.getByText("No Relay limit")).toBeVisible();
  await expect(limit.getByRole("radio")).toHaveCount(0);
  await limit.getByRole("button", { name: "Close", exact: true }).click();
  await owner.getByRole("button",{name:"Next page"}).click();
  await expect(saved.getByText("No proxy assigned")).toBeVisible();
  await expect(saved.getByText("No current model checks")).toBeVisible();
  const last=(await (await oh.get(sources+"?after="+first.next_after)).json()).data;
  const late = { id: "00000000-0000-0000-0000-000000000021", label: null, proxy_bound: false,
    credential_state: "stored", verified_models: [], serving: { revision: "0", state: "enabled", in_pool: false, draining: false } };
  expect(last).toEqual({items:[late, {...inventory.items[0],proxy_bound:false,verified_models:[],
    serving:{revision:"6",state:"enabled",in_pool:false,draining:false}}],next_after:null});
  await expect(saved.getByText("Provider key revoked")).toHaveCount(0);
  await expect(owner.getByRole("button",{name:"Next page"})).toHaveCount(0);
  await owner.getByRole("searchbox", { name: "Find on this page" }).fill("no-match-for-this-page");
  await expect(owner.getByText("No matching connections on this page")).toBeVisible();
  await owner.getByRole("searchbox", { name: "Find on this page" }).fill("");
  // Trash must see a source beyond the unfiltered first page.
  const lateEndpoint = `${sources}/${late.id}/serving`;
  const commit = async (revision, action) => {
    const challenge = await proof.prepare(lateEndpoint, op(revision, action));
    expect((await proof.post(proof.path(challenge), await proof.assertion(challenge))).status()).toBe(200);
  };
  await commit("0", "trash");
  await owner.getByRole("button", { name: "Trash", exact: true }).click();
  await expect(owner.getByRole("article", { name: late.id, exact: true }).getByRole("status")).toHaveText("In trash");
  expect((await (await oh.get(sources + "?view=trash")).json()).data).toEqual({
    items: [{ ...late, serving: { revision: "1", state: "trashed", in_pool: false, draining: false } }],
    next_after: null,
  });
  await commit("1", "restore");
  await commit("2", "resume");
  await owner.getByRole("button", { name: "API providers", exact: true }).click();
  await expect(owner.getByRole("button",{name:"Previous page"})).toHaveCount(0);
  await owner.getByRole("button",{name:"Next page"}).click();
  await expect(saved.getByRole("button", { name: "Pause", exact: true })).toBeEnabled();
  await owner.getByRole("button",{name:"Previous page"}).click();
  await owner.getByRole("searchbox", { name: "Find on this page" }).fill("");
  await expect(revoked.first()).toBeVisible();
  const second = first.items.find(source => source.label === "Second connection");
  await checkSourceRemoval({ owner, saved: owner.getByRole("article", { name: "Second connection", exact: true }),
    origin, api, sourceId: second.id, ownerRead });
  issue("connections-checked");
}
