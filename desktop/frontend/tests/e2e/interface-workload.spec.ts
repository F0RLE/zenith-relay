import { expect, test, type Page } from "../bun-playwright";
import { emitTauriEvent, installTauriMock } from "./tauri-mock";
import { openGatewayApi } from "./operations/helpers";

type WorkloadWindow = {
  __TAURI_INTERNALS__: { invoke: (name: string, args?: unknown, options?: unknown) => Promise<unknown> };
  __TAURI_TEST_INVOKES__: Array<{ command: string }>;
  __RELEASE_ROUTING__?: () => void;
};

const reads = (page: Page, command: string) => page.evaluate((name) => (
  window as unknown as WorkloadWindow
).__TAURI_TEST_INVOKES__.filter((call) => call.command === name).length, command);

test("a queued state notification does not read runtime after the window hides", async ({ page }) => {
  await page.clock.install();
  await installTauriMock(page, { locale: "en", populated: true });
  await page.goto("/");
  await expect(page.getByRole("button", { name: "Stop API", exact: true })).toBeVisible();
  const before = await reads(page, "get_local_runtime_state");
  await emitTauriEvent(page, "zenith-state-changed", null);
  await page.evaluate(() => {
    Object.defineProperty(document, "visibilityState", { configurable: true, value: "hidden" });
    document.dispatchEvent(new Event("visibilitychange"));
  });
  await page.clock.runFor(600);
  expect(await reads(page, "get_local_runtime_state")).toBe(before);
  await page.evaluate(() => {
    Object.defineProperty(document, "visibilityState", { configurable: true, value: "visible" });
    document.dispatchEvent(new Event("visibilitychange"));
  });
  await expect.poll(() => reads(page, "get_local_runtime_state")).toBe(before + 1);
});

