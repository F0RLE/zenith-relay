import { expect } from "@playwright/test";
import { readFixture } from "../../test-support/files.mjs";
import { join } from "node:path";

export async function pairDesktop({owner, ownerRead, origin, exchange, wait, issue}) {
  const url=origin+"/api/hosted/v1/admin/devices", client=owner.context().request;
  await owner.getByText("Paired devices",{exact:true}).click();
  await owner.getByLabel("Device name").fill("Operator laptop");
  const posted=()=>owner.waitForRequest(r=>r.method()==="POST"&&r.url()===url);
  const first=posted();
  await owner.getByRole("button",{name:"Create invitation"}).click();
  const invitation=await owner.getByLabel("Paste this invitation into Relay Desktop").inputValue();
  const cmd=(await first).postDataJSON(), csrf=(await ownerRead()).csrf_token;
  const submit=(body,token,site=origin)=>client.post(url,{headers:{"X-Relay-CSRF":token,Origin:site},data:body});
  expect((await submit(cmd,csrf)).status()).toBe(200);
  expect((await submit({...cmd,label:"Different laptop"},csrf)).status()).toBe(409);
  expect((await submit(cmd,"0".repeat(64))).status()).toBe(403);
  expect((await submit(cmd,csrf,"https://different.example")).status()).toBe(403);
  expect((await client.get(origin+"/api/hosted/v1/device/connections")).status()).toBe(401);
  expect((await fetch(url)).status).toBe(401);
  issue("device-invited",invitation);
  await wait("device-review");
  const code=readFixture(exchange,"device-review");
  const input=owner.getByLabel("Approval code from your Desktop");
  await input.fill(code.replace(cmd.device_id,crypto.randomUUID()));
  const approve=owner.getByRole("button",{name:"Allow access for 24 hours"});
  await expect(approve).toBeDisabled();
  await input.fill(code);
  const changed=posted();
  const decision=status=>owner.waitForResponse(r=>r.url()===url&&r.request().method()==="POST"&&r.status()===status);
  const stale=decision(403), committed=decision(200);
  await approve.click();
  expect((await (await stale).json()).error.code).toBe("fresh_passkey_required");
  const applied=(await changed).postDataJSON();
  expect((await committed).request().postDataJSON()).toEqual(applied);
  await expect(owner.getByText("Active · View connections only")).toBeVisible();
  for (const width of [320,1024]) {
    await owner.setViewportSize({width,height:760});
    expect(await owner.evaluate(()=>document.documentElement.scrollWidth<=innerWidth)).toBe(true);
    if (process.env.RELAY_TEST_CATALOG_CAPTURE)
      await owner.screenshot({path:join(process.env.RELAY_TEST_CATALOG_CAPTURE,`paired-devices-${width}.png`),fullPage:true});
  }
  const renewed=(await ownerRead()).csrf_token;
  expect(renewed===csrf).toBe(false);
  expect((await submit(applied,renewed)).status()).toBe(200);
  expect((await submit({...applied,operation_id:crypto.randomUUID()},renewed)).status()).toBe(409);
  const response=await client.get(url);
  expect(response.headers()["cache-control"]).toBe("no-store");
  const data=(await response.json()).data;
  expect(data.items).toHaveLength(1);
  expect(Object.keys(data.items[0]).sort()).toEqual(["expires_at_s","id","label","last_used_at_s","revision","scope","state"]);
  expect(data.items[0].scope).toBe("connections.read");
  expect(data.items[0].revision).toBe("2");
  issue("device-approved"); await wait("device-checked");
}

export async function revokeDesktop(owner) {
  await owner.getByRole("link",{name:"Connections",exact:true}).click();
  await owner.getByText("Paired devices",{exact:true}).click();
  await expect(owner.getByText(/Last used:/)).toBeVisible();
  await owner.getByRole("button",{name:"Revoke access"}).click();
  await expect(owner.getByText("Active · View connections only")).toBeVisible();
  await owner.getByRole("button",{name:"Confirm revocation"}).click();
  await expect(owner.getByText("Revoked · View connections only")).toBeVisible();
}
