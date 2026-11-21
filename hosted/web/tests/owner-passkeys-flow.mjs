import { expect } from "@playwright/test";
import { join } from "node:path";
import { signInOwner, virtualOwnerKey } from "./owner-session.mjs";

export async function checkOwnerPasskeys(browser, origin, backup, originalKey) {
  const overview = origin + "/admin/overview";
  const read = origin + "/api/hosted/v1/admin/overview";
  const list = origin + "/api/hosted/v1/admin/passkeys";
  const start = list + "/ceremonies";
  const finish = value => `${start}/${value.ceremony_id}`;
  const snapshot = async (page, url) => {
    const response = await page.request.get(url);
    expect(response.status()).toBe(200);
    return (await response.json()).data;
  };
  const session = async page => ({
    Origin: origin, "X-Relay-CSRF": (await snapshot(page, read)).csrf_token,
  });
  const post = (page, headers, url, data) => page.request.post(url, { headers, data });
  const begin = async (page, headers, data) => {
    const response = await post(page, headers, start, data);
    expect(response.status()).toBe(200);
    const state = (await response.json()).data;
    expect(state.step).toBe("verify");
    expect(state.options.publicKey.userVerification).toBe("required");
    return state;
  };
  const assertKey = (page, state) => page.evaluate(async options => {
    const key = await navigator.credentials.get({ publicKey: PublicKeyCredential.parseRequestOptionsFromJSON(options) });
    return key.toJSON();
  }, state.options.publicKey);

  expect((await backup.request.get(list)).status()).toBe(401);
  expect((await post(backup, { Origin: origin }, start, { action: "add" })).status()).toBe(401);
  await backup.setViewportSize({ width: 320, height: 740 });
  await backup.emulateMedia({ colorScheme: "dark", reducedMotion: "reduce" });
  await backup.goto(overview);
  await signInOwner(backup, origin);
  await expect(backup).toHaveURL(overview);
  const initial = (await snapshot(backup, list)).passkeys;
  expect(initial).toHaveLength(1);
  expect(Object.keys(initial[0]).sort()).toEqual(["created_ms", "current", "id", "label"]);
  const headers = await session(backup);
  const add = { action: "add", label: "Spare device" };
  const revoke = passkey_id => ({ action: "revoke", passkey_id });
  expect((await post(backup, { Origin: origin }, start, add)).status()).toBe(403);
  expect((await post(backup, { ...headers, Origin: "https://foreign.example" }, start, add)).status()).toBe(403);
  const last = await post(backup, headers, start, revoke(initial[0].id));
  expect(last.status()).toBe(409);
  expect((await last.json()).error.code).toBe("last_passkey");

  const spent = await begin(backup, headers, add);
  const proof = await assertKey(backup, spent);
  const other = await begin(backup, headers, add);
  expect((await post(backup, headers, finish(other), proof)).status()).toBe(400);
  expect((await post(backup, headers, finish(spent), {})).status()).toBe(400);
  expect((await post(backup, headers, finish(spent), proof)).status()).toBe(409);
  expect((await snapshot(backup, list)).passkeys).toEqual(initial);

  const panel = backup.locator(".owner-passkeys");
  const summary = panel.locator("summary");
  await expect(summary).toHaveText("Set up a backup passkey");
  await summary.focus();
  await backup.keyboard.press("Enter");
  const addButton = panel.getByRole("button", { name: "Add passkey" });
  await addButton.click();
  await expect(panel.getByLabel("Name (optional)")).toBeFocused();
  await backup.keyboard.press("Escape");
  await expect(addButton).toBeFocused();
  await addButton.click();
  await panel.getByLabel("Name (optional)").fill(add.label);
  const creation = backup.waitForResponse(r => r.url().startsWith(start + "/") && r.status() === 200);
  await panel.getByRole("button", { name: "Verify existing passkey" }).click();
  const createOptions = (await (await creation).json()).data.options.publicKey;
  expect(createOptions.authenticatorSelection.residentKey).toBe("required");
  expect(createOptions.excludeCredentials).toHaveLength(1);
  const first = (await originalKey.cdp.send("WebAuthn.getCredentials", { authenticatorId: originalKey.authenticatorId })).credentials;
  expect(first.length).toBe(1);
  await originalKey.cdp.send("WebAuthn.removeVirtualAuthenticator", { authenticatorId: originalKey.authenticatorId });
  await virtualOwnerKey(backup, originalKey.cdp);
  await panel.getByRole("button", { name: "Create backup passkey" }).click();
  await expect(panel.getByRole("button", { name: "Verify new passkey" })).toBeFocused();
  expect((await snapshot(backup, list)).passkeys).toEqual(initial);
  await panel.getByRole("button", { name: "Verify new passkey" }).click();
  await expect(panel.getByRole("status")).toHaveText("Backup passkey added.");
  await expect(panel.getByText(add.label, { exact: true })).toBeVisible();
  const enrolled = (await snapshot(backup, list)).passkeys;
  expect(enrolled.map(key => key.current)).toEqual([true, false]);
  expect(await backup.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  const shots = process.env.RELAY_TEST_OWNER_SCREENSHOT_DIR;
  const language = backup.locator(".language-select");
  await language.selectOption("ru");
  await expect(backup.locator("html")).toHaveAttribute("lang", "ru");
  expect(await backup.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  const actionEdges = await panel.locator("li > button").evaluateAll(buttons => buttons.map(button => button.getBoundingClientRect().right));
  expect(actionEdges).toHaveLength(2);
  expect(actionEdges[0]).toBe(actionEdges[1]);
  if (shots) await panel.screenshot({ path: join(shots, "owner-passkeys-phone.png") });
  await language.selectOption("en");

  // New sessions must genuinely sign in with both stored credentials.
  await backup.getByRole("button", { name: "Sign out" }).click();
  await expect(backup).toHaveURL(origin + "/admin/login");
  await signInOwner(backup, origin);
  await expect(backup).toHaveURL(overview);
  const original = await browser.newPage({ viewport: { width: 1080, height: 740 } });
  const { cdp, authenticatorId } = await virtualOwnerKey(original);
  await cdp.send("WebAuthn.addCredential", { authenticatorId, credential: first[0] });
  await original.goto(overview);
  await signInOwner(original, origin);
  const a = await session(original), b = await session(backup);
  expect((await snapshot(backup, list)).passkeys.map(key => key.current)).toEqual([false, true]);
  await original.locator(".owner-passkeys summary").click();
  await expect(original.getByRole("button", { name: "Revoke Spare device" })).toBeVisible();
  if (shots) await original.locator(".owner-passkeys").screenshot({ path: join(shots, "owner-passkeys-desktop.png") });

  const removeBackup = await begin(original, a, revoke(enrolled[1].id));
  expect(removeBackup.options.publicKey.allowCredentials).toHaveLength(1);
  const approveBackupRemoval = await assertKey(original, removeBackup);
  // An authenticated owner with a different session cannot use this challenge.
  expect((await post(backup, b, finish(removeBackup), approveBackupRemoval)).status()).toBe(409);

  await summary.click();
  await panel.getByRole("button", { name: "Revoke Owner passkey" }).click();
  await expect(panel.getByRole("button", { name: "Verify and revoke" })).toBeFocused();
  let firstRemoval;
  await backup.route(new RegExp(`${start}/[0-9a-f-]+$`), async route => {
    const [one, two] = await Promise.all([
      post(original, a, finish(removeBackup), approveBackupRemoval),
      route.fetch(),
    ]);
    firstRemoval = one.status();
    await route.fulfill({ response: two });
  }, { times: 1 });
  const removeOriginal = backup.waitForResponse(r => r.url().startsWith(start + "/") && r.request().method() === "POST");
  await panel.getByRole("button", { name: "Verify and revoke" }).click();
  const secondRemoval = (await removeOriginal).status();
  expect([firstRemoval, secondRemoval].sort()).toEqual([200, 401]);
  const [surviving, revoked] = firstRemoval === 200 ? [original, backup] : [backup, original];
  expect((await revoked.request.get(read)).status()).toBe(401);
  const remaining = (await snapshot(surviving, list)).passkeys;
  expect(remaining).toHaveLength(1);
  expect(remaining[0].id).toBe(firstRemoval === 200 ? enrolled[0].id : enrolled[1].id);
  expect((await post(original, a, finish(removeBackup), approveBackupRemoval)).status()).toBe(409);
  await revoked.goto(overview);
  await signInOwner(revoked, origin, 401);
}
