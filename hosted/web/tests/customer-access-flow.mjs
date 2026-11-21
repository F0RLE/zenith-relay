import { expect } from "@playwright/test";
import { join } from "node:path";

export async function checkCustomerAccess({ owner, a, b, api, origin, login, signOwner, account, funded }) {
  const workspace = (await account(a)).workspace_id, other = (await account(b)).workspace_id;
  const path = `/admin/customers/${workspace}`;
  const endpoint = api + path.slice(1), otherEndpoint = api + `admin/customers/${other}`;
  const ownerHttp = await signOwner();
  const csrf = (await (await ownerHttp.get(api + "admin/overview")).json()).data.csrf_token;
  const headers = { Origin: origin, "X-Relay-CSRF": csrf };
  const customerHeaders = { Origin: origin, "X-Relay-CSRF": (await account(a)).csrf_token };
  const state = async (url = endpoint) => (await (await ownerHttp.get(url)).json()).data;
  const command = (action, reason, expected) => ({
    action, reason_code: reason, expected_revision: expected, operation_id: crypto.randomUUID(),
  });
  const verifyMatch = url => url.pathname.startsWith("/api/hosted/v1/admin/customers/confirmations/") && url.pathname.endsWith("/verify");
  const status = value => owner.locator(".customer-case-status").getByText(value, { exact: true });

  await a.getByRole("link", { name: "Account security" }).click();
  await expect(a.getByRole("heading", { name: "Close account" })).toBeVisible();
  await expect(a.getByRole("link", { name: "Contact support" })).toHaveAttribute("href", "https://docs.example.org/support");
  await expect(a.locator(".account-closure-reference code")).toHaveText(workspace);
  expect(await state()).toMatchObject({ workspace_id: workspace, state: "active", revision: "1", last_change: null });
  const closeStale = command("close", "verified_customer_request", "1");
  expect((await a.request.get(endpoint)).status()).toBe(401);
  expect((await a.request.post(endpoint + "/options", { headers: customerHeaders, data: closeStale })).status()).toBe(401);
  expect((await ownerHttp.get(api + "admin/customers")).status()).toBe(404);
  expect((await ownerHttp.get(api + `admin/customers/${crypto.randomUUID()}`)).status()).toBe(404);
  expect((await ownerHttp.post(endpoint + "/options", { data: closeStale })).status()).toBe(403);
  expect((await ownerHttp.post(endpoint + "/options", {
    headers: { ...headers, Origin: "http://127.0.0.1:1" }, data: closeStale,
  })).status()).toBe(403);
  expect((await ownerHttp.post(endpoint, { headers, data: closeStale })).status()).toBe(403);
  expect((await ownerHttp.post(endpoint + "/options", {
    headers, data: { ...closeStale, reason_code: "policy_review" },
  })).status()).toBe(400);

  if (!funded) {
    await owner.goto(origin + "/admin/payments");
    await owner.getByRole("link", { name: `Open customer case ${workspace}` }).first().click();
  } else {
    await owner.goto(origin + `/admin/usage?workspace_id=${workspace}`);
    await owner.getByRole("link", { name: `Open customer case ${workspace}` }).click();
  }
  await expect(owner).toHaveURL(origin + path);
  await expect(status("Active")).toBeVisible();
  const suspend = owner.getByRole("button", { name: "Suspend access" });
  await suspend.click();
  await expect(owner.getByLabel("Reason")).toBeFocused();
  await expect(owner.getByRole("button", { name: "Confirm with passkey" })).toBeDisabled();
  await owner.keyboard.press("Escape");
  await expect(suspend).toBeFocused();
  await suspend.click();
  await owner.getByLabel("Reason").selectOption("security_review");
  const confirmed = owner.waitForResponse(r => verifyMatch(new URL(r.url())));
  await owner.getByRole("button", { name: "Confirm with passkey" }).click();
  expect((await confirmed).status()).toBe(200);
  await expect(status("Suspended")).toBeVisible();
  expect((await ownerHttp.post(endpoint + "/options", { headers, data: closeStale })).status()).toBe(409);
  expect((await a.request.get(api + "account")).status()).toBe(401);
  expect((await b.request.get(api + "account")).status()).toBe(200);
  await login(a);
  await expect(a.getByRole("alert")).toHaveText(/Access was not granted/);

  await owner.getByRole("button", { name: "Reinstate access" }).click();
  await owner.getByRole("button", { name: "Confirm with passkey" }).click();
  await expect(status("Active")).toBeVisible();
  expect((await a.request.get(api + "account")).status()).toBe(401);
  await login(a);
  await expect(a.locator(".hosted-tools .hosted-balance")).toBeVisible();
  const activeAccount = await account(a), keys = (await (await a.request.get(api + "keys")).json()).data.keys;
  expect(keys.length).toBeGreaterThan(0);
  expect(keys.map(key => key.status).every(v => v === "revoked")).toBe(true);
  const freshHeaders = { Origin: origin, "X-Relay-CSRF": activeAccount.csrf_token };
  const sessions = (await (await a.request.get(api + "account/sessions")).json()).data.sessions;
  const activeSession = sessions.find(session => session.current).id;
  const reissue = await a.request.post(api + "keys", {
    headers: freshHeaders,
    data: { name: "Old key", operation_id: crypto.randomUUID(), replaces_key_id: keys[0].id },
  });
  expect(reissue.status()).toBe(409);
  const newKey = await a.request.post(api + "keys", {
    headers: freshHeaders, data: { name: "New key", operation_id: crypto.randomUUID() },
  });
  expect(newKey.status()).toBe(200);
  expect((await newKey.json()).data.key.status).toBe("active");

  const closeButton = owner.getByRole("button", { name: "Close account" });
  if (!funded) {
    const capture = process.env.RELAY_TEST_CUSTOMER_ACCESS_CAPTURE;
    const panel = owner.locator(".customer-case-panel");
    for (const [width, theme] of [[1200, "light"], [1200, "dark"], [320, "light"], [320, "dark"]]) {
      await owner.setViewportSize({ width, height: 820 });
      await owner.evaluate(theme => localStorage.setItem("relay-hosted-theme", theme), theme);
      await owner.reload();
      await expect(owner.locator(".hosted-entry")).toHaveAttribute("data-theme", theme);
      await closeButton.click();
      await expect(panel.getByRole("button", { name: "Cancel" })).toBeFocused();
      await owner.keyboard.press("Tab");
      const confirm = panel.getByRole("button", { name: "Confirm with passkey" });
      await expect(confirm).toBeFocused();
      await expect(confirm).toHaveCSS("outline-style", "solid");
      expect(await panel.evaluate(el => document.documentElement.scrollWidth <= innerWidth &&
        [el, ...el.querySelectorAll("button, code")].every(child => {
          const rect = child.getBoundingClientRect();
          return rect.left >= 0 && rect.right <= innerWidth;
        }))).toBe(true);
      if (capture) await owner.screenshot({ path: join(capture, `close-${width}-${theme}.png`), fullPage: true });
    }
  } else await closeButton.click();
  const options = owner.waitForRequest(r => r.url() === endpoint + "/options");
  let sentProof;
  await owner.route(verifyMatch, async route => {
    sentProof = { url: route.request().url(), body: route.request().postDataJSON() };
    expect((await route.fetch()).status()).toBe(200);
    await route.abort();
  }, { times: 1 });
  await owner.getByRole("button", { name: "Confirm with passkey" }).click();
  const close = (await options).postDataJSON();
  await expect(owner.getByRole("button", { name: "Check result" })).toBeFocused();
  expect((await ownerHttp.post(sentProof.url, { headers, data: sentProof.body })).status()).toBe(409);
  expect((await ownerHttp.post(otherEndpoint, { headers, data: close })).status()).toBe(409);
  const readReceipt = owner.waitForResponse(r => r.url() === endpoint && r.request().method() === "POST");
  await owner.getByRole("button", { name: "Check result" }).click();
  expect((await (await readReceipt).json()).data).toMatchObject({ applied: true, operation_id: close.operation_id, revision: "4" });
  await expect(status("Closed")).toBeVisible();
  await expect(owner.getByRole("button", { name: "Reinstate access" })).toHaveCount(0);
  await expect(owner.getByRole("button", { name: "Close account" })).toHaveCount(0);
  expect((await state()).last_change).toMatchObject({ operation_id: close.operation_id, action: "close" });
  expect((await state(otherEndpoint)).state).toBe("active");
  expect((await ownerHttp.post(endpoint + "/options", {
    headers, data: command("reinstate", "review_complete", "4"),
  })).status()).toBe(409);
  for (const resource of ["account", "account/sessions", "keys", "notifications"])
    expect((await a.request.get(api + resource)).status()).toBe(401);
  expect((await a.request.delete(api + "account/sessions/" + activeSession, { headers: freshHeaders })).status()).toBe(401);
  expect((await a.request.post(api + "keys", {
    headers: freshHeaders, data: { name: "Blocked", operation_id: crypto.randomUUID() },
  })).status()).toBe(401);
  expect((await b.request.get(api + "keys")).status()).toBe(200);
  await login(a);
  await expect(a.getByRole("alert")).toHaveText(/Access was not granted/);
  if (!funded && process.env.RELAY_TEST_CUSTOMER_ACCESS_CAPTURE)
    await owner.screenshot({ path: join(process.env.RELAY_TEST_CUSTOMER_ACCESS_CAPTURE, "closed-320.png"), fullPage: true });
  await owner.setViewportSize({ width: 1024, height: 760 });
}
