import { expect } from "@playwright/test";
import { fixtureExists, readFixture, writeFixture } from "../../test-support/files.mjs";
import { signInOwner } from "./owner-session.mjs";

export async function checkOwnerRecovery({ origin, old, enroll, prove, waitForServer, exchange }) {
  const wait = name => expect.poll(() => fixtureExists(exchange, name), { timeout: 20000 }).toBe(true);
  const overview = origin + "/admin/overview";
  const read = origin + "/api/hosted/v1/admin/overview";
  await waitForServer();
  await old.goto(overview);
  await signInOwner(old, origin);
  await expect(old).toHaveURL(overview);
  expect((await old.context().request.get(read)).status()).toBe(200);
  writeFixture(exchange, "owner-session-ready");

  await wait("recovery-ready");
  const { first, second, otherOrigin } = JSON.parse(readFixture(exchange, "listeners.json"));
  const candidates = await Promise.all([enroll(first, undefined, true), enroll(second, otherOrigin, true)]);
  writeFixture(exchange, "replacement-staged");
  await wait("confirm-replacement");
  const results = await Promise.all(candidates.map(page => prove(page, true)));
  expect([...results].sort()).toEqual([200, 409]);
  await candidates[1].unrouteAll({ behavior: "wait" });

  await waitForServer();
  expect((await old.context().request.get(read)).status()).toBe(401);
  await old.goto(overview);
  await signInOwner(old, origin, 401);
  const denied = candidates[results.indexOf(409)];
  await denied.goto(overview);
  await signInOwner(denied, origin, 401);
  const replacement = candidates[results.indexOf(200)];
  await replacement.goto(overview);
  await signInOwner(replacement, origin);
  await expect(replacement).toHaveURL(overview);
  expect((await replacement.context().request.get(read)).status()).toBe(200);
}
