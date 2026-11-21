import { expect } from "@playwright/test";
import { join } from "node:path";
import { supplyConfirmation } from "./supply-confirmation.mjs";

const state = (revision, state, in_pool = false) => ({ revision, state, in_pool, draining: false });

export async function checkSourceServing({ owner, customer, saved, origin, api, sourceId, ownerRead }) {
  const endpoint = api + `admin/connections/${sourceId}/serving`;
  const options = endpoint + "/options";
  const csrf = async () => (await ownerRead()).csrf_token;
  const proof = supplyConfirmation(owner, origin, api, csrf);
  const read = async () => (await (await owner.request.get(api + "admin/connections")).json())
    .data.items.find(source => source.id === sourceId).serving;
  const cmd = { operation_id: crypto.randomUUID(), expected_revision: "0", action: "pause" };
  for (const path of [endpoint, options]) {
    expect((await proof.post(path, cmd, { client: customer.request })).status()).toBe(401);
    expect((await proof.post(path, cmd, { site: "https://foreign.invalid" })).status()).toBe(403);
    expect((await proof.post(path, cmd, { token: "0".repeat(64) })).status()).toBe(403);
  }
  const unsigned = await proof.post(endpoint, cmd);
  expect(unsigned.status()).toBe(403);
  expect((await unsigned.json()).error.code).toBe("fresh_passkey_required");
  expect(await read()).toEqual(state("0", "enabled"));

  const panel = owner.locator(".source-serving");
  const pause = saved.getByRole("button", { name: "Pause", exact: true });
  const check = panel.getByRole("button", { name: "Check result", exact: true });
  const retry = panel.getByRole("button", { name: "Pause", exact: true });
  const resume = saved.getByRole("button", { name: "Resume", exact: true });
  const proposed = [];
  const observe = async route => {
    proposed.push(route.request().postDataJSON());
    await route.continue();
  };
  await owner.route(options, observe);
  const verify = proof.verification + "*/verify";
  try {
    // The first attempt never reaches the server. A lookup cannot apply it.
    await owner.route(verify, route => route.abort("failed"), { times: 1 });
    await pause.press("Enter");
    await expect(check).toBeEnabled();
    await check.click();
    await expect(retry).toBeEnabled();
    expect(await read()).toEqual(state("0", "enabled"));

    // Retrying keeps the exact intent. This time the response is lost after commit.
    await owner.route(verify, async route => {
      const assertion = route.request().postDataJSON();
      expect(assertion).toMatchObject({ type: "public-key" });
      expect(assertion).not.toHaveProperty("action");
      const response = await route.fetch();
      expect(response.status()).toBe(200);
      expect((await response.json()).data).toEqual({ applied: true, operation_id: proposed[0].operation_id });
      await response.dispose();
      await route.abort("failed");
    }, { times: 1 });
    await retry.click();
    await expect(panel.getByRole("alert")).toHaveText("No response received. Check the result.");
    const lookup = owner.waitForRequest(r => r.url() === endpoint && r.method() === "POST");
    await check.click();
    expect((await lookup).postDataJSON()).toEqual(proposed[0]);
    await expect(resume).toBeEnabled();
    await expect(saved.getByRole("status")).toHaveText(/Outside pool.*Paused/);
    expect(proposed).toHaveLength(2);
    expect(proposed[1]).toEqual(proposed[0]);
    expect(proposed[0]).toMatchObject({ expected_revision: "0", action: "pause" });
    expect(await read()).toEqual(state("1", "paused"));
  } finally {
    await owner.unroute(options, observe);
  }

  const viewport = owner.viewportSize();
  for (const [width, theme] of [[320, "dark"], [1024, "light"]]) {
    await owner.setViewportSize({ width, height: 760 });
    await owner.emulateMedia({ colorScheme: theme, reducedMotion: "reduce" });
    await owner.reload();
    await expect(owner.locator(".hosted-entry")).toHaveAttribute("data-theme", theme);
    await expect(resume).toBeVisible();
    await expect(resume).toHaveCSS("border-end-end-radius", "11px");
    const sidebar = owner.locator(".relay-sidebar");
    const connections = sidebar.getByRole("link", { name: "Connections", exact: true });
    if (width === 320) {
      await expect(sidebar).toBeHidden();
      const trigger = owner.getByRole("button", { name: "Menu", exact: true });
      await trigger.press("Enter");
      const drawer = owner.getByRole("dialog", { name: "Navigation" });
      await expect(drawer).toBeVisible();
      await expect(connections).toHaveAttribute("aria-current", "page");
      await expect(drawer.locator(".hosted-mode")).toBeFocused();
      await owner.keyboard.press("Shift+Tab");
      await expect(drawer.getByRole("button", { name: "Sign out" })).toBeFocused();
      if (process.env.RELAY_TEST_CATALOG_CAPTURE)
        await owner.screenshot({ path: join(process.env.RELAY_TEST_CATALOG_CAPTURE, "owner-drawer-320-dark.png") });
      await owner.keyboard.press("Escape");
      await expect(drawer).toBeHidden();
      await expect(trigger).toBeFocused();
    } else {
      await expect(connections).toHaveAttribute("aria-current", "page");
      await expect(sidebar.getByRole("link", { name: "Payment methods" })).toHaveAttribute("href", "/admin/payment-methods");
      await expect(sidebar.getByRole("link", { name: "Models" })).toHaveCount(0);
      await expect(owner.locator(".hosted-owner-page")).not.toHaveCSS("width", "680px");
      await sidebar.getByRole("button", { name: "Collapse menu" }).click();
      await expect(sidebar).toHaveCSS("width", "56px");
      await sidebar.getByRole("button", { name: "Expand menu" }).click();
      await expect(sidebar).toHaveCSS("width", "232px");
    }
    expect(await owner.locator(".relay-content").evaluate(node => node.scrollWidth <= node.clientWidth)).toBe(true);
    expect(await resume.evaluate(node => {
      const box = node.getBoundingClientRect();
      return box.height >= 44 && box.left >= 0 && box.right <= innerWidth
        && document.documentElement.scrollWidth <= innerWidth;
    })).toBe(true);
    if (process.env.RELAY_TEST_CATALOG_CAPTURE)
      await owner.screenshot({ path: join(process.env.RELAY_TEST_CATALOG_CAPTURE, `source-paused-${width}-${theme}.png`), fullPage: true });
  }
  if (viewport) await owner.setViewportSize(viewport);
  await owner.emulateMedia({ colorScheme: null, reducedMotion: null });
  await resume.click();
  await expect(pause).toBeVisible();
  expect(await read()).toEqual(state("2", "enabled"));
  const replay = await proof.post(endpoint, proposed[0]);
  expect(replay.status()).toBe(200);
  expect((await replay.json()).data).toEqual({ applied: true, operation_id: proposed[0].operation_id });
  const rebound = await proof.post(options, { ...proposed[0], expected_revision: "2" });
  expect(rebound.status()).toBe(409);
  expect((await rebound.json()).error.code).toBe("source_serving_changed");
  expect(await read()).toEqual(state("2", "enabled"));
}

