import { expect } from "@playwright/test";
import { existsSync } from "node:fs";
import { join } from "node:path";

export async function checkPurchaseLimit({ owner, customer, origin, api, sourceId, ownerRead, issue, wait, exchange }) {
  const endpoint = api + `admin/connections/${sourceId}/purchase-limit`;
  const oh = owner.context().request, ah = customer.context().request;
  const csrf = async () => (await ownerRead()).csrf_token;
  const send = (client, token, data, from = origin) =>
    client.post(endpoint, { headers: { Origin: from, "X-Relay-CSRF": token }, data });
  const command = (revision, limit) => ({
    operation_id: crypto.randomUUID(), expected_revision: revision, limit_micro_usd: limit,
  });
  const read = async () => {
    const r = await oh.get(endpoint);
    expect(r.status()).toBe(200);
    expect(r.headers()["cache-control"]).toBe("no-store");
    return (await r.json()).data;
  };
  await wait("purchase-recorded");
  await owner.getByRole("link", { name: "Connections", exact: true }).click();
  const panel = owner.locator(".source-purchase");
  const expand = () => panel.locator("summary").click();
  await expand();
  const monthly = panel.getByRole("radio", { name: "Monthly limit" });
  const unlimited = panel.getByRole("radio", { name: "No Relay limit" });
  const amount = panel.getByLabel("Limit (USD)");
  const save = panel.getByRole("button", { name: "Save", exact: true });
  await expect(unlimited).toBeChecked();
  await expect(save).toBeDisabled();
  expect(await read()).toEqual({ source_id: sourceId, revision: "0", month_utc: expect.stringMatching(/^\d{4}-\d\d-01$/),
    limit_micro_usd: null, accounted_micro_usd: "310000", outstanding_micro_usd: "0", remaining_micro_usd: null, unpriced: false });
  const token = await csrf();
  const low = command("0", "929999");
  expect((await fetch(endpoint)).status).toBe(401);
  expect((await ah.get(endpoint)).status()).toBe(401);
  expect((await oh.get(api + `admin/connections/${crypto.randomUUID()}/purchase-limit`)).status()).toBe(404);
  expect((await send(ah, token, low)).status()).toBe(401);
  expect((await send(oh, "0".repeat(64), low)).status()).toBe(403);
  expect((await send(oh, token, low, "https://elsewhere.invalid")).status()).toBe(403);
  expect((await send(oh, token, { operation_id: low.operation_id, expected_revision: "0" })).status()).toBe(400);
  const stale = await send(oh, token, low);
  expect(stale.status()).toBe(403);
  expect((await stale.json()).error.code).toBe("fresh_passkey_required");
  await unlimited.press("ArrowRight");
  await expect(monthly).toBeChecked();
  await amount.fill("0.1234567");
  await expect(amount).toHaveAttribute("aria-invalid", "true");
  await expect(save).toBeDisabled();
  await amount.fill("0,929999");
  await save.click();
  await expect.poll(async () => (await read()).revision).toBe("1");
  await expect(save).toBeDisabled();
  await expect(amount).toHaveValue("0.929999");
  expect((await csrf()) !== token).toBe(true);
  issue("limit-too-low");
  await wait("cap-rejected");

  // A concurrent edit disables the cap but does not erase recorded spend.
  await amount.fill("0.93");
  const disable = command("1", null);
  expect((await send(oh, await csrf(), disable)).status()).toBe(200);
  await save.click();
  await expect(panel.getByRole("alert")).toHaveText(/limit changed/);
  await expect(unlimited).toBeChecked();
  expect(await read()).toMatchObject({ revision: "2", limit_micro_usd: null, accounted_micro_usd: "310000" });

  const writes = [];
  const lostAck = async route => {
    if (route.request().method() !== "POST") return route.continue();
    writes.push(route.request().postDataJSON());
    if (writes.length !== 1) return route.continue();
    const committed = await route.fetch();
    expect(committed.status()).toBe(200);
    await committed.dispose();
    await route.abort("failed");
  };
  await owner.route(endpoint, lostAck);
  try {
    await monthly.check();
    await amount.fill("0.93");
    await save.click();
    await expect(panel.getByRole("alert")).toHaveText(/Could not confirm the change/);
    await save.click();
    await expect(save).toBeDisabled();
  } finally { await owner.unroute(endpoint, lostAck); }
  expect(writes).toHaveLength(2);
  expect(writes[0]).toMatchObject({ expected_revision: "2", limit_micro_usd: "930000" });
  expect(writes[1]).toEqual(writes[0]);
  expect((await send(oh, await csrf(), { ...writes[0], limit_micro_usd: "0" })).status()).toBe(409);
  expect(await read()).toMatchObject({ revision: "3", limit_micro_usd: "930000", remaining_micro_usd: "620000" });
  for (const theme of ["light", "dark"]) {
    await owner.emulateMedia({ colorScheme: theme, reducedMotion: "reduce" });
    await owner.reload();
    await expect(owner.locator(".hosted-entry")).toHaveAttribute("data-theme", theme);
    await expand();
    await expect(amount).toHaveValue("0.93");
    for (const width of [320, 1024]) {
      await owner.setViewportSize({ width, height: 760 });
      expect(await panel.evaluate(el => document.documentElement.scrollWidth <= innerWidth &&
        [...el.querySelectorAll("summary, dd, label, input, button")].every(node => {
          const box = node.getBoundingClientRect();
          return box.left >= 0 && box.right <= innerWidth;
        }))).toBe(true);
      expect(await panel.evaluate(el => {
        const [input, button] = el.querySelectorAll(".source-purchase-action :is(input, button)");
        const top = [...el.querySelectorAll("dd")].map(node => node.getBoundingClientRect().top);
        return Math.abs(input.getBoundingClientRect().bottom - button.getBoundingClientRect().bottom) < 1
          && top.every(value => Math.abs(value - top[0]) < 1);
      })).toBe(true);
      if (process.env.RELAY_TEST_CATALOG_CAPTURE)
        await owner.screenshot({ path: join(process.env.RELAY_TEST_CATALOG_CAPTURE, `purchase-limit-${width}-${theme}.png`), fullPage: true });
    }
  }
  issue("limit-set");
  await expect.poll(() => existsSync(join(exchange, "receipts-closed")), { timeout: 110000 }).toBe(true);
  expect(await read()).toMatchObject({ revision: "3", accounted_micro_usd: "730000", outstanding_micro_usd: "0", remaining_micro_usd: "200000" });
}
