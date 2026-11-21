import { expect } from "@playwright/test";
import { join } from "node:path";
import { writeFixture } from "../../test-support/files.mjs";

export async function checkWebModelQualification({ page, guest, api, exchange, source, row, proof, declared, redacted }) {
  const resource = `${api}admin/connections/${source}/checks`;
  const options = `${resource}/options`;
  const command = model => ({
    operation_id: crypto.randomUUID(), expected_revision: "0",
    inputs: { model, max_input_bytes: 4000, max_output_tokens: 64, tokens_per_byte: 1, overhead_tokens: 0 },
  });
  const read = async (op, base = resource) => {
    const reply = await page.request.get(op ? `${base}?operation=${op}` : base);
    expect(reply.status()).toBe(200);
    expect(reply.headers()["cache-control"]).toBe("no-store");
    const { data } = await reply.json();
    redacted(data);
    return data.check;
  };
  const approve = async input => {
    const bind = await proof.prepare(resource, input);
    const reply = await proof.post(proof.path(bind), await proof.assertion(bind));
    expect(reply.status()).toBe(200);
    expect((await reply.json()).data).toEqual({ applied: true, operation_id: input.operation_id });
  };
  const fixture = command(declared);
  expect((await guest.request.get(resource)).status()).toBe(401);
  expect((await proof.post(options, fixture, { client: guest.request })).status()).toBe(401);
  expect((await proof.post(options, fixture, { token: "0".repeat(64) })).status()).toBe(403);
  expect((await proof.post(options, { ...fixture, endpoint: "http://127.0.0.1:1/v1" })).status()).toBe(400);
  expect((await proof.post(options, { ...fixture, inputs: { ...fixture.inputs, max_input_bytes: 0 } })).status()).toBe(400);
  expect((await proof.post(options, { ...fixture, expected_revision: "42" })).status()).toBe(409);
  await proof.prepare(resource, fixture);
  expect(await read()).toBeNull(); // A challenge is not permission to contact the provider.

  const abandoned = command("abandoned-check");
  await approve(abandoned);
  expect(await read(abandoned.operation_id)).toMatchObject({ state: "queued" });
  writeFixture(exchange, "simulate-check-interruption", abandoned.operation_id);
  await expect.poll(async () => (await read(abandoned.operation_id)).state).toBe("unconfirmed");
  expect(await read(abandoned.operation_id, `${api}admin/connections/${crypto.randomUUID()}/checks`)).toBeNull();

  await row.getByRole("button", { name: "Awaiting verification" }).click();
  const dialog = page.getByRole("dialog", { name: "Models", exact: true });
  await dialog.getByRole("button", { name: "Check model", exact: true }).press("Enter");
  const run = dialog.getByRole("button", { name: "Run check", exact: true });
  await expect(run).toBeDisabled();
  await expect(dialog.getByLabel("Model ID")).toHaveValue(declared);
  for (const [label, value] of [["Max input bytes", "4000"], ["Max output tokens", "64"],
    ["Input tokens per byte", "1"], ["Extra input tokens", "0"]])
    await dialog.getByLabel(label, { exact: true }).fill(value);
  await expect(run).toBeEnabled();
  await run.scrollIntoViewIfNeeded();
  expect(await dialog.evaluate(element => {
    const { left, right, top, bottom } = element.getBoundingClientRect();
    return left >= 0 && right <= innerWidth && top >= 0 && bottom <= innerHeight
      && [...element.querySelectorAll("input, button[type=submit]")].every(control => {
        const r = control.getBoundingClientRect();
        return r.width > 0 && r.left >= left && r.right <= right && r.top >= top && r.bottom <= bottom;
      });
  })).toBe(true);
  if (process.env.RELAY_TEST_SOURCE_INTAKE_CAPTURE)
    await page.screenshot({ path: join(process.env.RELAY_TEST_SOURCE_INTAKE_CAPTURE, "model-check-320-dark.png") });
  const submitted = page.waitForRequest(r => r.url() === options && r.method() === "POST");
  let approval;
  await page.route(proof.verification + "*/verify", async route => {
    const reply = await route.fetch();
    expect(reply.status()).toBe(200);
    approval = (await reply.json()).data;
    await reply.dispose();
    await route.abort("failed");
  }, { times: 1 });
  await run.click();
  const sent = (await submitted).postDataJSON();
  const result = dialog.locator(".owner-model-check-receipt[role=status]");
  await expect(result).toContainText("Queued");
  expect(approval).toEqual({ applied: true, operation_id: sent.operation_id });
  expect(sent.inputs).toEqual(fixture.inputs);
  expect(await read(sent.operation_id)).toEqual({ operation_id: sent.operation_id, model: declared, state: "queued" });
  expect((await (await proof.post(options, sent)).json()).data).toEqual(approval);
  expect((await proof.post(options, { ...sent, inputs: { ...sent.inputs, max_output_tokens: 65 } })).status()).toBe(409);
  writeFixture(exchange, "start-model-worker");
  await expect(result).toContainText("Check passed", { timeout: 15000 });
  await page.keyboard.press("Escape");
  await expect(row.getByRole("button", { name: /Verified models/ })).toBeVisible();
  await expect(row.getByText("Outside pool", { exact: true })).toBeVisible();
  const rules = (await (await page.request.get(`${api}admin/pools/model-rules`)).json()).data.items;
  expect(rules).toEqual([expect.objectContaining({ source_id: source, model: declared, admitted: false, effective: null })]);
  expect((await proof.post(options, command(declared))).status()).toBe(409);

  await page.setViewportSize({ width: 1024, height: 760 });
  await page.emulateMedia({ colorScheme: "light" });
  await page.reload();
  await row.getByRole("button", { name: /Verified models/ }).click();
  await expect(result).toContainText("Check passed");
  if (process.env.RELAY_TEST_SOURCE_INTAKE_CAPTURE)
    await page.screenshot({ path: join(process.env.RELAY_TEST_SOURCE_INTAKE_CAPTURE, "model-check-1024-light.png") });
  const missing = command("not-in-upstream-catalog");
  await approve(missing);
  await expect.poll(async () => (await read(missing.operation_id)).state, { timeout: 15000 }).toBe("not_verified");
  await page.keyboard.press("Escape");
  await expect(row.getByRole("button", { name: /Verified models/ })).toHaveText(/1/);

  // A passkey response cannot authorize an old binding, even after resuming it.
  const stale = command("check-before-pause");
  const staleChallenge = await proof.prepare(resource, stale);
  for (const [action, expected_revision] of [["pause", "0"], ["resume", "1"]]) {
    const lifecycle = await proof.prepare(`${api}admin/connections/${source}/serving`, {
      action, expected_revision, operation_id: crypto.randomUUID(),
    });
    expect((await proof.post(proof.path(lifecycle), await proof.assertion(lifecycle))).status()).toBe(200);
  }
  const refused = await proof.post(proof.path(staleChallenge), await proof.assertion(staleChallenge));
  expect(refused.status()).toBe(409);
  expect((await refused.json()).error.code).toBe("model_check_changed");
  expect(await read(stale.operation_id)).toBeNull();
}
