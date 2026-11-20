import { expect } from "@playwright/test";
import { join } from "node:path";
import { checkPurchaseLimit } from "./purchase-limit.mjs";
import { checkSellingReview } from "./selling-review.mjs";
import { checkModelStatus } from "./model-status-flow.mjs";
import { checkCapture } from "./diagnostic-capture-flow.mjs";

export async function checkPublishedCatalog({ a,b,owner,origin,api,ownerRead,issue,wait,exchange }) {
  const ah=a.context().request, oh=owner.context().request, endpoint=api+"admin/pools/model-rules";
  const read=async (client,url)=>(await (await client.get(url)).json()).data.items;
  const rules=()=>read(oh,endpoint), list=page=>read(page.context().request,api+"models");
  const send=(client,id,csrf,body)=>client.post(`${endpoint}/${id}`,{headers:{Origin:origin,"X-Relay-CSRF":csrf},data:body});
  const op=(rev,action,rates)=>({expected_revision:rev,operation_id:crypto.randomUUID(),action,...(rates&&{rates})});
  await wait("inventory-ready");
  const sources = api + "admin/connections";
  expect((await fetch(sources)).status).toBe(401);
  expect((await ah.get(sources)).status()).toBe(401);
  expect((await ah.get(origin+"/admin/connections",{maxRedirects:0})).headers().location).toBe("/admin/login");
  for (const q of ["?after=invalid","?workspace_id="+crypto.randomUUID(),"?after="+crypto.randomUUID()+"&after="+crypto.randomUUID()])
    expect((await oh.get(sources+q)).status()).toBe(400);
  await owner.getByRole("link",{name:"Connections",exact:true}).click();
  await expect(owner.getByRole("heading",{name:"Connections"})).toBeVisible();
  const saved = owner.getByRole("listitem").filter({hasText:"Test connection"});
  await expect(saved.getByText("Proxy assigned")).toBeVisible();
  await expect(saved.getByText("fixture-text",{exact:true})).toBeVisible();
  const response = await oh.get(sources);
  expect(response.headers()["cache-control"]).toBe("no-store");
  const inventory = (await response.json()).data, sourceId = inventory.items[0].id;
  expect(inventory).toEqual({items:[{id:sourceId,label:"Test connection",credential_state:"stored",
    proxy_bound:true,verified_models:["fixture-text"]}],next_after:null});
  for (const width of [320,1024]) {
    await owner.setViewportSize({width,height:760});
    expect(await owner.evaluate(()=>document.documentElement.scrollWidth<=innerWidth)).toBe(true);
    if (process.env.RELAY_TEST_CATALOG_CAPTURE)
      await owner.screenshot({path:join(process.env.RELAY_TEST_CATALOG_CAPTURE,`connections-${width}.png`),fullPage:true});
  }
  expect((await ah.get(endpoint)).status()).toBe(401);
  expect((await oh.get(api+"models")).status()).toBe(401);
  expect((await fetch(api+"models")).status).toBe(401);
  await a.getByRole("link",{name:"Models",exact:true}).click();
  await expect(a.getByText("No models available yet")).toBeVisible();
  await owner.getByRole("link",{name:"Pool",exact:true}).click();
  await expect(owner.getByText("Test connection")).toBeVisible();
  const [parked]=await rules();
  expect([parked.admitted,parked.enabled,parked.ready,parked.proxy,parked.effective,parked.purchase])
    .toEqual([false,false,true,true,null,null]);
  const csrf=async ()=>(await ownerRead()).csrf_token;
  expect((await send(ah,parked.id,await csrf(),op("0","admit"))).status()).toBe(401);
  expect((await send(oh,parked.id,"0".repeat(64),op("0","admit"))).status()).toBe(403);
  expect((await send(oh,parked.id,await csrf(),op("0","publish"))).status()).toBe(400);
  await owner.getByRole("button",{name:"Add to pool"}).click();
  await expect(owner.getByText("In pool",{exact:true})).toBeVisible();
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
  const sent=owner.waitForRequest(r=>r.method()==="POST" && r.url()===endpoint+"/"+parked.id && r.postDataJSON().action==="publish");
  await publish.click();
  await expect(owner.getByText("Published",{exact:true})).toBeVisible();
  const published=(await sent).postDataJSON();
  expect((await send(oh,parked.id,await csrf(),published)).status()).toBe(200);
  expect((await send(oh,parked.id,await csrf(),{...published,operation_id:crypto.randomUUID()})).status()).toBe(409);
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
  for (const theme of ["light","dark"]) {
    for (const [view,page] of [["pool",owner],["models",a]]) {
      await page.emulateMedia({colorScheme:theme,reducedMotion:"reduce"});
      await page.reload();
      await expect(page.locator(".hosted-entry")).toHaveAttribute("data-theme",theme);
      if (page===owner) {
        await owner.getByRole("button",{name:"Model rules"}).click();
        await expect(buy).toBeVisible();
      } else await expect(a.getByText(row.model,{exact:true})).toBeVisible();
      for (const width of [320,1024]) {
        await page.setViewportSize({width,height:760});
        expect(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth &&
          [...document.querySelectorAll(".entry-catalog :is(button,input,legend),.catalog-prices dd")].every(el=>{
            const box=el.getBoundingClientRect();return box.left>=0&&box.right<=innerWidth;
          }))).toBe(true);
        if (process.env.RELAY_TEST_CATALOG_CAPTURE)
          await page.screenshot({path:join(process.env.RELAY_TEST_CATALOG_CAPTURE,`${view}-${width}-${theme}.png`),fullPage:true});
      }
    }
  }
  await expect(a.getByRole("link", { name: "Status", exact: true })).toBeVisible();
  const status = await a.context().newPage();
  await checkModelStatus({ page: status, foreign: b, owner, origin, api, model: row.model });
  issue("rule-published"); await wait("first-hold");
  // Lose the acknowledgement after the price revision commits. Retry the same edit and operation.
  const writes=[];
  const mutation=endpoint+"/"+parked.id;
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
  issue("lower-price");
  await checkCapture({ owner, a, b, origin, api, ownerRead, issue, wait, exchange });
  await a.getByRole("link",{name:"Overview",exact:true}).click();
  await checkPurchaseLimit({ owner, customer: a, origin, api, sourceId, ownerRead, issue, wait, exchange });
  await owner.getByRole("link",{name:"Pool",exact:true}).click();
  await owner.getByRole("button",{name:"Model rules"}).click();
  await status.getByRole("button", { name: "Refresh model status" }).click();
  const requestTotal = status.locator(".model-status-summary dd").nth(1).locator(":scope > span").first();
  await expect(requestTotal).toHaveText(/^[1-9][0-9]*$/);
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
  await status.close();
  issue("republished");
  await checkSellingReview({ owner, rules, list: () => list(a), send, csrf, endpoint, issue, wait });
  await wait("connections-changed");
  await owner.getByRole("link",{name:"Connections",exact:true}).click();
  const first=(await (await oh.get(sources)).json()).data;
  expect(first.items).toHaveLength(32);
  expect(first.next_after).toBe(first.items.at(-1).id);
  expect(first.items.filter(r=>r.label!==null).map(r=>r.label)).toEqual(["Second connection"]);
  expect(first.items.every(r=>!r.proxy_bound && r.verified_models.length===0)).toBe(true);
  const revoked = owner.locator(".source-row").filter({hasText:"Provider key revoked"});
  await revoked.locator(".source-purchase summary").click();
  await expect(revoked.getByText("No Relay limit")).toBeVisible();
  await expect(revoked.getByRole("radio")).toHaveCount(0);
  await owner.getByRole("button",{name:"Next page"}).click();
  await expect(saved.getByText("No proxy assigned")).toBeVisible();
  await expect(saved.getByText("No current model checks")).toBeVisible();
  const last=(await (await oh.get(sources+"?after="+first.next_after)).json()).data;
  expect(last).toEqual({items:[{...inventory.items[0],proxy_bound:false,verified_models:[]}],next_after:null});
  await expect(owner.getByRole("button",{name:"Next page"})).toHaveCount(0);
  await owner.getByRole("button",{name:"Previous page"}).click();
  await expect(owner.getByText("Provider key revoked")).toBeVisible();
  issue("connections-checked");
}