export async function checkSourceLifecycle({ owner, saved, origin, api, sourceId, ownerRead, models }) {
  const endpoint = api + `admin/connections/${sourceId}/serving`;
  const proof = supplyConfirmation(owner, origin, api, async () => (await ownerRead()).csrf_token);
  const read = async () => (await (await owner.request.get(api + "admin/connections")).json())
    .data.items.find(source => source.id === sourceId);
  await owner.getByRole("link", { name: "Pool", exact: true }).click();
  const before = await read();
  expect(before.serving).toEqual(state("2", "enabled", true));
  expect(before.verified_models).toContain("fixture-text");
  const panel = owner.locator(".source-serving");
  const dialog = owner.getByRole("dialog");
  const edit = saved.getByRole("button", { name: "Edit Test connection", exact: true });
  const park = dialog.getByRole("button", { name: "Remove from pool", exact: true });
  const confirmPark = dialog.getByRole("button", { name: "Yes, remove from pool" });
  const settings = dialog.getByRole("tab", { name: "Settings", exact: true });
  await edit.press("Enter");
  await expect(dialog.getByText("fixture-text", { exact: true })).toBeVisible();
  await settings.press("Enter");
  await expect(park).toBeEnabled();
  await park.press("Enter");
  await expect(dialog.getByRole("button", { name: "Cancel", exact: true })).toBeFocused();
  await confirmPark.press("Escape");
  await expect(edit).toBeFocused();
  expect(await read()).toEqual(before);
  await edit.click();
  await settings.click();
  await park.click();
  await confirmPark.click();
  await expect(saved).toHaveCount(0);
  await owner.getByRole("link", { name: "Connections", exact: true }).click();
  await expect(saved.getByRole("status")).toHaveText(/Outside pool.*Paused/);
  expect(await read()).toEqual({ ...before, verified_models: [], serving: state("3", "paused") });
  expect((await models()).some(m => m.model === "fixture-text")).toBe(false);

  const viewport = owner.viewportSize();
  await owner.setViewportSize({ width: 320, height: 760 });
  await owner.emulateMedia({ colorScheme: "dark", reducedMotion: "reduce" });
  await owner.reload();
  await expect(owner.locator(".hosted-entry")).toHaveAttribute("data-theme", "dark");
  await expect(saved.getByRole("button", { name: "Remove from pool", exact: true })).toHaveCount(0);
  const actions = saved.getByRole("button", { name: "Actions: Test connection", exact: true });
  const trashAction = owner.getByRole("menuitem", { name: "Move to trash", exact: true });
  await expect(actions).toBeEnabled();
  await actions.press("ArrowDown");
  await expect(trashAction).toBeFocused();
  await trashAction.press("Escape");
  await expect(actions).toBeFocused();
  await actions.click();
  await trashAction.click();
  const confirmTrash = dialog.getByRole("button", { name: "Yes, move to trash" });
  await expect(confirmTrash).toBeVisible();
  expect(await confirmTrash.evaluate(node => {
    const box = node.getBoundingClientRect();
    return box.height >= 44 && box.left >= 0 && box.right <= innerWidth
      && document.documentElement.scrollWidth <= innerWidth;
  })).toBe(true);
  if (process.env.RELAY_TEST_CATALOG_CAPTURE)
    await owner.screenshot({ path: join(process.env.RELAY_TEST_CATALOG_CAPTURE, "trash-confirm-320-dark.png"), fullPage: true });
  let command;
  const observe = async route => {
    command = route.request().postDataJSON();
    await route.continue();
  };
  await owner.route(endpoint + "/options", observe);
  await owner.route(proof.verification + "*/verify", async route => {
    const response = await route.fetch();
    expect(response.status()).toBe(200);
    expect((await response.json()).data.operation_id).toBe(command.operation_id);
    await response.dispose();
    await route.abort("failed");
  }, { times: 1 });
  try {
    await confirmTrash.click();
    const check = panel.getByRole("button", { name: "Check result", exact: true });
    await expect(check).toBeVisible();
    await owner.evaluate(() => window.dispatchEvent(new Event("focus")));
    await expect(saved).toHaveCount(0);
    await owner.getByRole("button", { name: "Trash", exact: true }).click();
    await expect(saved.getByRole("status")).toHaveText("In trash");
    const lookup = owner.waitForRequest(r => r.url() === endpoint && r.method() === "POST");
    await check.click();
    expect((await lookup).postDataJSON()).toEqual(command);
    expect(command).toMatchObject({ action: "trash", expected_revision: "3" });
  } finally {
    await owner.unroute(endpoint + "/options", observe);
  }
  await owner.setViewportSize({ width: 1024, height: 760 });
  await owner.emulateMedia({ colorScheme: "light", reducedMotion: "reduce" });
  const restore = saved.getByRole("button", { name: "Restore outside pool", exact: true });
  await expect(restore).toBeEnabled();
  await owner.reload();
  await expect(owner.locator(".hosted-entry")).toHaveAttribute("data-theme", "light");
  await owner.getByRole("button", { name: "Trash", exact: true }).click();
  await expect(restore).toBeVisible();
  expect(await read()).toEqual({ ...before, verified_models: [], serving: state("4", "trashed") });
  if (process.env.RELAY_TEST_CATALOG_CAPTURE)
    await owner.screenshot({ path: join(process.env.RELAY_TEST_CATALOG_CAPTURE, "source-trashed-1024-light.png"), fullPage: true });
  await restore.click();
  await dialog.getByRole("button", { name: "Yes, restore", exact: true }).click();
  await expect(saved).toHaveCount(0);
  await owner.getByRole("button", { name: "API providers", exact: true }).click();
  await expect(saved.getByRole("status")).toHaveText(/Outside pool.*Paused/);
  expect(await read()).toEqual({ ...before, verified_models: [], serving: state("5", "paused") });
  expect((await proof.post(endpoint, command)).status()).toBe(200);
  await owner.getByRole("link", { name: "Pool", exact: true }).click();
  await expect(owner.getByRole("button", { name: "Members", exact: true })).toBeVisible();
  await expect(owner.getByText("fixture-text", { exact: true })).toHaveCount(0);
  await owner.getByRole("link", { name: "Connections", exact: true }).click();
  const resumeOptions = owner.waitForResponse(r => r.url() === endpoint + "/options");
  await saved.getByRole("button", { name: "Resume", exact: true }).click();
  const response = await resumeOptions;
  expect(response.status(), response.ok() ? "Resume confirmation" : await response.text()).toBe(200);
  await expect(saved.getByRole("button", { name: "Pause", exact: true })).toBeEnabled();
  expect(await read()).toEqual({ ...before, verified_models: [], serving: state("6", "enabled") });
  expect((await models()).some(m => m.model === "fixture-text")).toBe(false);
  if (viewport) await owner.setViewportSize(viewport);
  await owner.emulateMedia({ colorScheme: null, reducedMotion: null });
}

