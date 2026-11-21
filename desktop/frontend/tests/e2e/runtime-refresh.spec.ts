import { expect, test, type Page } from "../bun-playwright";
import { emitTauriEvent, installTauriMock } from "./tauri-mock";
import { openGatewayApi } from "./operations/helpers";
import type { CandidateRuntimeSnapshot, RuntimeSnapshot } from "../../src/features/relay/api/types";

type HeldSnapshotWindow = {
  __TAURI_INTERNALS__: { invoke: (command: string, args?: unknown, options?: unknown) => Promise<unknown> };
  __HELD_SNAPSHOT_READY__?: boolean;
  __RELEASE_SNAPSHOT__?: () => void;
};

for (const surface of ["Pool", "Connections"] as const) {
  test(`${surface} shows an occupied image lane as busy, not unavailable`, async ({ page }) => {
    await installTauriMock(page, { locale: "en", populated: true, accountCount: 1, quotaAvailable: true, usageActive: false });
    await page.goto("/");
    const account = await page.evaluate(async () => {
      const testWindow = window as unknown as HeldSnapshotWindow & { __FINISH_IMAGE__?: () => void };
      const native = testWindow.__TAURI_INTERNALS__;
      const invoke = native.invoke;
      const snapshot = await invoke("get_local_runtime_state") as RuntimeSnapshot;
      const account = snapshot.accounts.find((item) => item.inPool && !item.routeInactive)!;
      Object.assign(account, { operationalStatus: "rotation", lastErrorCode: null, routingBlockReason: null, quotaRefreshStatus: "updated" });
      const candidate: CandidateRuntimeSnapshot = {
        candidateId: account.id, kind: "oauth_account", runtimeId: 1, activityRevision: 1,
        available: false, capacityWait: true, quotaWait: false, inFlight: 0,
        activeRequestCount: 1, activeModels: [{ model: "gpt-image-2.6-sunburst", requestCount: 1 }],
        dispatches: 0, halfOpen: false, lastUsedAtMs: null, nextRetryAtMs: null,
      };
      snapshot.gateway.routingOrder = [candidate];
      snapshot.gateway.poolRotation = { inventoryRevision: 1, preset: null,
        memberOrder: [`account:${account.id}`], memberStatuses: { [`account:${account.id}`]: "rotation" } };
      testWindow.__FINISH_IMAGE__ = () => Object.assign(candidate, {
        available: true, capacityWait: false, activityRevision: 2, activeRequestCount: 0, activeModels: [],
      });
      native.invoke = async (command, args, options) => {
        if (command === "get_local_runtime_state") return structuredClone(snapshot);
        if (command === "get_local_runtime_order") return structuredClone({
          routingOrder: snapshot.gateway.routingOrder, poolRotation: snapshot.gateway.poolRotation,
        });
        return invoke(command, args, options);
      };
      return { id: account.id, label: account.label };
    });
    await emitTauriEvent(page, "zenith-state-changed", null);
    await page.getByRole("button", { name: surface, exact: true }).click();
    const card = page.locator(surface === "Pool" ? ".pool-member-card" : ".account-card").filter({ hasText: account.label });
    const status = card.locator(".relay-status-icon").first();
    await expect(status).toHaveAttribute("data-status", "warning");
    await expect(status).not.toHaveAttribute("aria-label", "Unavailable");
    await page.evaluate(() => (window as unknown as { __FINISH_IMAGE__: () => void }).__FINISH_IMAGE__());
    await emitTauriEvent(page, "zenith-runtime-activity", { candidateId: account.id, runtimeId: 1, revision: 2,
      inFlight: 0, activeRequestCount: 0, activeModels: [] });
    await expect(status).toHaveAttribute("data-status", "ready");
  });

  test(`${surface} recovers an unavailable card from a lightweight backend status`, async ({ page }) => {
    await installTauriMock(page, { locale: "en", populated: true, accountCount: 1, quotaAvailable: true, usageActive: false });
    await page.goto("/");
    const account = await page.evaluate(async () => {
      const testWindow = window as unknown as HeldSnapshotWindow & { __RECOVER_RUNTIME__?: () => void };
      const native = testWindow.__TAURI_INTERNALS__;
      const invoke = native.invoke.bind(native);
      const snapshot = await invoke("get_local_runtime_state") as RuntimeSnapshot;
      const account = snapshot.accounts.find((item) => item.inPool && !item.routeInactive)!;
      Object.assign(account, { operationalStatus: "unavailable", lastErrorCode: null, routingBlockReason: null, quotaRefreshStatus: "updated" });
      const candidate: CandidateRuntimeSnapshot = {
        candidateId: account.id, kind: "oauth_account", runtimeId: 1, activityRevision: 0,
        available: false, inFlight: 0, activeRequestCount: 0, activeModels: [],
        dispatches: 0, halfOpen: false, lastUsedAtMs: null, nextRetryAtMs: null,
      };
      snapshot.gateway.routingOrder = [candidate];
      snapshot.gateway.poolRotation = { inventoryRevision: 1, preset: null,
        memberOrder: [`account:${account.id}`], memberStatuses: { [`account:${account.id}`]: "unavailable" } };
      let recovered = false;
      testWindow.__RECOVER_RUNTIME__ = () => { recovered = true; };
      native.invoke = async (command, args, options) => {
        if (command === "get_local_runtime_state") return structuredClone(snapshot);
        if (command === "get_local_runtime_order") return {
          routingOrder: [{ ...candidate, available: recovered, activityRevision: recovered ? 2 : 0 }],
          poolRotation: { ...snapshot.gateway.poolRotation,
            memberStatuses: { [`account:${account.id}`]: recovered ? "rotation" : "unavailable" } },
        };
        return invoke(command, args, options);
      };
      return { id: account.id, label: account.label };
    });
    await emitTauriEvent(page, "zenith-state-changed", null);
    await page.getByRole("button", { name: surface, exact: true }).click();
    const card = page.locator(surface === "Pool" ? ".pool-member-card" : ".account-card").filter({ hasText: account.label });
    await expect(card.locator(".relay-status-icon").first()).toHaveAttribute("data-status", "error");
    await page.evaluate(() => (window as unknown as { __RECOVER_RUNTIME__: () => void }).__RECOVER_RUNTIME__());
    await emitTauriEvent(page, "zenith-runtime-activity", { candidateId: account.id, runtimeId: 1, revision: 1,
      inFlight: 0, activeRequestCount: 0, activeModels: [] });
    await expect(card.locator(".relay-status-icon").first()).toHaveAttribute("data-status", "ready");
    await expect(card.locator(".relay-status-icon").first()).toHaveAttribute("aria-label", "In rotation");
  });
}

