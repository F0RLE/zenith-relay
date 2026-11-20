import { expect, test } from "../bun-playwright";
import { installTauriMock } from "./tauri-mock";

test("Desktop reviews one destination before reading, pages through its sources and forgets only on confirmation", async ({page}, info) => {
  await installTauriMock(page, {mode:"hosted", locale:"en", populated:true});
  await page.goto("/");
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
      if (!["prepare_hosted_device","read_hosted_connections","forget_hosted_device"].includes(cmd)) return native(cmd,args);
      host.deviceCalls.push(cmd);
      if (args?.invitation !== data.invitation) throw "invalid_device_invitation";
      if (cmd === "prepare_hosted_device") return data;
      if (cmd === "forget_hosted_device") return;
      if (++reads === 3) throw "device_not_authorized";
      const second = reads === 2;
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
  await pane.getByRole("button",{name:"View server connections"}).click();
  await expect(pane.getByText("First source")).toBeVisible();
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
  const forget = pane.getByRole("button",{name:"Forget on this computer"});
  await forget.click();
  await pane.getByRole("button",{name:"Cancel",exact:true}).click();
  expect(await calls()).not.toContain("forget_hosted_device");
  await forget.click();
  await pane.getByRole("button",{name:"Confirm local removal"}).click();
  await expect(review).toBeDisabled();
  expect(await calls()).toEqual(["prepare_hosted_device","prepare_hosted_device",...Array(3).fill("read_hosted_connections"),"forget_hosted_device"]);
});

test("API source copy previews the effective proxy and separates approval, confirmation and recovery", async ({ page }, info) => {
  await installTauriMock(page, { mode: "hosted", locale: "en", populated: true });
  await page.goto("/");
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