export async function checkSourceRemoval({ owner, saved, origin, api, sourceId, ownerRead }) {
  const endpoint = api + `admin/connections/${sourceId}/serving`;
  const proof = supplyConfirmation(owner, origin, api, async () => (await ownerRead()).csrf_token);
  const panel = owner.locator(".source-serving");
  const actions = saved.getByRole("button", { name: /^Actions: / });
  const dialog = owner.getByRole("dialog");
  const remove = owner.getByRole("menuitem", { name: "Delete permanently", exact: true });
  await expect(remove).toHaveCount(0);
  await actions.click();
  await owner.getByRole("menuitem", { name: "Move to trash", exact: true }).click();
  await dialog.getByRole("button", { name: "Yes, move to trash", exact: true }).click();
  await expect(saved).toHaveCount(0);
  await owner.getByRole("button", { name: "Trash", exact: true }).click();
  await expect(saved.getByRole("status")).toHaveText("In trash");
  await actions.click();
  await remove.click();
  const cancel = dialog.getByRole("button", { name: "Cancel", exact: true });
  await expect(cancel).toBeFocused();
  await cancel.press("Escape");
  await expect(actions).toBeFocused();
  await actions.click();
  await remove.click();
  const confirm = dialog.getByRole("button", { name: "Yes, delete", exact: true });
  await expect(confirm).toBeVisible();
  if (process.env.RELAY_TEST_CATALOG_CAPTURE)
    await owner.screenshot({ path: join(process.env.RELAY_TEST_CATALOG_CAPTURE, "source-remove-confirm.png"), fullPage: true });
  const prepared = owner.waitForRequest(r => r.url() === endpoint + "/options" && r.method() === "POST");
  await owner.route(proof.verification + "*/verify", async route => {
    const response = await route.fetch();
    expect(response.status()).toBe(200);
    await response.dispose();
    await route.abort("failed");
  }, { times: 1 });
  await confirm.click();
  const command = (await prepared).postDataJSON();
  expect(command).toMatchObject({ action: "remove", expected_revision: "1" });
  const check = panel.getByRole("button", { name: "Check result", exact: true });
  await expect(check).toBeVisible();
  await owner.evaluate(() => window.dispatchEvent(new Event("focus")));
  await expect(saved).toHaveCount(0);
  await owner.getByRole("button", { name: "API providers", exact: true }).click();
  await expect(saved.getByText("Provider key revoked")).toBeVisible();
  await check.click();
  await expect(panel).toHaveCount(0);
  await expect(saved.getByRole("button", { name: "Restore outside pool", exact: true })).toHaveCount(0);
  expect((await proof.post(endpoint, command)).status()).toBe(200);
  expect((await proof.post(endpoint + "/options", { operation_id: crypto.randomUUID(),
    expected_revision: "2", action: "restore" })).status()).toBe(404);
}