test("periodic snapshots recover missed state notifications", async ({ page }) => {
  await page.clock.install();
  await installTauriMock(page, { locale: "en", populated: true });
  await page.goto("/");
  await expect(page.getByRole("button", { name: "Stop API", exact: true })).toBeVisible();
  await page.evaluate(async () => {
    await (window as unknown as HeldSnapshotWindow).__TAURI_INTERNALS__.invoke("stop_local_gateway");
  });
  // No state event: the bounded fallback must reread the backend anyway.
  await page.clock.fastForward(60_000);
  await expect(page.getByRole("button", { name: "Start API", exact: true })).toBeVisible();
});

for (const dirty of [false, true]) {
  test(`gateway snapshot changes refresh only a clean port field, dirty=${dirty}`, async ({ page }) => {
    await installTauriMock(page, { mode: "local", locale: "en", populated: true, gatewayRunning: false });
    await page.goto("/");
    await openGatewayApi(page);
    const port = page.locator("#gateway-api-port");
    await expect(port).toHaveValue("14998");
    if (dirty) await port.fill("18001");
    await page.evaluate(async () => {
      await (window as unknown as HeldSnapshotWindow).__TAURI_INTERNALS__.invoke("update_local_gateway_port", { port: 18002 });
    });
    // Access intentionally skips unsolicited full snapshots. A normal action
    // refreshes settings without navigating away from the unsaved port field.
    await page.getByRole("button", { name: /^Start/ }).click();
    await expect(page.locator(".gateway-api-address")).toHaveText("http://127.0.0.1:18002/v1");
    await expect(port).toHaveValue(dirty ? "18001" : "18002");
    if (dirty) {
      await port.press("Enter");
      await expect(page.locator(".gateway-api-address")).toHaveText("http://127.0.0.1:18001/v1");
      await expect(port).toHaveValue("18001");
    }
  });
}

