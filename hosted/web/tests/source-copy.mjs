import { expect } from "@playwright/test";
import { fixtureExists, readFixture, writeFixture } from "../../test-support/files.mjs";
import { ownerAssertion } from "./owner-session.mjs";

export async function checkSourceCopy(page, origin, exchange) {
  const wait = name => expect.poll(() => fixtureExists(exchange, name), { timeout: 25000 }).toBe(true);
  const notify = name => writeFixture(exchange, name);
  const api = "/api/hosted/v1";
  await expect.poll(() => page.evaluate(async path => {
    try { return (await fetch(path)).status; } catch { return 0; }
  }, api + "/public/config"), { timeout: 15000 }).toBe(200);
  await page.goto(origin + "/admin/connections");
  await page.getByRole("button", { name: "Sign in with a passkey" }).click();
  await page.getByRole("link", { name: "Connections", exact: true }).click();
  await page.getByRole("button", { name: "Desktop", exact: true }).click();
  await page.getByText("Desktop source copy", { exact: true }).click();
  notify("owner-ready");
  await wait("review");
  const code = readFixture(exchange, "review");
  const input = page.getByLabel("Review code from Desktop", { exact: true });
  await input.fill(code);
  const root = origin + api;
  const path = root + "/admin/source-copy", options = path + "/options", verify = root + "/admin/desktop/confirmations/";
  const overview = root + "/admin/overview", http = page.request;
  const read = async client => (await (await client.get(overview)).json()).data;
  const csrf = (await read(http)).csrf_token;
  const post = (url, data, client = http, token = csrf) =>
    client.post(url, { headers: { Origin: origin, "X-Relay-CSRF": token }, data });
  const preview = page.waitForResponse(r => r.url() === path && r.status() === 200);
  await page.getByRole("button", { name: "Review source" }).click();
  const reviewed = (await (await preview).json()).data;
  expect(reviewed.state).toBe("awaiting_approval");
  expect(reviewed.device_revision).toBe("2");
  await expect(page.getByText("Synthetic copied source", { exact: true })).toBeVisible();
  await expect(page.getByText("Device: Copy fixture")).toBeVisible();
  await expect(page.getByText(/Authentication included/)).toBeVisible();

  const command = { code, operation_id: reviewed.plan.operation_id, expected_device_revision: reviewed.device_revision };
  expect((await post(path, { code, approve: true })).status()).toBe(400);
  expect((await post(options, { ...command, expected_device_revision: "1" })).status()).toBe(409);
  expect((await post(options, { ...command, operation_id: crypto.randomUUID() })).status()).toBe(400);

  const secondary = await page.context().browser().newContext();
  const begin = async body => {
    const result = await post(options, body);
    expect(result.status()).toBe(200);
    const challenge = (await result.json()).data;
    expect(challenge.options.publicKey.userVerification).toBe("required");
    return challenge;
  };
  try {
    const signIn = root + "/auth/admin/sign-in";
    const login = await secondary.request.post(signIn + "/options", { headers: { Origin: origin } });
    expect(login.status()).toBe(200);
    const assertion = await ownerAssertion(page, (await login.json()).data);
    const authorized = await secondary.request.post(signIn + "/verify", { headers: { Origin: origin }, data: assertion });
    expect(authorized.status()).toBe(200);
    const otherCsrf = (await read(secondary.request)).csrf_token;
    expect(otherCsrf).not.toBe(csrf);
    const exact = await begin(command);
    const digest = JSON.parse(Buffer.from(code.slice("copy1.".length), "hex").toString());
    digest.plan.label = "Changed source";
    const changed = await begin({ ...command, code: "copy1." + Buffer.from(JSON.stringify(digest)).toString("hex") });
    const proof = await ownerAssertion(page, exact.options);
    const foreign = await post(`${verify}${exact.ceremony_id}/verify`, proof, secondary.request, otherCsrf);
    expect(foreign.status()).toBe(409);
    expect((await foreign.json()).error.code).toBe("passkey_ceremony_expired");
    const wrong = await post(`${verify}${changed.ceremony_id}/verify`, proof);
    expect(wrong.status()).toBe(400);
    expect((await wrong.json()).error.code).toBe("invalid_credential");
    const readOnly = await post(path, { code });
    expect((await readOnly.json()).data.state).toBe("awaiting_approval");
  } finally { await secondary.close(); }

  const approve = page.getByRole("button", { name: "Approve one copy" });
  await page.route(`${verify}*/verify`, async route => {
    const response = await route.fetch();
    expect(response.status()).toBe(200);
    expect((await response.json()).data).toEqual({ operation_id: command.operation_id, applied: true });
    await response.dispose();
    await route.abort("failed");
  }, { times: 1 });
  await approve.click();
  await expect(page.getByRole("alert")).toHaveText("The server did not confirm the result. Retry the same review.");
  const acknowledged = page.waitForResponse(r => r.url() === options);
  await approve.click();
  const receipt = await acknowledged;
  expect(receipt.request().postDataJSON()).toEqual(command);
  expect((await receipt.json()).data).toEqual({ operation_id: command.operation_id, applied: true });
  expect((await read(http)).csrf_token).toBe(csrf);
  await expect(page.getByRole("status").filter({ hasText: "Approved for 10 minutes" })).toBeVisible();
  notify("approved");
  await wait("committed");
  await page.getByRole("button", { name: "Check result" }).click();
  await expect(page.getByRole("status").filter({ hasText: "Saved outside the pool." })).toBeVisible();
  await page.getByRole("dialog", { name: "Desktop" }).getByRole("button", { name: "Close", exact: true }).click();
  const card = page.getByRole("article", { name: "Synthetic copied source", exact: true });
  await expect(card.getByText("Proxy assigned", { exact: true })).toBeVisible();
  await page.setViewportSize({ width: 320, height: 760 });
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  await expect(card).toBeVisible();
}
