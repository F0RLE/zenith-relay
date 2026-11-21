import { expect, test } from "../bun-playwright";
import type { RuntimeSnapshot } from "../../src/features/relay/api/types";
import { installTauriMock } from "./tauri-mock";
import { openPoolRotation } from "./pool-rotation";

test("manual moves wait for acknowledged order before accepting another move", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true, sourceCount: 3 });
  await page.goto("/");
  const dialog = await openPoolRotation(page);
  await dialog.getByRole("radio", { name: "Round robin", exact: true }).click();
  await expect(dialog.locator(".pool-routing-editor")).toHaveAttribute("aria-busy", "false");
  const rows = dialog.locator(".pool-routing-member");
  const memberId = await rows.last().getAttribute("data-member-id");
  const member = dialog.locator(`.pool-routing-member[data-member-id="${memberId}"]`);
  const initialIndex = await rows.count() - 1;
  await page.evaluate(() => {
    const host = window as unknown as { __TAURI_INTERNALS__: { invoke: (command: string, args?: unknown) => Promise<unknown> }; __releaseMove?: () => void };
    const invoke = host.__TAURI_INTERNALS__.invoke.bind(host.__TAURI_INTERNALS__);
    let first = true;
    host.__TAURI_INTERNALS__.invoke = async (command, args) => {
      if (command === "update_local_routing" && first) {
        first = false;
        await new Promise<void>((resolve) => { host.__releaseMove = resolve; });
      }
      return invoke(command, args);
    };
  });
  const up = member.getByRole("button", { name: /^Move .+ up$/ });
  await up.click();
  await expect(up).toBeDisabled();
  await expect(member.locator(".pool-routing-handle")).toBeDisabled();
  await page.evaluate(() => (window as unknown as { __releaseMove: () => void }).__releaseMove());
  await expect(up).toBeEnabled();
  await up.click();
  await expect(dialog.locator(".pool-routing-editor")).toHaveAttribute("aria-busy", "false");
  await expect(rows.nth(initialIndex - 2)).toHaveAttribute("data-member-id", memberId!);
});

for (const mode of ["local"] as const) {
  test(`${mode} rotation rebases a racing save on current membership and settings`, async ({ page }) => {
    await installTauriMock(page, { mode, locale: "en", populated: true, sourceCount: 2 });
    await page.goto("/");
    const dialog = await openPoolRotation(page);
    await page.evaluate((mode) => {
      const scope = window as unknown as { __TAURI_INTERNALS__: { invoke: (command: string, args?: any) => Promise<any> }; __routingAttempts: number };
      const invoke = scope.__TAURI_INTERNALS__.invoke;
      scope.__routingAttempts = 0;
      scope.__TAURI_INTERNALS__.invoke = async (command, args) => {
        const isSave = command === "update_local_routing";
        if (isSave && ++scope.__routingAttempts === 1) {
          const membership = { accountIds: ["account_synthetic"], sourceIds: [], inPool: false };
          await invoke("set_local_pool_membership", {
            input: membership,
          });
          const runtime = await invoke("get_local_runtime_state") as RuntimeSnapshot;
          const current = runtime.gateway.poolRouting!;
          const payload = { expectedPoolRouting: current,
            poolRouting: { ...current, members: current.members.map((member) => ({ ...member, weight: 9 })) },
            maxRetryCandidates: 5, defaultServiceTier: "fast" };
          await invoke(command, { input: payload });
        }
        return invoke(command, args);
      };
    }, mode);
    await dialog.getByRole("radio", { name: "Round robin", exact: true }).click();
    await expect(dialog.locator(".pool-routing-editor")).toHaveAttribute("aria-busy", "false");
    await expect(dialog.getByRole("alert")).toHaveCount(0);
    await expect(dialog.getByRole("listitem")).toHaveCount(2);
    const saved = await page.evaluate(async (mode) => {
      const scope = window as unknown as { __TAURI_INTERNALS__: { invoke: (command: string) => Promise<RuntimeSnapshot> }; __routingAttempts: number };
      return { runtime: await scope.__TAURI_INTERNALS__.invoke("get_local_runtime_state"), attempts: scope.__routingAttempts };
    }, mode);
    expect(saved.attempts).toBe(2);
    expect(saved.runtime.gateway.poolRouting).toMatchObject({ mode: "round_robin", members: [{ kind: "source", weight: 9 }, { kind: "source", weight: 9 }] });
    expect(saved.runtime.gateway.maxRetryCandidates).toBe(5);
    expect(saved.runtime.gateway.defaultServiceTier).toBe("fast");
  });
}