test("source name, address and price drafts survive a background source refresh", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Connections", exact: true }).click();
  await page.getByRole("tab", { name: "Sources", exact: true }).click();
  await page.getByRole("button", { name: "Edit", exact: true }).click();
  const editor = page.getByRole("dialog", { name: "Edit source", exact: true });
  await editor.getByLabel("Name", { exact: true }).fill("Unsaved source name");
  await editor.getByLabel("API address", { exact: true }).fill("https://draft.invalid/v1");
  await editor.getByRole("tab", { name: "Pricing", exact: true }).click();
  await editor.locator("details > summary").first().click();
  const price = editor.getByRole("textbox", { name: "Input token price for gpt-5.4", exact: true });
  await price.fill("4.25");
  await page.evaluate(async () => {
    const invoke = (window as unknown as HeldSnapshotWindow).__TAURI_INTERNALS__.invoke;
    const runtime = await invoke("get_local_runtime_state") as { sources: Array<{ id: string; [key: string]: unknown }> };
    const source = runtime.sources[0]!;
    await invoke("update_local_source", { input: { ...source, sourceId: source.id,
      name: "Externally updated source", baseUrl: "https://external.invalid/v1", modelPriceOverrides: {} } });
  });
  await emitTauriEvent(page, "zenith-state-changed", null);
  await expect(editor.getByText("https://external.invalid/v1", { exact: true })).toBeVisible();
  await expect(price).toHaveValue("4.25");
  await editor.getByRole("tab", { name: "General", exact: true }).click();
  await expect(editor.getByLabel("Name", { exact: true })).toHaveValue("Unsaved source name");
  await expect(editor.getByLabel("API address", { exact: true })).toHaveValue("https://draft.invalid/v1");
});

async function holdNextSnapshot(page: Page) {
  await page.evaluate((command) => {
    const testWindow = window as unknown as HeldSnapshotWindow;
    const internals = testWindow.__TAURI_INTERNALS__;
    const invoke = internals.invoke.bind(internals);
    let hold = true;
    testWindow.__HELD_SNAPSHOT_READY__ = false;
    internals.invoke = async (name, args, options) => {
      if (name !== command || !hold) return invoke(name, args, options);
      hold = false;
      // Capture the state before a later mutation, then deliver it last.
      const snapshot = await invoke(name, args, options);
      return new Promise((resolve) => {
        testWindow.__RELEASE_SNAPSHOT__ = () => resolve(snapshot);
        testWindow.__HELD_SNAPSHOT_READY__ = true;
      });
    };
  }, "get_local_runtime_state");
}

async function releaseSnapshot(page: Page) {
  await page.evaluate(async () => {
    const release = (window as unknown as HeldSnapshotWindow).__RELEASE_SNAPSHOT__;
    if (!release) throw new Error("No runtime snapshot is pending");
    release();
    // Allow React to apply a wrongly accepted snapshot before asserting.
    await new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve)));
  });
}

async function switchMode(page: Page, label: "Computer" | "Relay Hosted") {
  await page.locator('.mode-picker > button[aria-haspopup="menu"]').click();
  await page.getByRole("menuitemradio", { name: label, exact: true }).click();
  await expect(label === "Relay Hosted"
    ? page.getByRole("button", { name: "Open settings", exact: true })
    : page.getByRole("heading", { name: "Overview", exact: true })).toBeVisible();
}

for (const mode of ["local"] as const) {
  test(`${mode} saved source is not replaced by an older background snapshot`, async ({ page }) => {
    await installTauriMock(page, { mode, locale: "en", populated: true });
    await page.goto("/");
    await page.getByRole("button", { name: "Connections", exact: true }).click();
    await page.getByRole("tab", { name: "Sources", exact: true }).click();
    await expect(page.locator(".source-card").filter({ hasText: "Example compatible API" })).toBeVisible();

    await holdNextSnapshot(page);
    await emitTauriEvent(page, "zenith-state-changed", null);
    await expect.poll(() => page.evaluate(() => (window as unknown as HeldSnapshotWindow).__HELD_SNAPSHOT_READY__)).toBe(true);
    await page.getByRole("button", { name: "Edit", exact: true }).click();
    const editor = page.getByRole("dialog", { name: "Edit source", exact: true });
    await editor.getByLabel("Name", { exact: true }).fill("Updated source");
    await editor.getByRole("button", { name: "Save", exact: true }).click();
    await expect(editor).toBeHidden();
    await expect(page.locator(".source-card").filter({ hasText: "Updated source" })).toBeVisible();

    await releaseSnapshot(page);
    await expect(page.locator(".source-card").filter({ hasText: "Updated source" })).toBeVisible();
    await expect(page.locator(".source-card").filter({ hasText: "Example compatible API" })).toHaveCount(0);
  });
}

