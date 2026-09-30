import { expect, test, type Page } from "../bun-playwright";
import { emitTauriEvent, installTauriMock } from "./tauri-mock";

type HeldSnapshotWindow = {
  __TAURI_INTERNALS__: { invoke: (command: string, args?: unknown, options?: unknown) => Promise<unknown> };
  __HELD_SNAPSHOT_READY__?: boolean;
  __RELEASE_SNAPSHOT__?: () => void;
};

async function holdNextSnapshot(page: Page, mode: "local" | "remote") {
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
  }, mode === "local" ? "get_local_runtime_state" : "get_remote_server_state");
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

async function switchMode(page: Page, label: "Computer" | "On your server") {
  await page.locator('.mode-picker > button[aria-haspopup="menu"]').click();
  await page.getByRole("menuitemradio", { name: label, exact: true }).click();
  await expect(page.getByRole("heading", { name: "Overview", exact: true })).toBeVisible();
}

for (const mode of ["local", "remote"] as const) {
  test(`${mode} saved source is not replaced by an older background snapshot`, async ({ page }) => {
    await installTauriMock(page, { mode, locale: "en", populated: true });
    await page.goto("/");
    await page.getByRole("button", { name: "Connections", exact: true }).click();
    await page.getByRole("tab", { name: "Sources", exact: true }).click();
    await expect(page.getByRole("row").filter({ hasText: "Example compatible API" })).toBeVisible();

    await holdNextSnapshot(page, mode);
    await emitTauriEvent(page, "zenith-state-changed", null);
    await expect.poll(() => page.evaluate(() => (window as unknown as HeldSnapshotWindow).__HELD_SNAPSHOT_READY__)).toBe(true);
    await page.getByRole("button", { name: "Edit", exact: true }).click();
    const editor = page.getByRole("dialog", { name: "Edit source", exact: true });
    await editor.getByLabel("Name", { exact: true }).fill("Updated source");
    await editor.getByRole("button", { name: "Save", exact: true }).click();
    await expect(editor).toBeHidden();
    await expect(page.getByRole("row").filter({ hasText: "Updated source" })).toBeVisible();

    await releaseSnapshot(page);
    await expect(page.getByRole("row").filter({ hasText: "Updated source" })).toBeVisible();
    await expect(page.getByRole("row").filter({ hasText: "Example compatible API" })).toHaveCount(0);
  });
}

test("returning to a mode ignores its previous snapshot", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true });
  await page.goto("/");
  await expect(page.getByRole("button", { name: "Stop API", exact: true })).toBeVisible();
  await holdNextSnapshot(page, "local");
  await emitTauriEvent(page, "zenith-state-changed", null);
  await expect.poll(() => page.evaluate(() => (window as unknown as HeldSnapshotWindow).__HELD_SNAPSHOT_READY__)).toBe(true);

  await switchMode(page, "On your server");
  // A native change can happen while another runtime is displayed.
  await page.evaluate(() => (window as unknown as HeldSnapshotWindow).__TAURI_INTERNALS__.invoke("stop_local_gateway"));
  await switchMode(page, "Computer");
  await expect(page.getByRole("button", { name: "Start API", exact: true })).toBeVisible();
  await releaseSnapshot(page);
  expect(await page.getByRole("button", { name: "Start API", exact: true }).count()).toBe(1);
  expect(await page.getByRole("button", { name: "Stop API", exact: true }).count()).toBe(0);
});

test("saving retires an old background read without blocking later updates", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true });
  await page.goto("/");
  await expect(page.getByRole("button", { name: "Stop API", exact: true })).toBeVisible();
  await holdNextSnapshot(page, "local");
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
  await holdNextSnapshot(page, "local");
  await emitTauriEvent(page, "zenith-state-changed", null);
  await expect.poll(() => page.evaluate(() => (window as unknown as HeldSnapshotWindow).__HELD_SNAPSHOT_READY__)).toBe(true);

  await switchMode(page, "On your server");
  await switchMode(page, "Computer");
  await expect(page.getByRole("button", { name: "Stop API", exact: true })).toBeVisible();
  await page.evaluate(() => (window as unknown as HeldSnapshotWindow).__TAURI_INTERNALS__.invoke("stop_local_gateway"));
  await emitTauriEvent(page, "zenith-state-changed", null);
  await expect(page.getByRole("button", { name: "Start API", exact: true })).toBeVisible();
  await releaseSnapshot(page);
  await expect(page.getByRole("button", { name: "Start API", exact: true })).toBeVisible();
});

for (const trigger of ["interval", "focus"] as const) {
  test(`remote changes refresh on ${trigger} without a desktop state event`, async ({ page }) => {
    await installTauriMock(page, { mode: "remote", locale: "en", populated: true });
    await page.clock.install();
    await page.goto("/");
    await page.getByRole("button", { name: "Connections", exact: true }).click();
    await page.getByRole("tab", { name: "Sources", exact: true }).click();
    await expect(page.getByRole("row").filter({ hasText: "Example compatible API" })).toBeVisible();
    await page.evaluate(() => {
      const internals = (window as unknown as HeldSnapshotWindow).__TAURI_INTERNALS__;
      const invoke = internals.invoke.bind(internals);
      internals.invoke = async (command, args, options) => {
        const result = await invoke(command, args, options);
        if (command === "get_remote_server_state") {
          const snapshot = result as { sources: Array<{ name: string }> };
          if (snapshot.sources[0]) snapshot.sources[0].name = "Changed on server";
        }
        return result;
      };
    });
    if (trigger === "interval") await page.clock.fastForward(60_000);
    else await page.evaluate(() => window.dispatchEvent(new Event("focus")));
    await expect(page.getByRole("row").filter({ hasText: "Changed on server" })).toBeVisible();
  });
}

for (const mode of ["local", "remote"] as const) {
  test(`${mode} periodic snapshots stay idle when the visible page does not use them`, async ({ page }) => {
    await installTauriMock(page, { mode, locale: "en", populated: true });
    await page.clock.install();
    await page.goto("/");
    await page.getByRole("button", { name: "Settings", exact: true }).click();
    const snapshotReads = () => page.evaluate(() => (window as unknown as {
      __TAURI_TEST_INVOKES__: Array<{ command: string }>;
    }).__TAURI_TEST_INVOKES__.filter(({ command }) => command === "get_local_runtime_state" || command === "get_remote_server_state").length);
    const before = await snapshotReads();
    await page.clock.fastForward(60_000);
    await page.evaluate(() => window.dispatchEvent(new Event("focus")));
    expect(await snapshotReads()).toBe(before);
  });
}