test("rotation serializes rapid edits without waiting for timing writes and preserves focus before close", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true });
  await page.goto("/");
  const dialog = await openPoolRotation(page);
  await page.evaluate(() => {
    const scope = window as unknown as { __TAURI_INTERNALS__: { invoke: (command: string, args?: any) => Promise<any> }; __releaseRouting?: () => void; __routingPeak: number };
    const invoke = scope.__TAURI_INTERNALS__.invoke;
    let saves = 0;
    let active = 0;
    scope.__routingPeak = 0;
    scope.__TAURI_INTERNALS__.invoke = async (command, args) => {
      if (command === "record_local_performance_sample" && args?.name === "command_save"
          && args?.context === "update_local_routing:ok") {
        return new Promise<void>(() => undefined);
      }
      if (command !== "update_local_routing") return invoke(command, args);
      scope.__routingPeak = Math.max(scope.__routingPeak, ++active);
      try {
        if (++saves === 1) await new Promise<void>((resolve) => { scope.__releaseRouting = resolve; });
        return await invoke(command, args);
      } finally { active -= 1; }
    };
  });
  await dialog.getByRole("radio", { name: "Automatic", exact: true }).click();
  const weight = dialog.getByLabel("Request share: Example compatible API", { exact: true });
  await weight.fill("1");
  await weight.press("End");
  await weight.pressSequentially("2");
  await expect(weight).toHaveValue("12");
  await expect(weight).toBeFocused();
  await dialog.getByLabel("Concurrent requests: Example compatible API", { exact: true }).fill("23");
  await dialog.getByRole("button", { name: "Close", exact: true }).click();
  await expect(dialog).toBeVisible();
  await page.evaluate(() => (window as unknown as { __releaseRouting: () => void }).__releaseRouting());
  await expect(dialog).toBeHidden();
  const saved = await page.evaluate(async () => {
    const scope = window as unknown as { __TAURI_INTERNALS__: { invoke: (command: string) => Promise<RuntimeSnapshot> }; __routingPeak: number };
    return { runtime: await scope.__TAURI_INTERNALS__.invoke("get_local_runtime_state"), peak: scope.__routingPeak };
  });
  expect(saved.peak).toBe(1);
  expect(saved.runtime.gateway.poolRouting).toMatchObject({ mode: "automatic" });
  expect(saved.runtime.gateway.poolRouting?.members.find((member) => member.kind === "source")).toMatchObject({ weight: 12, maxConcurrency: 23 });
});

test("rotation retains queued edits when the post-save display refresh fails", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true });
  await page.goto("/");
  const dialog = await openPoolRotation(page);
  await page.evaluate(() => {
    const host = window as unknown as {
      __TAURI_INTERNALS__: { invoke: (command: string, args?: unknown, options?: unknown) => Promise<unknown> };
      __releaseRouting?: () => void;
      __failedRoutingDisplayReads: number;
    };
    const invoke = host.__TAURI_INTERNALS__.invoke.bind(host.__TAURI_INTERNALS__);
    let firstSave = true;
    let committed = false;
    let readsAfterSave = 0;
    host.__failedRoutingDisplayReads = 0;
    host.__TAURI_INTERNALS__.invoke = async (command, args, options) => {
      if (command === "get_local_runtime_state" && committed && ++readsAfterSave === 2) {
        host.__failedRoutingDisplayReads += 1;
        throw new Error("Synthetic display snapshot failure");
      }
      if (command === "update_local_routing" && firstSave) {
        firstSave = false;
        await new Promise<void>((resolve) => { host.__releaseRouting = resolve; });
        const result = await invoke(command, args, options);
        committed = true;
        return result;
      }
      return invoke(command, args, options);
    };
  });
  await dialog.getByRole("radio", { name: "Automatic", exact: true }).click();
  await dialog.getByLabel("Request share: Example compatible API", { exact: true }).fill("12");
  await dialog.getByLabel("Concurrent requests: Example compatible API", { exact: true }).fill("23");
  await page.evaluate(() => (window as unknown as { __releaseRouting: () => void }).__releaseRouting());
  await expect(dialog.locator(".pool-routing-editor")).toHaveAttribute("aria-busy", "false");
  const saved = await page.evaluate(async () => {
    const host = window as unknown as {
      __TAURI_INTERNALS__: { invoke: (command: string) => Promise<RuntimeSnapshot> };
      __failedRoutingDisplayReads: number;
    };
    return { runtime: await host.__TAURI_INTERNALS__.invoke("get_local_runtime_state"), failedReads: host.__failedRoutingDisplayReads };
  });
  expect(saved.failedReads).toBe(1);
  expect(saved.runtime.gateway.poolRouting?.members.find((member) => member.kind === "source"))
    .toMatchObject({ weight: 12, maxConcurrency: 23 });
});

