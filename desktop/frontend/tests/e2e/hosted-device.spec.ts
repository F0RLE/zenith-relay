import { expect, test, type Page } from "../bun-playwright";
import { installTauriMock } from "./tauri-mock";

const navigate = (page: Page, name: string) => page.locator(".relay-sidebar").getByRole("button", { name, exact: true }).click();

test("Desktop reviews one destination before reading, pages through its sources and forgets only on confirmation", async ({page}, info) => {
  await installTauriMock(page, {mode:"hosted", locale:"en", populated:true});
  await page.goto("/");
  await page.getByRole("button", { name: "Open settings", exact: true }).click();
  const pane = page.locator(".hosted-device-pane");
  const review = pane.getByRole("button", {name:"Review server"});
  await expect(review).toBeDisabled();
  const origin = "https://operator.example", installation = "00000000-0000-4000-8000-000000000001";
  const device = "00000000-0000-4000-8000-000000000002";
  const invitation = `${origin}/admin/connections#relay-device=${installation}.${device}`;
  const approval_code = `review1.${device}.${"ab".repeat(32)}`;
  await page.evaluate(data => {
    const host = window as unknown as {
      __TAURI_INTERNALS__: {invoke: (cmd:string,args?:Record<string,unknown>)=>Promise<unknown>};
      deviceCalls: string[];
    };
    host.deviceCalls = [];
    const native = host.__TAURI_INTERNALS__.invoke.bind(host.__TAURI_INTERNALS__);
    let reads = 0;
    host.__TAURI_INTERNALS__.invoke = async (cmd,args) => {
      if (!["prepare_hosted_device","read_hosted_access","read_hosted_connections","forget_hosted_device"].includes(cmd)) return native(cmd,args);
      host.deviceCalls.push(cmd);
      if (args?.invitation !== data.invitation) throw "invalid_device_invitation";
      if (cmd === "prepare_hosted_device") return data;
      if (cmd === "read_hosted_access") return null; // pre-scope server
      if (cmd === "forget_hosted_device") return;
      if (++reads === 4) throw "device_not_authorized";
      const second = reads === 3;
      if (args.after !== (second ? data.destination.device : null)) throw "invalid_device_cursor";
      return {scope:"connections.read",device_id:data.destination.device,installation_id:data.destination.installation,
        items:[{id:data.destination.device, label:second?"Later source":"First source", proxy_bound:!second,
          credential_state:"stored",verified_models:second?[]:["sample-model"]}],
        next_after:second?null:data.destination.device, expires_at_s:Math.floor(Date.now()/1000)+86400};
    };
  }, {destination:{origin, installation, device}, invitation, approval_code});
  await pane.getByRole("textbox").fill("not an invitation");
  await review.click();
  await expect(pane.getByRole("alert")).toContainText("complete invitation");
  await pane.getByRole("textbox").fill(invitation);
  await review.click();
  await expect(pane.getByRole("textbox")).toHaveValue(approval_code);
  await expect(pane.getByText(origin,{exact:true})).toBeVisible();
  const calls = () => page.evaluate(() => (window as unknown as {deviceCalls:string[]}).deviceCalls);
  expect(await calls()).toEqual(["prepare_hosted_device","prepare_hosted_device"]);
  await pane.getByRole("button",{name:"Check access"}).click();
  await expect(pane.getByRole("status")).toContainText("Access until");
  await navigate(page, "Usage");
  await expect(pane.getByText("This device was not granted access to this section.")).toBeVisible();
  expect(await calls()).toEqual(["prepare_hosted_device", "prepare_hosted_device", "read_hosted_access", "read_hosted_connections"]);
  await navigate(page, "Connections");
  await expect(pane.getByText("First source")).toBeVisible();
  await expect(pane.getByRole("tablist")).toHaveCount(0);
  await expect(pane.getByText("Proxy assigned")).toBeVisible();
  for (const width of [320,1024]) {
    await page.setViewportSize({width,height:760});
    expect(await pane.evaluate(n=>n.scrollWidth<=n.clientWidth)).toBe(true);
  }
  await pane.screenshot({path:info.outputPath("device-sources.png")});
  await pane.getByRole("button",{name:"Next page"}).click();
  await expect(pane.getByText("Later source")).toBeVisible();
  await pane.getByRole("button",{name:"Back",exact:true}).click();
  await expect(pane.getByRole("alert")).toContainText("no access");
  await expect(pane.getByText("Later source")).toHaveCount(0);
  await navigate(page, "Settings");
  const forget = pane.getByRole("button",{name:"Forget on this computer"});
  await forget.click();
  await pane.getByRole("button",{name:"Cancel",exact:true}).click();
  expect(await calls()).not.toContain("forget_hosted_device");
  await forget.click();
  await pane.getByRole("button",{name:"Confirm local removal"}).click();
  await expect(review).toBeDisabled();
  expect(await calls()).toEqual(["prepare_hosted_device","prepare_hosted_device","read_hosted_access",...Array(4).fill("read_hosted_connections"),"forget_hosted_device"]);
});

