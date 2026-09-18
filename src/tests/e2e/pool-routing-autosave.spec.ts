import { expect, test, type Page } from "../bun-playwright";
import type { RuntimeSnapshot } from "../../src/features/relay/api/types";
import { installTauriMock } from "./tauri-mock";

async function openRotation(page: Page) {
  await page.getByRole("button", { name: "Pool", exact: true }).click();
  await page.getByRole("button", { name: "Pool rotation settings", exact: true }).click();
  return page.getByRole("dialog", { name: "Pool rotation", exact: true });
}

for (const mode of ["local", "remote"] as const) {
  test(`${mode} rotation rebases a racing save on current membership and settings`, async ({ page }) => {
    await installTauriMock(page, { mode, locale: "en", populated: true, sourceCount: 2 });
    await page.goto("/");
    const dialog = await openRotation(page);
    await page.evaluate((mode) => {
      const scope = window as unknown as { __TAURI_INTERNALS__: { invoke: (command: string, args?: any) => Promise<any> }; __routingAttempts: number };
      const invoke = scope.__TAURI_INTERNALS__.invoke;
      scope.__routingAttempts = 0;
      scope.__TAURI_INTERNALS__.invoke = async (command, args) => {
        const isSave = command === "update_local_routing" || (command === "execute_remote_server_action" && args?.input?.action?.type === "set_routing_policy");
        if (isSave && ++scope.__routingAttempts === 1) {
          const membership = { accountIds: ["account_synthetic"], sourceIds: [], inPool: false };
          await invoke(mode === "local" ? "set_local_pool_membership" : "execute_remote_server_action", {
            input: mode === "local" ? membership : { action: { type: "set_pool_membership" }, payload: membership },
          });
          const runtime = await invoke(mode === "local" ? "get_local_runtime_state" : "get_remote_server_state") as RuntimeSnapshot;
          const current = runtime.gateway.poolRouting!;
          const payload = { ...(mode === "local" ? args.input : args.input.payload), expectedPoolRouting: current,
            poolRouting: { ...current, members: current.members.map((member) => ({ ...member, weight: 9 })) },
            maxRetryCandidates: 5, defaultServiceTier: "fast" };
          await invoke(command, { input: mode === "local" ? payload : { action: { type: "set_routing_policy" }, payload } });
        }
        return invoke(command, args);
      };
    }, mode);
    await dialog.getByRole("radio", { name: "In order", exact: true }).click();
    await expect(dialog.locator(".pool-routing-editor")).toHaveAttribute("aria-busy", "false");
    await expect(dialog.getByRole("alert")).toHaveCount(0);
    await expect(dialog.getByRole("listitem")).toHaveCount(2);
    const saved = await page.evaluate(async (mode) => {
      const scope = window as unknown as { __TAURI_INTERNALS__: { invoke: (command: string) => Promise<RuntimeSnapshot> }; __routingAttempts: number };
      return { runtime: await scope.__TAURI_INTERNALS__.invoke(mode === "local" ? "get_local_runtime_state" : "get_remote_server_state"), attempts: scope.__routingAttempts };
    }, mode);
    expect(saved.attempts).toBe(2);
    expect(saved.runtime.gateway.poolRouting).toMatchObject({ mode: "in_order", members: [{ kind: "source", weight: 9 }, { kind: "source", weight: 9 }] });
    expect(saved.runtime.gateway.maxRetryCandidates).toBe(5);
    expect(saved.runtime.gateway.defaultServiceTier).toBe("fast");
  });
}