test("rotation rolls back a failed save and accepts the next edit", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true });
  await page.goto("/");
  const dialog = await openPoolRotation(page);
  await page.evaluate(() => {
    const scope = window as unknown as { __TAURI_INTERNALS__: { invoke: (command: string, args?: any) => Promise<any> } };
    const invoke = scope.__TAURI_INTERNALS__.invoke;
    let failed = false;
    scope.__TAURI_INTERNALS__.invoke = async (command, args) => {
      if (command === "update_local_routing" && !failed) { failed = true; throw { code: "invalid_state", message: "synthetic save failure" }; }
      return invoke(command, args);
    };
  });
  await dialog.getByRole("radio", { name: "Round robin", exact: true }).click();
  await expect(dialog.getByRole("alert")).toBeVisible();
  await expect(dialog.getByRole("radio", { name: "Automatic", exact: true })).toHaveAttribute("aria-checked", "true");
  await dialog.getByRole("radio", { name: "Round robin", exact: true }).click();
  await expect(dialog.locator(".pool-routing-editor")).toHaveAttribute("aria-busy", "false");
  await expect(dialog.getByRole("alert")).toHaveCount(0);
  await expect(dialog.getByRole("radio", { name: "Round robin", exact: true })).toHaveAttribute("aria-checked", "true");
  const samples = await page.evaluate(() => (window as unknown as {
    __TAURI_TEST_INVOKES__: Array<{ command: string; args: Record<string, unknown> }>;
  }).__TAURI_TEST_INVOKES__.filter(call => call.command === "record_local_performance_sample"
    && call.args.name === "command_save"
    && String(call.args.context).startsWith("update_local_routing:")).map(call => call.args));
  expect(samples.map(sample => sample.context)).toEqual([
    "update_local_routing:error", "update_local_routing:ok",
  ]);
  for (const sample of samples) {
    expect(Object.keys(sample).sort()).toEqual(["context", "durationMs", "name"]);
    expect(Number.isFinite(sample.durationMs)).toBe(true);
    expect(sample.durationMs).toBeGreaterThanOrEqual(0);
  }
});

test("pointer drag highlights the target and Escape cancels without saving", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true });
  await page.goto("/");
  const dialog = await openPoolRotation(page);
  await dialog.getByRole("radio", { name: "Round robin", exact: true }).click();
  await expect(dialog.locator(".pool-routing-editor")).toHaveAttribute("aria-busy", "false");
  const ids = () => dialog.getByRole("listitem").evaluateAll((rows) => rows.map((row) => row.getAttribute("data-member-id")));
  const before = await ids();
  const handle = dialog.locator(".pool-routing-handle").first();
  const target = dialog.getByRole("listitem").last();
  await handle.hover();
  await page.mouse.down();
  await target.hover();
  await expect(target).toHaveAttribute("data-drop-target", "true");
  await page.keyboard.press("Escape");
  await page.mouse.up();
  expect(await ids()).toEqual(before);
  await expect(dialog).toBeVisible();
  await handle.dragTo(target);
  expect(await ids()).toEqual([...before].reverse());
  await expect(dialog.locator(".pool-routing-editor")).toHaveAttribute("aria-busy", "false");
  await dialog.getByRole("button", { name: "Close", exact: true }).last().click();
  await page.getByRole("button", { name: "Pool rotation settings", exact: true }).click();
  expect(await ids()).toEqual([...before].reverse());
});

test("repeated routing conflicts stop after three attempts and restore stored values", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true });
  await page.goto("/");
  const dialog = await openPoolRotation(page);
  await page.evaluate(() => {
    const scope = window as unknown as { __TAURI_INTERNALS__: { invoke: (command: string, args?: any) => Promise<any> }; __routingAttempts: number };
    const invoke = scope.__TAURI_INTERNALS__.invoke;
    scope.__routingAttempts = 0;
    scope.__TAURI_INTERNALS__.invoke = async (command, args) => {
      if (command === "update_local_routing") {
        scope.__routingAttempts += 1;
        throw { code: "conflict", message: "synthetic concurrent update" };
      }
      return invoke(command, args);
    };
  });
  await dialog.getByRole("radio", { name: "Round robin", exact: true }).click();
  await expect(dialog.getByRole("alert")).toContainText("Current values are shown");
  await expect(dialog.locator(".pool-routing-editor")).toHaveAttribute("aria-busy", "false");
  await expect(dialog.getByRole("radio", { name: "Automatic", exact: true })).toHaveAttribute("aria-checked", "true");
  expect(await page.evaluate(() => (window as unknown as { __routingAttempts: number }).__routingAttempts)).toBe(3);
});