test("Hosted retires live routing polling and returning resumes it", async ({ page }) => {
  await page.clock.install();
  await installTauriMock(page, { mode: "local", locale: "en", populated: true });
  const routingReads = () => page.evaluate(() => (window as unknown as {
    __TAURI_TEST_INVOKES__: Array<{ command: string }>;
  }).__TAURI_TEST_INVOKES__.filter(({ command }) => command === "get_local_runtime_order").length);
  await page.goto("/");
  await page.getByRole("button", { name: "Pool", exact: true }).click();
  await expect.poll(routingReads).toBeGreaterThan(0);
  await switchMode(page, "Relay Hosted");
  const pausedReads = await routingReads();
  await page.clock.fastForward(20_000);
  expect(await routingReads()).toBe(pausedReads);
  await switchMode(page, "Computer");
  await page.getByRole("button", { name: "Pool", exact: true }).click();
  await expect.poll(routingReads).toBeGreaterThan(pausedReads);
});

test("returning to a mode ignores its previous snapshot", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true });
  await page.goto("/");
  await expect(page.getByRole("button", { name: "Stop API", exact: true })).toBeVisible();
  await holdNextSnapshot(page);
  await emitTauriEvent(page, "zenith-state-changed", null);
  await expect.poll(() => page.evaluate(() => (window as unknown as HeldSnapshotWindow).__HELD_SNAPSHOT_READY__)).toBe(true);

  await switchMode(page, "Relay Hosted");
  // A native change can happen while another runtime is displayed.
  await page.evaluate(() => (window as unknown as HeldSnapshotWindow).__TAURI_INTERNALS__.invoke("stop_local_gateway"));
  await switchMode(page, "Computer");
  await expect(page.getByRole("button", { name: "Start API", exact: true })).toBeVisible();
  await releaseSnapshot(page);
  expect(await page.getByRole("button", { name: "Start API", exact: true }).count()).toBe(1);
  expect(await page.getByRole("button", { name: "Stop API", exact: true }).count()).toBe(0);
});

test("returning from Hosted does not restore completed activity or stale card status", async ({ page }) => {
  await installTauriMock(page, { locale: "en", populated: true, accountCount: 1, quotaAvailable: true, usageActive: false });
  await page.goto("/");
  const label = await page.evaluate(async () => {
    const target = window as unknown as HeldSnapshotWindow & { __COMPLETE_HIDDEN_IMAGE__?: () => void };
    const invoke = target.__TAURI_INTERNALS__.invoke.bind(target.__TAURI_INTERNALS__);
    const snapshot = await invoke("get_local_runtime_state") as RuntimeSnapshot;
    const account = snapshot.accounts.find((item) => item.inPool && !item.routeInactive)!;
    account.operationalStatus = "unavailable";
    const candidate: CandidateRuntimeSnapshot = {
      candidateId: account.id, kind: "oauth_account", available: false, capacityWait: true,
      runtimeId: 1, activityRevision: 1, inFlight: 0, activeRequestCount: 1,
      activeModels: [{ model: "gpt-image-2.6-sunburst", requestCount: 1 }],
      dispatches: 0, halfOpen: false, lastUsedAtMs: null, nextRetryAtMs: null,
    };
    snapshot.gateway.routingOrder = [candidate];
    snapshot.gateway.poolRotation = { inventoryRevision: 90, preset: null,
      memberOrder: [`account:${account.id}`], memberStatuses: { [`account:${account.id}`]: "unavailable" } };
    target.__COMPLETE_HIDDEN_IMAGE__ = () => {
      Object.assign(candidate, { available: true, capacityWait: false, runtimeId: 2, activityRevision: 0,
        activeRequestCount: 0, activeModels: [] });
      account.operationalStatus = "rotation";
      snapshot.gateway.poolRotation = { inventoryRevision: 1, preset: null,
        memberOrder: [`account:${account.id}`], memberStatuses: { [`account:${account.id}`]: "rotation" } };
    };
    target.__TAURI_INTERNALS__.invoke = async (command, args, options) => {
      if (command === "get_local_runtime_state") return structuredClone(snapshot);
      if (command === "get_local_runtime_order") return structuredClone({ routingOrder: snapshot.gateway.routingOrder,
        poolRotation: snapshot.gateway.poolRotation });
      return invoke(command, args, options);
    };
    return account.label;
  });
  await emitTauriEvent(page, "zenith-state-changed", null);
  await page.getByRole("button", { name: "Pool", exact: true }).click();
  const card = page.locator(".pool-member-card").filter({ hasText: label });
  await expect(card).toHaveAttribute("data-current", "true");
  await switchMode(page, "Relay Hosted");
  await page.evaluate(() => (window as unknown as { __COMPLETE_HIDDEN_IMAGE__: () => void }).__COMPLETE_HIDDEN_IMAGE__());
  await holdNextSnapshot(page);
  await switchMode(page, "Computer");
  await expect.poll(() => page.evaluate(() => (window as unknown as HeldSnapshotWindow).__HELD_SNAPSHOT_READY__)).toBe(true);
  await page.getByRole("button", { name: "Pool", exact: true }).click();
  await expect(card).toHaveAttribute("data-current", "false");
  await releaseSnapshot(page);
  await expect(card.locator(".relay-status-icon").first()).toHaveAttribute("data-status", "ready");
});