test("returning to Settings reuses process-stable storage paths without a loading flash", async ({ page }) => {
  await installTauriMock(page, { locale: "en", populated: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Settings", exact: true }).click();
  const path = page.locator(".settings-path-row code").first();
  await expect(path).toContainText("Zenith Relay");
  const previousPath = await path.textContent();
  await page.getByRole("button", { name: "Help", exact: true }).click();
  await expect(page.locator('.relay-shell[data-page="help"]')).toBeVisible();
  // A second read must not be needed just to rediscover immutable process paths.
  await page.evaluate(() => {
    const internals = (window as unknown as WorkloadWindow).__TAURI_INTERNALS__;
    const invoke = internals.invoke.bind(internals);
    internals.invoke = async (name, args, options) => {
      const value = await invoke(name, args, options);
      return name === "get_relay_storage_info" ? new Promise(() => {}) : value;
    };
  });
  await page.getByRole("button", { name: "Settings", exact: true }).click();
  await expect(page.locator(".settings-page")).toBeVisible();
  expect(await path.textContent()).toBe(previousPath);
  expect(await reads(page, "get_relay_storage_info")).toBe(1);
});

test("failed storage path discovery retries on the next visit", async ({ page }) => {
  await installTauriMock(page, { locale: "en", populated: true });
  await page.addInitScript(() => {
    const internals = (window as unknown as WorkloadWindow).__TAURI_INTERNALS__;
    const invoke = internals.invoke.bind(internals);
    let fail = true;
    internals.invoke = async (name, args, options) => {
      const result = await invoke(name, args, options);
      if (name === "get_relay_storage_info" && fail) {
        fail = false;
        throw new Error("Synthetic storage path failure");
      }
      return result;
    };
  });
  await page.goto("/");
  await page.getByRole("button", { name: "Settings", exact: true }).click();
  await expect.poll(() => reads(page, "get_relay_storage_info")).toBe(1);
  await expect(page.locator(".settings-path-row code").first()).not.toContainText("Zenith Relay");
  await page.getByRole("button", { name: "Help", exact: true }).click();
  await expect(page.locator('.relay-shell[data-page="help"]')).toBeVisible();
  await page.getByRole("button", { name: "Settings", exact: true }).click();
  await expect(page.locator(".settings-path-row code").first()).toContainText("Zenith Relay");
  expect(await reads(page, "get_relay_storage_info")).toBe(2);
});

test("copying the API key does not restart network observation or refresh the runtime", async ({ page }) => {
  await page.clock.install();
  await installTauriMock(page, { locale: "en", populated: true });
  await page.goto("/");
  await openGatewayApi(page);
  await expect(page.locator(".gateway-network-access input")).toBeEnabled();
  const beforeNetwork = await reads(page, "get_local_network_access");
  const beforeRuntime = await reads(page, "get_local_runtime_state");
  await page.locator(".gateway-api-local").getByRole("button", { name: "Copy key", exact: true }).click();
  await expect.poll(() => reads(page, "reveal_local_gateway_api_key")).toBe(1);
  await page.clock.runFor(600);
  expect(await reads(page, "get_local_network_access")).toBe(beforeNetwork);
  expect(await reads(page, "get_local_runtime_state")).toBe(beforeRuntime);
});

test("opening a folder and exporting usage do not reload runtime state", async ({ page }) => {
  await page.clock.install();
  await installTauriMock(page, { locale: "en", populated: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Settings", exact: true }).click();
  await expect(page.locator(".settings-page")).toBeVisible();
  const before = await reads(page, "get_local_runtime_state");
  await page.getByRole("button", { name: "Open data folder", exact: true }).click();
  await expect.poll(() => reads(page, "open_relay_folder")).toBe(1);
  await page.clock.runFor(600);
  expect(await reads(page, "get_local_runtime_state")).toBe(before);

  await page.getByRole("button", { name: "Usage", exact: true }).click();
  await expect(page.locator(".usage-page")).toBeVisible();
  const beforeExport = await reads(page, "get_local_runtime_state");
  await page.getByLabel("Actions", { exact: true }).click();
  await page.getByRole("menuitem", { name: "Export", exact: true }).click();
  await expect.poll(() => reads(page, "export_usage")).toBe(1);
  await page.clock.runFor(600);
  expect(await reads(page, "get_local_runtime_state")).toBe(beforeExport);
});

for (const [command, success] of [
  ["export_local_accounts", "Account export saved."],
  ["export_usage", "Redacted export created."],
] as const) {
  test(`${command} cancellation stays silent, preserves the form and permits retry without runtime reads`, async ({ page }) => {
    await page.clock.install();
    await installTauriMock(page, { locale: "en", mode: "local", populated: true });
    await page.addInitScript((cancelledCommand) => {
      const target = window as unknown as WorkloadWindow;
      const invoke = target.__TAURI_INTERNALS__.invoke.bind(target.__TAURI_INTERNALS__);
      let cancelFirst = true;
      target.__TAURI_INTERNALS__.invoke = async (name, args, options) => {
        const value = await invoke(name, args, options);
        if (name !== cancelledCommand || !cancelFirst) return value;
        cancelFirst = false;
        if (name === "export_usage") return null;
        const cancelled = { ...(value as Record<string, unknown>) };
        delete cancelled.path;
        return cancelled;
      };
    }, command);
    await page.goto("/");
    const accountExport = command === "export_local_accounts";
    const dialog = page.getByRole("dialog", { name: "Export accounts" });
    if (accountExport) {
      await page.getByRole("button", { name: "Connections", exact: true }).click();
      await page.locator(".account-bulk-menu summary").click();
      await page.getByRole("menuitem", { name: "Export all" }).click();
      await dialog.getByLabel("Markdown description").fill("Synthetic package description");
    } else {
      await page.getByRole("button", { name: "Usage", exact: true }).click();
      await expect(page.locator(".usage-page")).toBeVisible();
    }
    const startExport = async () => {
      if (accountExport) await dialog.getByRole("button", { name: "Download JSON" }).click();
      else {
        await page.getByLabel("Actions", { exact: true }).click();
        await page.getByRole("menuitem", { name: "Export", exact: true }).click();
      }
    };
    const before = await reads(page, "get_local_runtime_state");
    await startExport();
    await expect.poll(() => reads(page, command)).toBe(1);
    if (accountExport) {
      await expect(dialog.getByRole("button", { name: "Download JSON" })).toBeEnabled();
      await expect(dialog.getByLabel("Markdown description")).toHaveValue("Synthetic package description");
    }
    await page.clock.runFor(600);
    await expect(page.getByText(success, { exact: true })).toHaveCount(0);
    expect(await reads(page, "get_local_runtime_state")).toBe(before);
    expect(await reads(page, "record_frontend_diagnostic")).toBe(0);
    await startExport();
    await expect(page.getByText(success, { exact: true })).toBeVisible();
    expect(await reads(page, command)).toBe(2);
    if (accountExport) await expect(dialog).toHaveCount(0);
    await page.clock.runFor(600);
    expect(await reads(page, "get_local_runtime_state")).toBe(before);
  });
}

for (const holdRead of [false, true]) {
  test(`moving between Pool and Connections preserves one routing observer (${holdRead ? "pending" : "settled"})`, async ({ page }) => {
    await page.clock.install();
    await installTauriMock(page, { locale: "en", populated: true });
    await page.goto("/");
    await expect(page.getByRole("button", { name: "Stop API", exact: true })).toBeVisible();
    if (holdRead) await page.evaluate(() => {
      const target = window as unknown as WorkloadWindow;
      const invoke = target.__TAURI_INTERNALS__.invoke.bind(target.__TAURI_INTERNALS__);
      let first = true;
      target.__TAURI_INTERNALS__.invoke = async (name, args, options) => {
        const result = await invoke(name, args, options);
        if (name !== "get_local_runtime_order" || !first) return result;
        first = false;
        await new Promise<void>((resolve) => { target.__RELEASE_ROUTING__ = resolve; });
        return result;
      };
    });
    await page.getByRole("button", { name: "Pool", exact: true }).click();
    await expect.poll(() => reads(page, "get_local_runtime_order")).toBe(1);
    await page.getByRole("button", { name: "Connections", exact: true }).click();
    await expect(page.locator(".relay-shell")).toHaveAttribute("data-page", "connections");
    await page.clock.runFor(600);
    expect(await reads(page, "get_local_runtime_order")).toBe(1);
    await page.getByRole("button", { name: "Pool", exact: true }).click();
    await expect(page.locator(".pool-controls")).toBeVisible();
    await page.clock.runFor(600);
    expect(await reads(page, "get_local_runtime_order")).toBe(1);
    if (holdRead) await page.evaluate(() => (window as unknown as WorkloadWindow).__RELEASE_ROUTING__?.());
    await page.clock.runFor(5_000);
    await expect.poll(() => reads(page, "get_local_runtime_order")).toBe(2);
  });
}

for (const eventDuringRead of [false, true]) {
  test(`a slow routing read queues only real changes, not timer ticks (${eventDuringRead ? "activity" : "idle"})`, async ({ page }) => {
    await page.clock.install();
    await installTauriMock(page, { locale: "en", populated: true });
    await page.goto("/");
    await expect(page.getByRole("button", { name: "Stop API", exact: true })).toBeVisible();
    await page.evaluate(() => {
      const target = window as unknown as WorkloadWindow;
      const invoke = target.__TAURI_INTERNALS__.invoke.bind(target.__TAURI_INTERNALS__);
      let hold = true;
      target.__TAURI_INTERNALS__.invoke = async (name, args, options) => {
        const result = await invoke(name, args, options);
        if (name !== "get_local_runtime_order" || !hold) return result;
        hold = false;
        return new Promise((resolve) => { target.__RELEASE_ROUTING__ = () => resolve(result); });
      };
    });
    await page.getByRole("button", { name: "Pool", exact: true }).click();
    await expect.poll(() => page.evaluate(() => Boolean((window as unknown as WorkloadWindow).__RELEASE_ROUTING__))).toBe(true);
    await page.clock.runFor(5_100);
    if (eventDuringRead) {
      await emitTauriEvent(page, "zenith-runtime-activity", {
        runtimeId: 1, revision: 10, candidateId: "account_synthetic", inFlight: 1,
        activeRequestCount: 1, activeModels: [{ model: "gpt-5.4", requestCount: 1 }],
      });
      await page.clock.runFor(600);
    }
    expect(await reads(page, "get_local_runtime_order")).toBe(1);
    await page.evaluate(() => (window as unknown as WorkloadWindow).__RELEASE_ROUTING__?.());
    await page.clock.runFor(600);
    expect(await reads(page, "get_local_runtime_order")).toBe(eventDuringRead ? 2 : 1);
  });
}

test("routing fallback follows the last read and resumes after hiding", async ({ page }) => {
  await page.clock.install();
  await installTauriMock(page, { locale: "en", populated: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Pool", exact: true }).click();
  await expect(page.locator(".pool-controls")).toBeVisible();
  await expect.poll(() => reads(page, "get_local_runtime_order")).toBe(1);
  await page.clock.runFor(4_000);
  await emitTauriEvent(page, "zenith-runtime-activity", {
    runtimeId: 1, revision: 10, candidateId: "account_synthetic", inFlight: 1,
    activeRequestCount: 1, activeModels: [{ model: "gpt-5.4", requestCount: 1 }],
  });
  await page.clock.runFor(600);
  expect(await reads(page, "get_local_runtime_order")).toBe(2);
  await page.clock.runFor(600);
  expect(await reads(page, "get_local_runtime_order")).toBe(2);
  await page.clock.runFor(5_000);
  expect(await reads(page, "get_local_runtime_order")).toBe(3);
  await page.evaluate(() => {
    Object.defineProperty(document, "visibilityState", { configurable: true, value: "hidden" });
    document.dispatchEvent(new Event("visibilitychange"));
  });
  await page.clock.runFor(20_000);
  expect(await reads(page, "get_local_runtime_order")).toBe(3);
  await page.evaluate(() => {
    Object.defineProperty(document, "visibilityState", { configurable: true, value: "visible" });
    document.dispatchEvent(new Event("visibilitychange"));
  });
  await page.clock.runFor(600);
  expect(await reads(page, "get_local_runtime_order")).toBe(4);
  await page.getByRole("button", { name: "Settings", exact: true }).click();
  await expect(page.locator(".settings-page")).toBeVisible();
  await page.clock.runFor(10_000);
  expect(await reads(page, "get_local_runtime_order")).toBe(4);
});

test("routing fallback retries after a failed read", async ({ page }) => {
  await page.clock.install();
  await installTauriMock(page, { locale: "en", populated: true });
  await page.addInitScript(() => {
    const internals = (window as unknown as WorkloadWindow).__TAURI_INTERNALS__;
    const invoke = internals.invoke.bind(internals);
    let fail = true;
    internals.invoke = async (name, args, options) => {
      const result = await invoke(name, args, options);
      if (name === "get_local_runtime_order" && fail) {
        fail = false;
        throw new Error("Synthetic routing failure");
      }
      return result;
    };
  });
  await page.goto("/");
  await page.getByRole("button", { name: "Pool", exact: true }).click();
  await expect(page.locator(".pool-controls")).toBeVisible();
  await expect.poll(() => reads(page, "get_local_runtime_order")).toBe(1);
  await page.clock.runFor(5_100);
  expect(await reads(page, "get_local_runtime_order")).toBe(2);
  await expect(page.locator(".pool-member-card").first()).toBeVisible();
});
