import { expect } from "@playwright/test";
import { join } from "node:path";

export async function checkPoolRotation({ owner, customer, endpoint, proof, list }) {
  const path = new URL("rotation", endpoint).href;
  const read = async () => {
    const response = await owner.request.get(path);
    expect(response.status()).toBe(200);
    expect(response.headers()["cache-control"]).toBe("no-store");
    return (await response.json()).data;
  };
  const initial = await read();
  expect(initial.mode).toBe("automatic");
  expect(initial.members.map(row => row.label).sort()).toEqual(["Second connection", "Test connection"]);
  const desired = initial.members.map(row => row.source_id).reverse();
  const draft = {
    operation_id: crypto.randomUUID(), scope_id: initial.scope_id, expected_revision: initial.revision,
    mode: "failover", member_order: desired,
  };
  expect((await customer.request.get(path)).status()).toBe(401);
  expect((await owner.request.get(path + "?scope=" + initial.scope_id)).status()).toBe(400);
  expect((await proof.post(path + "/options", draft, { client: customer.request })).status()).toBe(401);
  expect((await proof.post(path + "/options", draft, { site: "https://outside.invalid" })).status()).toBe(403);
  expect((await proof.post(path + "/options", draft, { token: "0".repeat(64) })).status()).toBe(403);
  expect((await proof.post(path + "/options", { ...draft, mode: "round_robin" })).status()).toBe(400);
  expect((await proof.post(path + "/options", { ...draft, member_order: [desired[0], desired[0]] })).status()).toBe(400);
  const unsigned = await proof.post(path, draft);
  expect(unsigned.status()).toBe(403);
  expect((await unsigned.json()).error.code).toBe("fresh_passkey_required");

  const catalog = await list();
  await owner.bringToFront();
  await owner.getByRole("button", { name: "Members", exact: true }).click();
  const opener = owner.getByRole("button", { name: "Pool rotation settings", exact: true });
  const dialog = owner.getByRole("dialog", { name: "Pool rotation settings", exact: true });
  const automatic = dialog.getByRole("radio", { name: "Automatic", exact: true });
  const failover = dialog.getByRole("radio", { name: "Primary & reserve", exact: true });
  const save = dialog.getByRole("button", { name: "Save", exact: true });
  const rank = dialog.getByRole("list", { name: "Member order", exact: true });
  const labels = initial.members.map(row => row.label).reverse();
  async function edit() {
    await opener.click();
    await expect(automatic).toBeChecked();
    await expect(dialog.getByRole("radio")).toHaveCount(2);
    await expect(dialog.getByRole("spinbutton")).toHaveCount(0);
    await expect(rank).toBeHidden();
    await automatic.press("End");
    await expect(failover).toBeChecked();
    await expect(failover).toBeFocused();
    await dialog.getByRole("button", { name: `Move ${initial.members[0].label} down`, exact: true }).press("Enter");
    await expect(rank.locator(".owner-rotation-name strong")).toHaveText(labels);
  }

  await edit();
  await owner.keyboard.press("Escape");
  await expect(opener).toBeFocused();
  expect(await read()).toEqual(initial);
  await edit();
  const commands = [];
  const track = async route => { commands.push(route.request().postDataJSON()); await route.continue(); };
  await owner.route(path + "/options", track);
  await owner.route(proof.verification + "*/verify", async route => {
    const response = await route.fetch();
    expect(response.status()).toBe(200);
    await response.dispose();
    await route.abort("failed");
  }, { times: 1 });
  try {
    await save.click();
    await expect(dialog.getByRole("alert")).toHaveText(/Could not confirm the change/);
    await expect(automatic).toBeDisabled();
    await dialog.getByRole("button", { name: "Retry", exact: true }).click();
    await expect(dialog).toBeHidden();
  } finally {
    await owner.unroute(path + "/options", track);
  }
  expect(commands).toHaveLength(2);
  expect(commands[1]).toEqual(commands[0]);
  expect(commands[0]).toMatchObject({ mode: "failover", member_order: desired, expected_revision: initial.revision });
  const current = await read();
  expect(current.mode).toBe("failover");
  expect(current.members.map(row => row.source_id)).toEqual(desired);
  expect((await proof.post(path, commands[0])).status()).toBe(200);
  const stale = await proof.post(path + "/options", draft);
  expect(stale.status()).toBe(409);
  expect((await stale.json()).error.code).toBe("pool_rotation_changed");
  expect(await list()).toEqual(catalog);
  await expect(owner.locator(".live-pool-strategy")).toHaveText("Primary & reserve");
  await expect(opener).toBeFocused();
  await owner.reload();
  const currentOrder = () => owner.locator(".live-pool-card").evaluateAll(rows => rows.map(row => row.dataset.sourceId));
  await expect.poll(currentOrder).toEqual(desired);

  if (process.env.RELAY_TEST_CATALOG_CAPTURE) {
    const viewport = owner.viewportSize();
    const shell = owner.locator(".hosted-entry");
    const original = await shell.getAttribute("data-theme");
    const preference = await owner.evaluate(() => localStorage.getItem("relay-hosted-theme"));
    await owner.emulateMedia({ reducedMotion: "reduce" });
    const theme = async name => {
      if (await shell.getAttribute("data-theme") !== name) await owner.locator("[data-theme-toggle]").click();
      await expect(shell).toHaveAttribute("data-theme", name);
    };
    for (const [width, palette] of [[1024, "light"], [320, "dark"]]) {
      await owner.setViewportSize({ width, height: 700 });
      await theme(palette);
      await opener.click();
      await expect(rank.locator(".owner-rotation-name strong")).toHaveText(labels);
      expect(await dialog.evaluate(node => {
        const rect = node.getBoundingClientRect();
        return rect.left >= 0 && rect.right <= innerWidth && rect.top >= 0 && rect.bottom <= innerHeight
          && node.scrollWidth <= node.clientWidth && [...node.querySelectorAll(".owner-rotation-order-actions button")].every(button => {
            const box = button.getBoundingClientRect();
            return box.width >= 44 && box.height >= 44 && box.left >= rect.left && box.right <= rect.right;
          });
      })).toBe(true);
      await owner.screenshot({ path: join(process.env.RELAY_TEST_CATALOG_CAPTURE, `live-rotation-${width}-${palette}.png`) });
      await owner.keyboard.press("Escape");
      await expect(opener).toBeFocused();
    }
    if (viewport) await owner.setViewportSize(viewport);
    await theme(original);
    await owner.evaluate(value => {
      if (value === null) localStorage.removeItem("relay-hosted-theme");
      else localStorage.setItem("relay-hosted-theme", value);
    }, preference);
    await owner.emulateMedia({ reducedMotion: null });
  }
  await opener.click();
  await expect(failover).toBeChecked();
  await automatic.click();
  const start = owner.waitForResponse(response => response.url() === path + "/options");
  const confirmation = owner.waitForResponse(response =>
    response.url().startsWith(proof.verification) && response.request().method() === "POST").catch(() => undefined);
  await save.click();
  const options = await start;
  expect(options.status(), options.ok() ? "Restore Automatic" : (await options.json()).error.code).toBe(200);
  const verify = await confirmation;
  expect(verify, "Passkey verification response").toBeDefined();
  expect(verify.status(), verify.ok() ? "Confirm Automatic" : (await verify.json()).error.code).toBe(200);
  await expect(dialog).toBeHidden();
  const restored = await read();
  expect(restored.mode).toBe("automatic");
  expect(restored.members.map(row => row.source_id)).toEqual(desired);
  expect(await list()).toEqual(catalog);
  await owner.getByRole("button", { name: "Model rules", exact: true }).click();
}