test("Hosted sidebar reads the selected server view and clears data when access is revoked", async ({ page }, info) => {
  await installTauriMock(page, { mode: "hosted", locale: "en" });
  await page.goto("/");
  await navigate(page, "Settings");
  const pane = page.locator(".hosted-device-pane");
  const target = { origin: "https://operator.example", installation: crypto.randomUUID(), device: crypto.randomUUID() };
  const invitation = `${target.origin}/admin/connections#relay-device=${target.installation}.${target.device}`;
  await page.evaluate(({target, invitation}) => {
    const host = window as unknown as {
      __TAURI_INTERNALS__: { invoke: (cmd: string, args?: Record<string, unknown>) => Promise<unknown> };
      hostedReads: Array<{cmd: string; args: unknown}>;
      hostedRevoked: boolean;
    };
    host.hostedReads = [];
    let overviewReads = 0;
    const invoke = host.__TAURI_INTERNALS__.invoke.bind(host.__TAURI_INTERNALS__);
    const anchor = Date.now(), expires_at_s = Math.floor(anchor / 1000) + 3600;
    const request_id = "3578de23-4f5a-42df-820d-37697e991123";
    const meta = { installation_id: target.installation, device_id: target.device, expires_at_s };
    host.__TAURI_INTERNALS__.invoke = async (cmd, args) => {
      if (cmd === "prepare_hosted_device") return { destination: target, invitation, approval_code: "synthetic review" };
      if (!cmd.startsWith("read_hosted_")) return invoke(cmd, args);
      if (args?.invitation !== invitation) throw "device_target_changed";
      host.hostedReads.push({ cmd, args });
      if (host.hostedRevoked) throw "device_not_authorized";
      if (cmd === "read_hosted_access") return { ...meta, result: ["overview.read", "usage.read"] };
      if (cmd === "read_hosted_connections") return { ...meta, scope: "connections.read", items: [], next_after: null };
      if (cmd === "read_hosted_overview") return { ...meta, result: {
        hours: args?.hours, as_of_ms: anchor, requests: "35", errors: "1", total_tokens: "12004",
        token_samples: "33", partial_tokens: "1", active_requests: "3", active_attempts: "2",
        metrics: ++overviewReads === 1 ? { pool: { members: "4", ready: "2", busy: "1" },
          generation: { samples: "12", tokens_per_second: 42.5 } } : { pool: null, generation: null },
      } };
      const f = args?.filter as { model: string | null; execution: string | null; hours: number; before: string | null; as_of_ms: number | null };
      const items = f.model ? [{ request_id, started_at_ms: anchor - 60_000, model: f.model, execution: "failed",
        diagnostic_code: "upstream_status", tokens: null, partial_tokens: false }] : [];
      return { ...meta, result: { ...f, as_of_ms: anchor, next_before: null, items } };
    };
  }, {target, invitation});
  await pane.getByRole("textbox").fill(invitation);
  await pane.getByRole("button", { name: "Review server" }).click();
  await pane.getByRole("button", { name: "Check access" }).click();
  await expect(pane.getByRole("status")).toContainText("Access until");
  await navigate(page, "Connections");
  await expect(pane.getByText("No saved connections")).toBeVisible();
  await navigate(page, "Overview");
  await expect(page.locator(".sidebar-main-nav button[aria-current=page]")).toHaveText("Overview");
  await expect(pane.getByText("12,004")).toBeVisible();
  await expect(pane.getByText("Measured for 33 of 35 requests · 1 partial")).toBeVisible();
  await expect(pane.getByText("2 / 4", { exact: true })).toBeVisible();
  await expect(pane.getByText("Busy: 1", { exact: true })).toBeVisible();
  await expect(pane.getByText("42.5 tok/s", { exact: true })).toBeVisible();
  const coverage = pane.getByText("Measured streams: 12", { exact: true });
  await expect(coverage).toBeVisible();
  for (const width of [1024, 320]) {
    await page.setViewportSize({ width, height: 760 });
    await expect.poll(() => pane.evaluate(n => Math.max(0, n.scrollWidth - n.clientWidth)),
      { message: `Hosted overview overflow at ${width}px` }).toBe(0);
    await coverage.scrollIntoViewIfNeeded();
    await expect(coverage).toBeInViewport({ ratio: 1 });
    await page.screenshot({ path: info.outputPath(`hosted-overview-${width}.png`) });
  }
  const refresh = page.locator(".relay-page-header").getByRole("button", { name: "Refresh", exact: true });
  await refresh.click();
  await expect(pane.locator(".hosted-metrics dd").getByText("—", { exact: true })).toHaveCount(2);
  await expect(pane.getByText("42.5 tok/s", { exact: true })).toHaveCount(0);
  await expect(pane.getByText("Busy: 1", { exact: true })).toHaveCount(0);
  await navigate(page, "Help");
  await expect(page.getByRole("heading", { name: "Help", exact: true })).toBeVisible();
  await navigate(page, "Usage");
  await expect(pane.getByText("No requests match these filters.")).toBeVisible();
  await pane.getByRole("textbox", { name: "Model", exact: true }).fill("fixture-text");
  await pane.getByRole("button", { name: "Search", exact: true }).click();
  await pane.getByText("fixture-text", { exact: true }).click();
  await expect(pane.getByText("upstream_status", { exact: true })).toBeVisible();
  for (const width of [1024, 320]) {
    await page.setViewportSize({ width, height: 760 });
    await pane.screenshot({ path: info.outputPath(`hosted-usage-${width}.png`) });
    const layout = await pane.evaluate(n => {
      const right = n.getBoundingClientRect().right;
      return {
        overflow: n.scrollWidth - n.clientWidth,
        clipped: [...n.querySelectorAll("*")].filter(e => e.getBoundingClientRect().right > right + 1)
          .map(e => `${e.tagName}.${e.getAttribute("class") ?? ""}`),
      };
    });
    expect(layout, `Hosted usage at ${width}px`).toEqual({ overflow: 0, clipped: [] });
  }
  const reads = await page.evaluate(() => (window as unknown as { hostedReads: Array<{cmd:string;args:unknown}> }).hostedReads);
  expect(reads.map(r => r.cmd)).toEqual(["read_hosted_access", "read_hosted_connections", "read_hosted_overview", "read_hosted_overview", "read_hosted_usage", "read_hosted_usage"]);
  expect(reads.at(-1)?.args).toEqual({ invitation, filter: { hours: 24, model: "fixture-text", execution: null, before: null, as_of_ms: null } });
  await page.evaluate(() => { (window as unknown as { hostedRevoked: boolean }).hostedRevoked = true; });
  await refresh.click();
  await expect(pane.getByRole("alert")).toContainText("no access");
  await expect(pane.getByText("fixture-text", { exact: true })).toHaveCount(0);
  await expect(pane.getByRole("tablist")).toHaveCount(0);
  await navigate(page, "Overview");
  await expect(pane.getByRole("alert")).toContainText("no access");
  await expect(pane.getByText("12,004")).toHaveCount(0);
  const afterRevocation = await page.evaluate(() => (window as unknown as { hostedReads: Array<{cmd:string}> }).hostedReads.map(r => r.cmd));
  expect(afterRevocation).toEqual([...reads.map(r => r.cmd), "read_hosted_usage"]);
});