test("saving retires an old background read without blocking later updates", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true });
  await page.goto("/");
  await expect(page.getByRole("button", { name: "Stop API", exact: true })).toBeVisible();
  await holdNextSnapshot(page);
  await emitTauriEvent(page, "zenith-state-changed", null);
  await expect.poll(() => page.evaluate(() => (window as unknown as HeldSnapshotWindow).__HELD_SNAPSHOT_READY__)).toBe(true);

  await page.getByRole("button", { name: "Stop API", exact: true }).click();
  await expect(page.getByRole("button", { name: "Start API", exact: true })).toBeVisible();
  await page.evaluate(() => (window as unknown as HeldSnapshotWindow).__TAURI_INTERNALS__.invoke("start_local_gateway"));
  await emitTauriEvent(page, "zenith-state-changed", null);
  await expect(page.getByRole("button", { name: "Stop API", exact: true })).toBeVisible();
  await releaseSnapshot(page);
  await expect(page.getByRole("button", { name: "Stop API", exact: true })).toBeVisible();
});

test("a previous mode's pending read cannot block background updates after returning", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true });
  await page.goto("/");
  await expect(page.getByRole("button", { name: "Stop API", exact: true })).toBeVisible();
  await holdNextSnapshot(page);
  await emitTauriEvent(page, "zenith-state-changed", null);
  await expect.poll(() => page.evaluate(() => (window as unknown as HeldSnapshotWindow).__HELD_SNAPSHOT_READY__)).toBe(true);

  await switchMode(page, "Relay Hosted");
  await switchMode(page, "Computer");
  await expect(page.getByRole("button", { name: "Stop API", exact: true })).toBeVisible();
  await page.evaluate(() => (window as unknown as HeldSnapshotWindow).__TAURI_INTERNALS__.invoke("stop_local_gateway"));
  await emitTauriEvent(page, "zenith-state-changed", null);
  await expect(page.getByRole("button", { name: "Start API", exact: true })).toBeVisible();
  await releaseSnapshot(page);
  await expect(page.getByRole("button", { name: "Start API", exact: true })).toBeVisible();
});


for (const mode of ["local"] as const) {
  test(`${mode} periodic snapshots stay idle when the visible page does not use them`, async ({ page }) => {
    await installTauriMock(page, { mode, locale: "en", populated: true });
    await page.clock.install();
    await page.goto("/");
    await page.getByRole("button", { name: "Settings", exact: true }).click();
    const snapshotReads = () => page.evaluate(() => (window as unknown as {
      __TAURI_TEST_INVOKES__: Array<{ command: string }>;
    }).__TAURI_TEST_INVOKES__.filter(({ command }) => command === "get_local_runtime_state").length);
    const before = await snapshotReads();
    await page.clock.fastForward(60_000);
    await page.evaluate(() => window.dispatchEvent(new Event("focus")));
    expect(await snapshotReads()).toBe(before);
  });
}
