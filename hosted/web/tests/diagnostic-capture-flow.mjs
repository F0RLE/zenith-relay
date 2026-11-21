import { expect } from "@playwright/test";
import { readFixture } from "../../test-support/files.mjs";
import { join } from "node:path";
import { forkOwnerSession, ownerAssertion } from "./owner-session.mjs";

export async function checkCapture({ owner, a, b, origin, api, ownerRead, issue, wait, exchange }) {
  await wait("capture-request");
  const target = readFixture(exchange, "capture-request");
  const path = api + "admin/operations/diagnostic-sessions";
  const options = path + "/options";
  const verify = data => `${path}/${data.ceremony_id}/verify`;
  const http = owner.context().request;
  const previousPreferences = await owner.evaluate(() => [localStorage.getItem("relay-hosted-theme"),
    localStorage.getItem("relay-hosted-locale")]);
  await owner.bringToFront();
  const previous = await forkOwnerSession(owner, origin);
  const csrf = (await ownerRead()).csrf_token;
  const command = { request_id: target, operation_id: crypto.randomUUID(), reason: "request_timing", seconds: 60 };
  const post = (url, data, { client = http, token = csrf, from = origin } = {}) =>
    client.post(url, { data, headers: { Origin: from, "X-Relay-CSRF": token } });
  const send = body => post(path, body);
  const prepare = async body => {
    const r = await post(options, body);
    expect(r.status()).toBe(200);
    return (await r.json()).data;
  };
  try {
    expect((await fetch(path)).status).toBe(401);
    for (const client of [a.context().request, b.context().request]) {
      expect((await client.get(path)).status()).toBe(401);
      expect((await post(path, command, { client })).status()).toBe(401);
      expect((await post(options, command, { client })).status()).toBe(401);
    }
    expect((await http.get(path + "?raw=true")).status()).toBe(400);
    for (const url of [path, options]) {
      expect((await post(url, command, { token: "00".repeat(32) })).status()).toBe(403);
      expect((await post(url, command, { from: "https://other.example" })).status()).toBe(403);
      expect((await post(url, { ...command, max_events: 128 })).status()).toBe(400);
    }
    const withoutProof = await send(command);
    expect(withoutProof.status()).toBe(403);
    expect((await withoutProof.json()).error.code).toBe("fresh_passkey_required");
    const original = await prepare(command);
    const proof = await ownerAssertion(owner, original.options);
    const earlier = await previous.request.get(api + "admin/overview");
    expect(earlier.status()).toBe(200);
    const foreign = await post(verify(original), proof, {
      client: previous.request, token: (await earlier.json()).data.csrf_token,
    });
    expect(foreign.status()).toBe(409);
    expect((await foreign.json()).error.code).toBe("passkey_ceremony_expired");
    const changed = await prepare({ ...command, reason: "pending_outcome", seconds: 30 });
    const wrong = await post(verify(changed), proof);
    expect(wrong.status()).toBe(400);
    expect((await wrong.json()).error.code).toBe("invalid_credential");
    expect((await (await http.get(path)).json()).data.items).toEqual([]);
  } finally { await previous.close(); }

  await owner.setViewportSize({ width: 1120, height: 760 });
  await owner.goto(origin + "/admin/operations");
  const panel = owner.getByRole("region", { name: "Temporary diagnostics" });
  const start = panel.getByRole("button", { name: "Start capture" });
  const stop = panel.getByRole("button", { name: "Stop capture" });
  await expect(start).toBeVisible();
  const screenshot = async (name) => {
    if (process.env.RELAY_TEST_HISTORY_UI_CAPTURE)
      await panel.screenshot({ path: join(process.env.RELAY_TEST_HISTORY_UI_CAPTURE, `capture-${name}.png`) });
  };
  await panel.getByLabel("Request ID", { exact: true }).fill(target);
  await panel.getByLabel("Duration (seconds)", { exact: true }).fill("60");
  expect(await fitsFor(panel)).toBe(true);
  await screenshot("ready-1120-en");
  await owner.setViewportSize({ width: 320, height: 760 });
  expect(await fitsFor(panel)).toBe(true);
  await screenshot("ready-320-en");
  await owner.setViewportSize({ width: 1120, height: 760 });

  let ack;
  const plan = owner.waitForRequest(r => r.url() === options);
  const verifyRoute = `${path}/*/verify`;
  const committed = async route => {
    const response = await route.fetch();
    expect(response.status()).toBe(200);
    ack = (await response.json()).data.session;
    await response.dispose();
    const replay = await post(route.request().url(), route.request().postDataJSON());
    expect(replay.status()).toBe(409);
    expect((await replay.json()).error.code).toBe("passkey_ceremony_expired");
    await route.abort("failed");
  };
  await owner.route(verifyRoute, committed, { times: 1 });
  try {
    await start.focus();
    await start.press("Enter");
    await expect(stop).toBeVisible();
    await expect(panel.getByRole("alert")).toHaveCount(0);
  } finally { await owner.unroute(verifyRoute, committed); }
  const confirmed = (await plan).postDataJSON();
  expect((await ownerRead()).csrf_token).toBe(csrf);
  const catalog = await http.get(path);
  expect(catalog.headers()["cache-control"]).toBe("no-store");
  const [active] = (await catalog.json()).data.items;
  const detailUrl = path + "/" + active.id;
  expect(active).toEqual(ack);
  expect(active).toMatchObject({ state: "active", request_id: target, reason: "request_timing",
    operation_id: confirmed.operation_id, consumed: { events: 0, bytes: 0 },
    limits: { events: 16, bytes: 4096, events_per_second: 16 } });
  expect(active.expires_at_ms - active.started_at_ms).toBe(60000);
  for (const url of [path, options]) {
    const repeated = await post(url, confirmed);
    expect(repeated.status()).toBe(200);
    expect((await repeated.json()).data.session).toEqual(active);
  }
  const conflict = await send({ ...confirmed, seconds: 61 });
  expect(conflict.status()).toBe(409);
  expect((await conflict.json()).error.code).toBe("capture_conflict");
  expect((await send(command)).status()).toBe(409);
  expect((await b.context().request.get(detailUrl)).status()).toBe(401);
  expect((await a.context().request.post(detailUrl + "/stop", { headers: { Origin: origin, "X-Relay-CSRF": csrf } })).status()).toBe(401);
  expect((await http.post(detailUrl + "/stop", { headers: { Origin: origin } })).status()).toBe(403);
  issue("capture-started");
  await wait("capture-finished");
  await expect(panel.getByText("Accounting settled", { exact: true })).toBeVisible();
  const detail = (await (await http.get(detailUrl)).json()).data;
  expect(detail.coverage).toBe("partial");
  expect(detail.events.map(row => row.code)).toEqual([
    "upstream_response", "terminal_observed", "evidence_committed", "accounting_settled",
  ]);
  for (const [index, row] of detail.events.entries())
    expect(row).toEqual({ sequence: index + 1, code: expect.any(String), occurred_at_ms: expect.any(Number) });
  expect(JSON.stringify(detail)).not.toMatch(/resp_fixture|fixture-loopback-only|xxxxxxxxxxxx|proof_tag|csrf_token/);
  await screenshot("active-1120-en");
  await stop.click();
  await expect(panel.getByText("Stopped", { exact: true })).toBeVisible();
  await expect(start).toHaveCount(0);
  const newCapture = panel.getByRole("button", { name: "New capture" });
  await newCapture.click();
  await expect(panel.getByLabel("Request ID", { exact: true })).toBeFocused();
  await panel.getByRole("button", { name: "Cancel", exact: true }).click();
  await expect(newCapture).toBeFocused();
  const stopped = await http.post(detailUrl + "/stop", { headers: { Origin: origin, "X-Relay-CSRF": csrf } });
  expect(stopped.status()).toBe(200);
  expect((await stopped.json()).data.session).toMatchObject({ state: "stopped", expires_at_ms: active.expires_at_ms,
    consumed: { events: 4 } });
  const finished = await send(command);
  expect(finished.status()).toBe(409);
  expect((await finished.json()).error.code).toBe("capture_request_unavailable");

  for (const [theme, locale, title, state] of [
    ["dark", "ru", "Временная диагностика", "Остановлено"],
    ["light", "zh-CN", "临时诊断", "已停止"],
    ["light", "en", "Temporary diagnostics", "Stopped"],
  ]) {
    await owner.setViewportSize({ width: 320, height: 760 });
    await owner.evaluate(([theme, locale]) => {
      localStorage.setItem("relay-hosted-theme", theme);
      localStorage.setItem("relay-hosted-locale", locale);
    }, [theme, locale]);
    await owner.reload();
    const section = owner.getByRole("region", { name: title });
    await expect(section.getByText(state, { exact: true })).toBeVisible();
    await expect(section.locator(".capture-events li")).toHaveCount(4);
    expect(await fitsFor(section)).toBe(true);
    if (process.env.RELAY_TEST_HISTORY_UI_CAPTURE)
      await section.screenshot({ path: join(process.env.RELAY_TEST_HISTORY_UI_CAPTURE, `capture-stopped-320-${locale}-${theme}.png`) });
  }
  await owner.setViewportSize({ width: 1120, height: 760 });
  await owner.evaluate(values => {
    ["relay-hosted-theme", "relay-hosted-locale"].forEach((key, i) =>
      values[i] === null ? localStorage.removeItem(key) : localStorage.setItem(key, values[i]));
  }, previousPreferences);
  issue("capture-checked");
}

function fitsFor(panel) {
  return panel.evaluate(el => document.documentElement.scrollWidth <= innerWidth &&
    [...el.querySelectorAll("button, select, input, code, time")].every(node => {
      const rect = node.getBoundingClientRect();
      return rect.left >= 0 && rect.right <= innerWidth;
    }));
}
