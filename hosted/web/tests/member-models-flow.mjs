import { expect } from "@playwright/test";
import { join } from "node:path";

const fixture = "fixture-text", review = "review-text";
const flags = allowed => [fixture, review].map(model => ({ model, allowed }));

export async function checkMemberModels({ owner, customer, endpoint, list, proof, source }) {
  const path = new URL(`members/${source}/models`, endpoint).href;
  const read = async () => {
    const response = await owner.request.get(path);
    expect(response.status()).toBe(200);
    expect(response.headers()["cache-control"]).toBe("no-store");
    return (await response.json()).data;
  };
  const expected = (snapshot, changes) => ({
    operation_id: crypto.randomUUID(), scope_id: snapshot.scope_id, expected_revision: snapshot.revision,
    expected_source_revision: snapshot.source_revision, changes,
  });
  const reject = async (pending, status, code) => {
    const response = await pending;
    expect(response.status()).toBe(status);
    if (code) expect((await response.json()).error.code).toBe(code);
  };
  const published = await list();
  const initial = await read();
  expect(initial.items).toEqual(flags(true));
  const draft = expected(initial, flags(false));
  expect((await fetch(path)).status).toBe(401);
  await reject(customer.request.get(path), 401);
  await reject(owner.request.get(path + "?model=" + fixture), 400, "invalid_member_models");
  for (const route of [path, path + "/options"]) {
    await reject(proof.post(route, draft, { client: customer.request }), 401);
    await reject(proof.post(route, draft, { site: "https://foreign.invalid" }), 403);
    await reject(proof.post(route, draft, { token: "0".repeat(64) }), 403);
    await reject(proof.post(route, { ...draft, changes: [...draft.changes, draft.changes[0]] }), 400, "invalid_member_models");
  }
  await reject(proof.post(path, draft), 403, "fresh_passkey_required");
  await reject(proof.post(path + "/options", { ...draft, changes: [{ model: "not-admitted", allowed: false }] }),
    409, "member_models_changed");
  expect(await read()).toEqual(initial);

  await owner.bringToFront();
  await owner.getByRole("button", { name: "Members", exact: true }).click();
  const pencil = owner.getByRole("button", { name: "Edit Test connection", exact: true });
  const dialog = owner.getByRole("dialog", { name: "Edit member", exact: true });
  const modelSwitch = model => dialog.getByRole("switch", { name: `Allow ${model}`, exact: true });
  const save = dialog.getByRole("button", { name: "Save", exact: true });
  const retry = dialog.getByRole("button", { name: "Retry", exact: true });
  const open = async () => {
    await pencil.click();
    await expect(modelSwitch(fixture)).toBeVisible();
  };

  await owner.route(path, route => route.fulfill({ status: 503 }), { times: 1 });
  await pencil.click();
  await expect(dialog.getByRole("alert")).toHaveText("Cannot connect to a compatible Relay server.");
  await expect(dialog.getByRole("switch")).toHaveCount(0);
  await retry.click();
  await expect(modelSwitch(fixture)).toBeChecked();
  await expect(save).toBeDisabled();
  const models = dialog.getByRole("tab", { name: "Models", exact: true });
  await models.press("End");
  await expect(dialog.getByRole("button", { name: "Pause", exact: true })).toBeEnabled();
  await dialog.getByRole("tab", { name: "Settings", exact: true }).press("Home");
  await expect(models).toBeFocused();
  await modelSwitch(fixture).press("Space");
  await expect(modelSwitch(fixture)).not.toBeChecked();
  expect(await read()).toEqual(initial);
  await owner.keyboard.press("Escape");
  await expect(pencil).toBeFocused();

  const viewport = owner.viewportSize();
  const shell = owner.locator(".hosted-app");
  const originalTheme = await shell.getAttribute("data-theme");
  const themePreference = await owner.evaluate(() => localStorage.getItem("relay-hosted-theme"));
  const setTheme = async theme => {
    if (await shell.getAttribute("data-theme") !== theme)
      await owner.locator("[data-theme-toggle]").click();
    await expect(shell).toHaveAttribute("data-theme", theme);
  };
  await owner.setViewportSize({ width: 1024, height: 760 });
  await owner.emulateMedia({ reducedMotion: "reduce" });
  await setTheme("light");
  await open();
  await expect(modelSwitch(fixture)).toBeChecked();
  const capture = async name => {
    if (process.env.RELAY_TEST_CATALOG_CAPTURE)
      await owner.screenshot({ path: join(process.env.RELAY_TEST_CATALOG_CAPTURE, name) });
  };
  await capture("live-member-models-1024-light.png");

  await modelSwitch(fixture).uncheck();
  await modelSwitch(review).uncheck();
  expect(await list()).toEqual(published);
  const writes = [];
  const track = async route => { writes.push(route.request().postDataJSON()); await route.continue(); };
  await owner.route(path + "/options", track);
  await owner.route(proof.verification + "*/verify", async route => {
    const committed = await route.fetch();
    expect(committed.status()).toBe(200);
    expect((await committed.json()).data).toEqual({ applied: true, operation_id: writes[0].operation_id });
    await committed.dispose();
    await route.abort();
  }, { times: 1 });
  try {
    await save.click();
    await expect(dialog.getByRole("alert")).toHaveText(/Could not confirm the change/);
    await expect(modelSwitch(review)).toBeDisabled();
    await retry.click();
    await expect(dialog).toBeHidden();
  } finally { await owner.unroute(path + "/options", track); }
  expect(writes).toHaveLength(2);
  expect(writes[1]).toEqual(writes[0]);
  expect(writes[0].changes).toEqual(flags(false));
  const disabled = await read();
  expect(disabled.items).toEqual(flags(false));
  await reject(proof.post(path, writes[0]), 200);
  await reject(proof.post(path, { ...writes[0], changes: flags(true) }), 409, "member_models_changed");
  expect(await list()).toEqual(published.filter(row => row.model !== fixture));
  await expect(pencil).toBeFocused();

  await owner.setViewportSize({ width: 320, height: 640 });
  await setTheme("dark");
  await open();
  await expect(modelSwitch(fixture)).not.toBeChecked();
  expect(await dialog.evaluate(el => {
    const rect = el.getBoundingClientRect(), footer = el.querySelector("footer")?.getBoundingClientRect();
    return rect.left >= 0 && rect.right <= innerWidth && rect.top >= 0 && rect.bottom <= innerHeight
      && el.scrollWidth <= el.clientWidth && footer && footer.bottom <= rect.bottom
      && [...el.querySelectorAll("li label")].every(label => label.getBoundingClientRect().height >= 44);
  })).toBe(true);
  await capture("live-member-models-320-dark.png");

  // An edit confirmed elsewhere makes this local draft stale.
  await modelSwitch(fixture).check();
  const external = expected(disabled, [{ model: review, allowed: true }]);
  const challenge = await proof.prepare(path, external);
  await reject(proof.post(proof.path(challenge), await proof.assertion(challenge)), 200);
  await save.click();
  await expect(dialog.getByRole("alert")).toHaveText("Model settings changed. Review the current selection.");
  await expect(modelSwitch(fixture)).not.toBeChecked();
  await expect(modelSwitch(review)).toBeChecked();
  await expect(save).toBeDisabled();
  await modelSwitch(fixture).check();
  await save.click();
  await expect(dialog).toBeHidden();
  expect(await list()).toEqual(published);
  expect((await read()).items).toEqual(flags(true));
  if (viewport) await owner.setViewportSize(viewport);
  await setTheme(originalTheme);
  await owner.evaluate(preference => {
    if (preference === null) localStorage.removeItem("relay-hosted-theme");
    else localStorage.setItem("relay-hosted-theme", preference);
  }, themePreference);
  await owner.emulateMedia({ reducedMotion: null });
  await owner.getByRole("button", { name: "Model rules", exact: true }).click();
}