test("Desktop prepares pool changes without claiming membership until the server refreshes", async ({ page }, info) => {
  await installTauriMock(page, { mode: "hosted", locale: "en" });
  await page.goto("/");
  await navigate(page, "Settings");
  const origin = "http://localhost:1443";
  const installation = crypto.randomUUID(), device = crypto.randomUUID(), source = crypto.randomUUID(), operation_id = crypto.randomUUID();
  const invitation = `${origin}/admin/connections#relay-device=${installation}.${device}`;
  const revision = "ac".repeat(32);
  await page.evaluate(({ origin, installation, device, source, operation_id, invitation, revision }) => {
    const host = window as unknown as {
      __TAURI_INTERNALS__: { invoke: (cmd: string, args?: Record<string, unknown>) => Promise<unknown> };
      poolMode: "reader" | "writer" | "included" | "stale";
      changed: unknown[];
    };
    const invoke = host.__TAURI_INTERNALS__.invoke.bind(host.__TAURI_INTERNALS__);
    host.poolMode = "reader";
    host.changed = [];
    const expires_at_s = Math.floor(Date.now() / 1000) + 86400;
    const meta = { installation_id: installation, device_id: device, expires_at_s };
    host.__TAURI_INTERNALS__.invoke = async (cmd, args) => {
      if (cmd === "prepare_hosted_device") return { invitation, destination: { origin, installation, device }, approval_code: "synthetic review" };
      if (!["read_hosted_access", "read_hosted_connections", "prepare_hosted_pool_change"].includes(cmd)) return invoke(cmd, args);
      if (args?.invitation !== invitation) throw "device_target_changed";
      if (cmd === "read_hosted_access") return { ...meta, result: [] };
      if (cmd === "prepare_hosted_pool_change") {
        host.changed.push(args.change);
        if (host.poolMode === "stale") throw "desktop_pool_changed";
        return { operation_id, source_id: source, action: "include", expires_at_s: Math.floor(Date.now() / 1000) + 300 };
      }
      const in_pool = host.poolMode === "included", pool_access = host.poolMode !== "reader";
      return { ...meta, scope: "connections.read", next_after: null, pool_access, items: [{
        id: source, label: "Checked source", credential_state: "stored", proxy_bound: true, verified_models: ["fixture-text"],
        pool: { state: "enabled", in_pool, draining: false, action: pool_access ? (in_pool ? "exclude" : "include") : null,
          revision: pool_access ? revision : null },
      }] };
    };
  }, { origin, installation, device, source, operation_id, invitation, revision });
  const pane = page.locator(".hosted-device-pane");
  await pane.getByRole("textbox").fill(invitation);
  await pane.getByRole("button", { name: "Review server" }).click();
  await pane.getByRole("button", { name: "Check access" }).click();
  await expect(pane.getByRole("status")).toContainText("Access until");
  await navigate(page, "Connections");
  await expect(pane.getByText("Outside pool", { exact: true })).toBeVisible();
  await expect(pane.getByRole("button", { name: "Add to pool" })).toHaveCount(0);
  await navigate(page, "Pool");
  await expect(pane.getByText("No members or eligible connections on this page.", { exact: true })).toBeVisible();
  await expect(pane.getByText("Checked source", { exact: true })).toHaveCount(0);
  const refresh = page.locator(".relay-page-header").getByRole("button", { name: "Refresh", exact: true });
  await page.evaluate(() => Object.assign(window, { poolMode: "writer" }));
  await refresh.click();
  await pane.getByRole("button", { name: "Add to pool" }).click();
  const review = pane.getByRole("textbox", { name: /Confirm in Admin/ });
  await expect(review).toHaveValue(operation_id);
  await expect(review).toBeFocused();
  await expect(pane.getByText("Outside pool", { exact: true })).toBeVisible();
  await page.setViewportSize({ width: 320, height: 760 });
  expect(await pane.evaluate(n => n.scrollWidth <= n.clientWidth)).toBe(true);
  await pane.screenshot({ path: info.outputPath("desktop-pool-review-320.png") });
  await page.evaluate(() => Object.assign(window, { poolMode: "included" }));
  await refresh.click();
  await expect(review).toHaveCount(0);
  await expect(pane.getByText("In pool", { exact: true })).toBeVisible();
  await navigate(page, "Connections");
  await expect(pane.getByText("Checked source", { exact: true })).toBeVisible();
  await expect(pane.getByRole("button", { name: "Remove from pool" })).toHaveCount(0);
  await navigate(page, "Pool");
  await expect(pane.getByText("In pool", { exact: true })).toBeVisible();
  await page.evaluate(() => Object.assign(window, { poolMode: "stale" }));
  await pane.getByRole("button", { name: "Remove from pool" }).click();
  await expect(pane.getByRole("alert")).toHaveText("The connection changed. Refresh before trying again.");
  const changes = await page.evaluate(() => (window as unknown as { changed: unknown[] }).changed);
  expect(changes).toEqual(["include", "exclude"].map(action => ({ source_id: source, action, expected_revision: revision })));
});

