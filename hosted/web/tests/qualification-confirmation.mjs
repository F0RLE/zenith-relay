import { expect } from "@playwright/test";
import { ownerAssertion, signInOwner } from "./owner-session.mjs";

export async function checkQualificationConfirmation({ browser, owner, customer, origin, issuer, candidates, beforeApproval, thorough }) {
  const api = origin + "/api/hosted/v1/";
  const path = api + "admin/operations/service/transitions";
  const options = path + "/options";
  const verify = state => `${path}/${state.ceremony_id}/verify`;
  const ownerHttp = owner.request;
  const read = async client => {
    const response = await client.get(api + "admin/overview");
    expect(response.status()).toBe(200);
    return (await response.json()).data;
  };
  let csrf = (await read(ownerHttp)).csrf_token;
  const post = (url, data, client = ownerHttp, token = csrf, site = origin) =>
    client.post(url, { headers: { Origin: site, "X-Relay-CSRF": token }, data });
  const request = { operation_id: crypto.randomUUID(), expected_revision: "1", target: "qualification",
    reason_code: "verify_customer_access", candidate_ids: candidates };
  const code = async (response, status, error) => {
    expect(response.status()).toBe(status);
    expect((await response.json()).error.code).toBe(error);
  };

  for (const url of [path, options]) {
    expect((await post(url, request, customer)).status()).toBe(401);
    expect((await post(url, request, ownerHttp, "0".repeat(64))).status()).toBe(403);
    expect((await post(url, request, ownerHttp, csrf, issuer)).status()).toBe(403);
    expect((await post(url, { ...request, candidate_ids: [crypto.randomUUID()] })).status()).toBe(400);
    expect((await post(url, { ...request, target: "live" })).status()).toBe(400);
  }
  // Even the session just used to approve a Desktop cannot approve customer access.
  await code(await post(path, request), 403, "fresh_passkey_required");
  if (thorough) {
    const begin = async (data, client, token) => {
      const response = await post(options, data, client, token);
      expect(response.status()).toBe(200);
      expect(response.headers()["cache-control"]).toBe("no-store");
      const state = (await response.json()).data;
      expect(state.options.publicKey.userVerification).toBe("required");
      return state;
    };
    const second = await browser.newContext();
    try {
      await second.addCookies(await owner.context().cookies(origin));
      await owner.context().clearCookies({ name: "__Host-relay_admin" });
      await owner.goto(origin + "/admin/login");
      await signInOwner(owner, origin);
      await expect(owner).toHaveURL(origin + "/admin/overview");
      csrf = (await read(ownerHttp)).csrf_token;
      const foreign = (await read(second.request)).csrf_token;
      const smaller = { ...request, operation_id: crypto.randomUUID(), candidate_ids: candidates.slice(0, 1) };
      const limited = await begin(smaller);
      const proof = await ownerAssertion(owner, limited.options);
      const different = await begin(request);
      await code(await post(verify(limited), proof, second.request, foreign), 409, "passkey_ceremony_expired");
      expect((await post(verify(limited), proof, ownerHttp, csrf, issuer)).status()).toBe(403);
      await code(await post(verify(different), proof), 400, "invalid_credential");
      await code(await post(verify(limited), {}), 400, "invalid_credential");
      await code(await post(verify(limited), proof), 409, "passkey_ceremony_expired");
      const obsolete = await begin(smaller, second.request, foreign);
      const obsoleteProof = await ownerAssertion(owner, obsolete.options);
      const cookie = await second.cookies(origin);
      expect((await post(api + "auth/admin/logout", undefined, second.request, foreign)).status()).toBe(200);
      await second.addCookies(cookie);
      expect((await post(verify(obsolete), obsoleteProof, second.request, foreign)).status()).toBe(401);
      expect((await read(ownerHttp)).service_mode).toBe("setup");
    } finally { await second.close(); }
  }
  await beforeApproval();
  await code(await post(path, request), 403, "fresh_passkey_required");
  const apply = owner.getByRole("button", { name: "Approve 24-hour access" });
  const command = owner.waitForRequest(r => r.url() === options);
  if (thorough) {
    await owner.route(`${path}/*/verify`, async route => {
      const response = await route.fetch();
      expect(response.status()).toBe(200);
      await response.dispose();
      await code(await post(route.request().url(), route.request().postDataJSON()), 409, "passkey_ceremony_expired");
      await route.abort("failed");
    }, { times: 1 });
  }
  await owner.getByLabel("Review IDs (one per tester)").fill(candidates.join(", "));
  await apply.click();
  const approved = (await command).postDataJSON();
  if (thorough) {
    await expect(owner.getByRole("alert")).toHaveText("Access change not confirmed. Try again.");
    const receipt = owner.waitForResponse(r => r.url() === options);
    await apply.click();
    expect((await (await receipt).json()).data).toEqual({ operation_id: approved.operation_id, applied: true });
  }
  await expect(owner.getByRole("button", { name: "Pause customer access" })).toBeVisible();
  expect((await read(ownerHttp)).csrf_token === csrf).toBe(true);
  expect((await post(path, { ...approved, candidate_ids: [...candidates].reverse() })).status()).toBe(200);
  expect((await post(path, { ...approved, candidate_ids: candidates.slice(0, 1) })).status()).toBe(409);
  expect((await post(path, request)).status()).toBe(409);
}