test("rotation serializes rapid edits, preserves numeric focus and waits before closing", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true });
  await page.goto("/");
  const dialog = await openRotation(page);
  await page.evaluate(() => {
    const scope = window as unknown as { __TAURI_INTERNALS__: { invoke: (command: string, args?: any) => Promise<any> }; __releaseRouting?: () => void; __routingPeak: number };
    const invoke = scope.__TAURI_INTERNALS__.invoke;
    let saves = 0;
    let active = 0;
    scope.__routingPeak = 0;
    scope.__TAURI_INTERNALS__.invoke = async (command, args) => {
      if (command !== "update_local_routing") return invoke(command, args);
      scope.__routingPeak = Math.max(scope.__routingPeak, ++active);
      try {
        if (++saves === 1) await new Promise<void>((resolve) => { scope.__releaseRouting = resolve; });
        return await invoke(command, args);
      } finally { active -= 1; }
    };
  });
  await dialog.getByRole("radio", { name: "Round robin", exact: true }).click();
  const weight = dialog.getByLabel("Request share: Example compatible API", { exact: true });
  await weight.fill("1");
  await weight.press("End");
  await weight.pressSequentially("2");
  await expect(weight).toHaveValue("12");
  await expect(weight).toBeFocused();
  await dialog.getByLabel("Concurrent requests: Example compatible API", { exact: true }).fill("23");
  await page.keyboard.press("Escape");
  await expect(dialog).toBeVisible();
  await page.evaluate(() => (window as unknown as { __releaseRouting: () => void }).__releaseRouting());
  await expect(dialog).toBeHidden();
  const saved = await page.evaluate(async () => {
    const scope = window as unknown as { __TAURI_INTERNALS__: { invoke: (command: string) => Promise<RuntimeSnapshot> }; __routingPeak: number };
    return { runtime: await scope.__TAURI_INTERNALS__.invoke("get_local_runtime_state"), peak: scope.__routingPeak };
  });
  expect(saved.peak).toBe(1);
  expect(saved.runtime.gateway.poolRouting).toMatchObject({ mode: "round_robin" });
  expect(saved.runtime.gateway.poolRouting?.members.find((member) => member.kind === "source")).toMatchObject({ weight: 12, maxConcurrency: 23 });
});

test("rotation rolls back a failed save and accepts the next edit", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true });
  await page.goto("/");
  const dialog = await openRotation(page);
  await page.evaluate(() => {
    const scope = window as unknown as { __TAURI_INTERNALS__: { invoke: (command: string, args?: any) => Promise<any> } };
    const invoke = scope.__TAURI_INTERNALS__.invoke;
    let failed = false;
    scope.__TAURI_INTERNALS__.invoke = async (command, args) => {
      if (command === "update_local_routing" && !failed) { failed = true; throw { code: "invalid_state", message: "synthetic save failure" }; }
      return invoke(command, args);
    };
  });
  await dialog.getByRole("radio", { name: "In order", exact: true }).click();
  await expect(dialog.getByRole("alert")).toBeVisible();
  await expect(dialog.getByRole("radio", { name: "Smart", exact: true })).toHaveAttribute("aria-checked", "true");
  await dialog.getByRole("radio", { name: "Round robin", exact: true }).click();
  await expect(dialog.locator(".pool-routing-editor")).toHaveAttribute("aria-busy", "false");
  await expect(dialog.getByRole("alert")).toHaveCount(0);
  await expect(dialog.getByRole("radio", { name: "Round robin", exact: true })).toHaveAttribute("aria-checked", "true");
});

test("pointer drag highlights the target and Escape cancels without saving", async ({ page }) => {
  await installTauriMock(page, { mode: "local", locale: "en", populated: true });
  await page.goto("/");
  const dialog = await openRotation(page);
  await dialog.getByRole("radio", { name: "In order", exact: true }).click();
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
  const dialog = await openRotation(page);
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
  await dialog.getByRole("radio", { name: "In order", exact: true }).click();
  await expect(dialog.getByRole("alert")).toContainText("Current values are shown");
  await expect(dialog.locator(".pool-routing-editor")).toHaveAttribute("aria-busy", "false");
  await expect(dialog.getByRole("radio", { name: "Smart", exact: true })).toHaveAttribute("aria-checked", "true");
  expect(await page.evaluate(() => (window as unknown as { __routingAttempts: number }).__routingAttempts)).toBe(3);
});
