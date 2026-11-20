import { chromium, expect } from "@playwright/test";
import { createHash, X509Certificate } from "node:crypto";
import { dirname } from "node:path";
import { readFixture, writeFixture, fixtureOrigin } from "../../test-support/files.mjs";

const [rawOrigin, file, mode, otherOrigin, otherFile, journey] = process.argv.slice(2);
const sourceCopy = mode === "source-copy";
const origin = fixtureOrigin(rawOrigin, { secure: sourceCopy });
const args = [];
if (sourceCopy) {
  const cert = new X509Certificate(readFixture(process.env.RELAY_TEST_EDGE_CERT));
  const key = cert.publicKey.export({ type: "spki", format: "der" });
  args.push("--ignore-certificate-errors-spki-list=" + createHash("sha256").update(key).digest("base64"));
}
const browser = await chromium.launch({ executablePath: process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH, args });
async function enroll(codePath, listener, recovery = false) {
  const page = await browser.newPage();
  if (listener) {
    listener = fixtureOrigin(listener, { numeric: true });
    // Both private setup processes use the same RP/origin behind two test edges.
    await page.route(origin + "/**", async route => {
      const res = await route.fetch({ url: listener + new URL(route.request().url()).pathname, maxRedirects: 0 });
      await route.fulfill({ response: res });
    });
  }
  const cdp = await page.context().newCDPSession(page);
  await cdp.send("WebAuthn.enable");
  await cdp.send("WebAuthn.addVirtualAuthenticator", { options: {
    protocol: "ctap2", transport: "internal", hasResidentKey: true, hasUserVerification: true,
    isUserVerified: true, automaticPresenceSimulation: true,
  } });
  await page.goto(origin + (recovery ? "/setup/recovery" : "/setup/owner"));
  await page.getByLabel(recovery ? "Local recovery code" : "Local setup code").fill(readFixture(codePath));
  await page.getByRole("button", { name: "Create passkey" }).click();
  await expect(page.getByRole("button", { name: "Verify passkey" })).toBeVisible();
  return page;
}

async function prove(page, recovery = false) {
  const ack = page.waitForResponse(r => r.url() === origin + "/api/hosted/v1/setup/prove");
  await page.getByRole("button", { name: "Verify passkey" }).click({ timeout: 8000 });
  const status = (await ack).status();
  if (status === 200) await expect(page.getByRole("status")).toHaveText(recovery
    ? "Owner access recovered. Previous passkeys and sessions are no longer valid."
    : "Owner passkey verified. The service is still closed.");
  else await expect(page.getByRole("alert")).toBeVisible();
  expect(await page.context().cookies()).toEqual([]);
  return status;
}

async function waitForServer() {
  await expect.poll(async () => {
    try { return (await fetch(origin + "/api/hosted/v1/public/config", { redirect: "error" })).status; }
    catch { return 0; }
  }, { timeout: 20000 }).toBe(200);
}

try {
  const page = await enroll(file);
  if (mode === "invalid-signature") {
    await page.route(origin + "/api/hosted/v1/setup/prove", async route => {
      const data = route.request().postDataJSON();
      data.response.signature = "AA";
      await route.continue({ postData: JSON.stringify(data) });
    });
    expect(await prove(page)).toBe(400);
  } else if (mode === "customer-identity") {
    expect(await prove(page)).toBe(200);
    const { checkCustomerJourney } = await import("./customer-journey.mjs");
    await checkCustomerJourney({ browser, owner: page, origin, issuer: otherOrigin, exchange: otherFile, funded:journey==="funded" });
  } else if (mode === "recover-owner") {
    expect(await prove(page)).toBe(200);
    const { checkOwnerRecovery } = await import("./owner-recovery-flow.mjs");
    await checkOwnerRecovery({ origin, old: page, enroll, prove, waitForServer, exchange: otherOrigin });
  } else if (sourceCopy) {
    expect(await prove(page)).toBe(200);
    const { checkSourceCopy } = await import("./source-copy.mjs");
    await checkSourceCopy(page, origin, otherOrigin);
  } else {
    const second = await enroll(otherFile, otherOrigin);
    // Each browser already holds a verified registration and a fresh proof challenge.
    const result = await Promise.all([prove(page), prove(second)]);
    expect([...result].sort()).toEqual([200, 409]);
    await second.unrouteAll({ behavior: "wait" });
    // Rust replaces both local setup processes with the normal server on the same RP.
    await waitForServer();
    const { checkOwnerSession } = await import("./owner-session.mjs");
    const [owner, foreign] = result[0] === 200 ? [page, second] : [second, page];
    await checkOwnerSession(origin, owner, foreign);
    writeFixture(dirname(dirname(file)), "restart-owner-guard");
    const read = origin + "/api/hosted/v1/admin/overview";
    await expect.poll(async () => {
      try { return (await owner.context().request.get(read)).status(); } catch { return 0; }
    }, { timeout: 15000 }).toBe(401);
    await owner.goto(origin + "/admin/overview");
    await expect(owner).toHaveURL(origin + "/admin/login");
  }
} finally {
  await browser.close();
}
