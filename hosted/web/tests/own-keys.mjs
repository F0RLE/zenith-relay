import { expect } from "@playwright/test";
import { navigateCustomer } from "./customer-shell.mjs";

export async function checkOwnKeys({ a, b, ownerHttp, origin, acctA, acctB, checkpoint, wait }) {
  const url = origin + "/api/hosted/v1/keys";
  const ah = a.context().request, bh = b.context().request;
  const headers = csrf => ({ Origin:origin, "X-Relay-CSRF":csrf });
  const post = (client, csrf, data) => client.post(url,{headers:headers(csrf),data});
  const list = async (client, query = "") => (await (await client.get(url + query)).json()).data.keys;
  const reveal = a.getByRole("region",{name:"Key created"});
  const row = name => a.getByRole("listitem").filter({has:a.getByText(name,{exact:true})});
  await navigateCustomer(a, "API keys");
  await expect(a.getByText("No keys yet. Create the first one for your app.")).toBeVisible();
  await a.getByLabel("Key name").fill("Test client");
  await a.getByRole("button",{name:"Create key",exact:true}).click();
  await expect(reveal).toBeVisible();
  const first = await reveal.locator("code").innerText();
  const [record] = await list(ah);
  expect(Object.keys(record).sort()).toEqual(["expires_at_s","id","name","operation_id","prefix","replaces_key_id","status"]);
  expect(first.startsWith(record.prefix) && first.length === 101).toBe(true);
  expect(await a.evaluate(value => ![sessionStorage,localStorage].some(store =>
    Object.values(store).some(entry => String(entry).includes(value))),first)).toBe(true);
  await reveal.getByRole("button",{name:"Done"}).click();
  await a.reload(); await expect(row("Test client")).toBeVisible(); await expect(reveal).toHaveCount(0);
  const request = {operation_id:record.operation_id,name:record.name};
  expect((await ownerHttp.get(url)).status()).toBe(401);
  expect((await post(ownerHttp,acctA.csrf_token,request)).status()).toBe(401);
  expect((await post(ah,acctB.csrf_token,request)).status()).toBe(403);
  expect((await ah.post(url,{headers:{...headers(acctA.csrf_token),Origin:"https://elsewhere.invalid"},data:request})).status()).toBe(403);
  expect((await post(ah,acctA.csrf_token,{...request,workspace_id:acctB.workspace_id})).status()).toBe(400);
  expect((await post(ah,acctA.csrf_token,{...request,operation_id:crypto.randomUUID(),expires_at_s:1})).status()).toBe(400);
  expect((await post(ah,acctA.csrf_token,{...request,name:"Changed intent"})).status()).toBe(409);
  const replay = await (await post(ah,acctA.csrf_token,request)).json();
  expect("secret" in replay.data).toBe(false);
  expect(replay.data.key.id).toBe(record.id);
  expect((await list(ah,"?operation_id=" + record.operation_id))[0].id).toBe(record.id);
  expect(await list(bh,"?operation_id=" + record.operation_id)).toHaveLength(0);
  expect((await bh.delete(url + "/" + record.id,{headers:headers(acctB.csrf_token)})).status()).toBe(404);
  expect((await post(bh,acctB.csrf_token,{...request,replaces_key_id:record.id})).status()).toBe(404);
  const unauth = await a.context().browser().newContext();
  try {
    expect((await unauth.request.get(url,{headers:{Authorization:"Bearer " + first}})).status()).toBe(401);
  } finally { await unauth.close(); }
  await row("Test client").getByRole("button",{name:"Reissue",exact:true}).click();
  await expect(row("Test client").getByRole("button",{name:"Yes, reissue"})).toBeVisible();
  expect((await list(ah)).length).toBe(1);
  await row("Test client").getByRole("button",{name:"Yes, reissue"}).click();
  await expect(reveal).toBeVisible();
  const second = await reveal.locator("code").innerText();
  expect(first !== second && second.length === 101).toBe(true);
  await reveal.getByRole("button",{name:"Done"}).click();
  const keys = await list(ah);
  expect(keys.map(k => k.status)).toEqual(["active","active"]);
  expect(keys[0].replaces_key_id).toBe(record.id);

  // Only the Rust test reads this anonymous pipe. Never put bearer material in
  // checkpoint files, assertions that echo values, screenshots, URLs or logs.
  process.stdout.write(first + second);
  checkpoint("keys-issued"); await wait("keys-verified");
  const original = row("Test client").filter({hasText:record.prefix});
  await original.getByRole("button",{name:"Revoke",exact:true}).click();
  await original.getByRole("button",{name:"Yes, revoke"}).click();
  await expect(original.getByText("Revoked",{exact:true})).toBeVisible();
  expect((await ah.delete(url+"/"+record.id,{headers:headers(acctA.csrf_token)})).status()).toBe(200);
  expect((await post(ah,acctA.csrf_token,{...request,operation_id:crypto.randomUUID(),replaces_key_id:record.id})).status()).toBe(409);

  // Concurrent requests in two DB transactions: one secret, one row.
  const duel = {operation_id:crypto.randomUUID(),name:"Concurrent issue"};
  const raced = await Promise.all([post(bh,acctB.csrf_token,duel),post(bh,acctB.csrf_token,duel)]);
  expect(raced.every(r => r.status() === 200)).toBe(true);
  const replies = await Promise.all(raced.map(async r => (await r.json()).data));
  expect(replies.filter(v => typeof v.secret === "string").length).toBe(1);
  expect(replies[0].key.id).toBe(replies[1].key.id);
  expect(await list(bh)).toHaveLength(1);

  // The database commits the POST but the browser never receives its response.
  let sends = 0;
  await a.route(url,async route => {
    if (route.request().method() !== "POST") return route.continue();
    sends++;
    const committed = await route.fetch();
    expect(committed.status()).toBe(200); await committed.dispose(); await route.abort("failed");
  });
  await a.getByLabel("Key name").fill("Lost reply");
  await a.getByRole("button",{name:"Create key",exact:true}).click();
  await expect(a.getByRole("status").filter({hasText:"Your key was created"})).toBeVisible();
  await expect(row("Lost reply")).toHaveCount(1); await expect(reveal).toHaveCount(0);
  await a.reload(); await expect(row("Lost reply")).toBeVisible();
  expect(sends).toBe(1); await a.unroute(url);
  expect((await list(ah)).length).toBe(3);
  const widths = [320,1024];
  for (const width of widths) {
    await a.setViewportSize({width,height:760});
    expect(await a.evaluate(() => document.documentElement.scrollWidth <= innerWidth
      && [...document.querySelectorAll(".own-keys button,.own-keys input")].every(el => {
        const box = el.getBoundingClientRect(); return box.left >= 0 && box.right <= innerWidth;
      }))).toBe(true);
    const dir = process.env.RELAY_TEST_KEY_UI_CAPTURE;
    if (dir) await a.screenshot({path:`${dir}/keys-${width}.png`,fullPage:true});
  }
  await navigateCustomer(a, "Overview");
  return replies.find(v => typeof v.secret === "string").secret;
}