test("API source copy previews the effective proxy and separates approval, confirmation and recovery", async ({ page }, info) => {
  await installTauriMock(page, { mode: "hosted", locale: "en", populated: true });
  await page.goto("/");
  await navigate(page, "Settings");
  const origin = "https://operator.example";
  const installation = "00000000-0000-4000-8000-000000000001";
  const device = "00000000-0000-4000-8000-000000000002";
  const operation = "00000000-0000-4000-8000-000000000003";
  const source = "00000000-0000-4000-8000-000000000004";
  const invitation = `${origin}/admin/connections#relay-device=${installation}.${device}`;
  await page.evaluate(({ origin, installation, device, operation, invitation, source }) => {
    const host = window as unknown as {
      __TAURI_INTERNALS__: { invoke: (cmd: string, args?: Record<string, unknown>) => Promise<unknown> };
      copyCalls: string[];
    };
    const native = host.__TAURI_INTERNALS__.invoke.bind(host.__TAURI_INTERNALS__);
    host.copyCalls = [];
    const review = {
      code: `copy1.${"ab".repeat(500)}`,
      plan: { origin, installation_id: installation, device_id: device, operation_id: operation,
        source_id: source, label: "Synthetic local source", endpoint: "https://api.example/v1",
        proxy: { endpoint: "http://proxy.example:8080", authenticated: true } },
    };
    let prepared = false, committed = false;
    host.__TAURI_INTERNALS__.invoke = async (cmd, args) => {
      if (cmd === "prepare_hosted_device") {
        return { invitation, destination: { origin, installation, device }, approval_code: "test-device-review" };
      }
      if (!cmd.includes("hosted_copy")) return native(cmd, args);
      host.copyCalls.push(cmd);
      if (args?.invitation !== invitation) throw "device_target_changed";
      if (cmd === "get_hosted_copy") return prepared ? review : null;
      if (cmd === "list_hosted_copy_sources") return [{ id: source, label: review.plan.label }];
      if (cmd === "prepare_hosted_copy" && args?.sourceId === source) {
        prepared = true;
        return review;
      }
      if (args?.operationId !== operation) throw "device_target_changed";
      if (cmd === "hosted_copy_status") return committed ? "committed" : "approved";
      if (cmd === "confirm_hosted_copy") {
        committed = true;
        throw "copy_result_unconfirmed";
      }
    };
  }, { origin, installation, device, operation, invitation, source });
  await page.getByRole("textbox", { name: /invitation/i }).fill(invitation);
  await page.getByRole("button", { name: "Review server" }).click();
  await expect(page.getByRole("textbox", { name: /approval code/ })).toHaveValue("test-device-review");
  await navigate(page, "Connections");
  const calls = () => page.evaluate(() => (window as unknown as { copyCalls: string[] }).copyCalls);
  expect(await calls()).toEqual([]);
  const panel = page.locator(".hosted-copy");
  await panel.locator("summary").click();
  await panel.getByRole("button", { name: "Local API source: Choose a source" }).click();
  await page.getByRole("option", { name: "Synthetic local source" }).click();
  await panel.getByRole("button", { name: "Review copy" }).click();
  await expect(panel.getByText("http://proxy.example:8080")).toBeVisible();
  await expect(panel.getByText("The local source stays unchanged.")).toBeVisible();
  await expect(panel.getByRole("button", { name: "Copy to server" })).toHaveCount(0);
  expect(await calls()).toEqual(["get_hosted_copy", "list_hosted_copy_sources", "prepare_hosted_copy"]);
  for (const width of [320, 1024]) {
    await page.setViewportSize({ width, height: 760 });
    expect(await panel.evaluate(n => n.scrollWidth <= n.clientWidth)).toBe(true);
    await panel.screenshot({ path: info.outputPath(`copy-${width}.png`) });
  }
  const check = panel.getByRole("button", { name: "Check server result" });
  await check.click();
  await expect(panel.getByRole("status")).toContainText("Approved");
  await panel.getByRole("button", { name: "Copy to server" }).click();
  await expect(panel.getByRole("alert")).toContainText("Result not confirmed");
  await check.click();
  await expect(panel.getByRole("status")).toContainText("Saved on the server outside the pool");
  expect((await calls()).filter(x => x === "confirm_hosted_copy")).toHaveLength(1);
});
