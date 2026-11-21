import { expect } from "@playwright/test";
import { ownerAssertion } from "./owner-session.mjs";

export function supplyConfirmation(owner, origin, api, csrf) {
  const verification = api + "admin/supply/confirmations/";
  const path = challenge => `${verification}${challenge.ceremony_id}/verify`;
  const post = async (url, data, { client = owner.request, token, site = origin } = {}) =>
    client.post(url, { headers: { Origin: site, "X-Relay-CSRF": token ?? await csrf() }, data });
  const prepare = async (resource, command) => {
    const response = await post(resource + "/options", command);
    expect(response.status()).toBe(200);
    expect(response.headers()["cache-control"]).toBe("no-store");
    const challenge = (await response.json()).data;
    expect(challenge.options.publicKey.userVerification).toBe("required");
    return challenge;
  };
  const assertion = challenge => ownerAssertion(owner, challenge.options);
  return { verification, path, post, prepare, assertion };
}
