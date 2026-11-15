import { expect, test, type Page } from "../bun-playwright";
import type { RuntimeSnapshot, SourceStats } from "../../src/features/relay/api/types";
import { emitTauriEvent, installTauriMock } from "./tauri-mock";

type StatsControl = {
  observations: Record<string, SourceStats>;
  revisions: Record<string, number>;
  endpoints: Record<string, string>;
  removed: string[];
  uncached: string[];
  calls: string[];
  hold: boolean;
  fail: boolean;
  completed: number;
  marker: number;
  release?: () => void;
};
type ControlledWindow = Window & {
  __statsControl: StatsControl;
  __TAURI_INTERNALS__: { invoke: (command: string, args?: Record<string, unknown>, options?: unknown) => Promise<unknown> };
};

async function installControlledSources(page: Page) {
  await installTauriMock(page, { mode: "local", locale: "en", sourceCount: 2 });
  await page.addInitScript(() => {
    const host = window as ControlledWindow;
    const observed: SourceStats = {
      provider: "sub2api", status: "available", balanceMicroUsd: 12_340_000,
      spentMicroUsd: null, requests: null, totalTokens: null, asOfMs: 100,
    };
    const state = host.__statsControl = {
      observations: { source_synthetic: observed, source_synthetic_2: observed },
      revisions: {}, endpoints: {}, removed: [], uncached: [], calls: [], hold: false, fail: false, completed: 0, marker: 0,
    } as StatsControl;
    const invoke = host.__TAURI_INTERNALS__.invoke.bind(host.__TAURI_INTERNALS__);
    host.__TAURI_INTERNALS__.invoke = async (command, args, options) => {
      if (command === "get_local_source_stats") {
        const sourceId = String(args?.sourceId);
        state.calls.push(sourceId);
        if (args?.force) {
          if (state.hold) await new Promise<void>((resolve) => { state.release = resolve; });
          state.completed += 1;
          if (state.fail) throw { code: "upstream_error", message: "Synthetic balance failure" };
          return { ...observed, balanceMicroUsd: 22_000_000, asOfMs: 200 };
        }
        return structuredClone(state.observations[sourceId]);
      }
      const result = await invoke(command, args, options);
      if (command === "get_local_runtime_state") {
        const snapshot = result as RuntimeSnapshot;
        snapshot.sources = snapshot.sources.filter((source) => !state.removed.includes(source.id));
        for (const source of snapshot.sources) {
          source.providerStats = state.uncached.includes(source.id) ? null : state.observations[source.id];
          source.refreshRevision = state.revisions[source.id] ?? 1;
          source.baseUrl = state.endpoints[source.id] ?? source.baseUrl;
          source.name = `${source.id} snapshot ${state.marker}`;
        }
      }
      return result;
    };
  });
  await page.goto("/");
  await page.getByRole("button", { name: "Pool", exact: true }).click();
  const source = page.locator('.pool-member-card[data-member-kind="source"]').first();
  await expect(source.locator('[data-metric="balance"]')).toContainText("$12.34");
  return source;
}

async function refreshSnapshot(page: Page) {
  const marker = await page.evaluate(() => ++(window as ControlledWindow).__statsControl.marker);
  await emitTauriEvent(page, "zenith-state-changed", null);
  await expect(page.locator('.pool-member-card[data-member-kind="source"]').first())
    .toContainText(`source_synthetic snapshot ${marker}`);
}

for (const change of ["revision", "remove"] as const) {
  test(`another source's ${change} does not roll back a newer balance or refetch it`, async ({ page }) => {
    const source = await installControlledSources(page);
    await source.getByRole("button", { name: "Refresh balance", exact: true }).click();
    await expect(source.locator('[data-metric="balance"]')).toContainText("$22.00");
    await page.evaluate((change) => {
      const state = (window as ControlledWindow).__statsControl;
      if (change === "revision") state.revisions.source_synthetic_2 = 2;
      else state.removed.push("source_synthetic_2");
    }, change);
    await refreshSnapshot(page);
    await expect(source.locator('[data-metric="balance"]')).toContainText("$22.00");
    expect(await page.evaluate(() => (window as ControlledWindow).__statsControl.calls)).toEqual(["source_synthetic"]);
  });
}

test("another source's revision does not cancel a pending balance refresh", async ({ page }) => {
  const source = await installControlledSources(page);
  await page.evaluate(() => { (window as ControlledWindow).__statsControl.hold = true; });
  await source.getByRole("button", { name: "Refresh balance", exact: true }).click();
  await expect(source.locator(".source-stats-panel")).toHaveAttribute("aria-busy", "true");
  await page.evaluate(() => { (window as ControlledWindow).__statsControl.revisions.source_synthetic_2 = 2; });
  await refreshSnapshot(page);
  await expect(source.locator(".source-stats-panel")).toHaveAttribute("aria-busy", "true");
  await page.evaluate(() => (window as ControlledWindow).__statsControl.release!());
  await expect(source.locator('[data-metric="balance"]')).toContainText("$22.00");
  await expect(source.locator(".source-stats-panel")).not.toHaveAttribute("aria-busy");
});

test("an unrelated revision does not reread a source without a cached host observation", async ({ page }) => {
  const source = await installControlledSources(page);
  await source.getByRole("button", { name: "Refresh balance", exact: true }).click();
  await expect(source.locator('[data-metric="balance"]')).toContainText("$22.00");
  await page.evaluate(() => {
    const state = (window as ControlledWindow).__statsControl;
    state.uncached.push("source_synthetic");
    state.revisions.source_synthetic_2 = 2;
  });
  await refreshSnapshot(page);
  expect(await page.evaluate(() => (window as ControlledWindow).__statsControl.calls)).toEqual(["source_synthetic"]);
  await expect(source.locator('[data-metric="balance"]')).toContainText("$22.00");
});

test("retrying a failed read retains its stale warning until success", async ({ page }) => {
  const source = await installControlledSources(page);
  const balance = source.locator('[data-metric="balance"]');
  await page.evaluate(() => { (window as ControlledWindow).__statsControl.fail = true; });
  await source.getByRole("button", { name: "Refresh balance", exact: true }).click();
  await expect(balance).toHaveAttribute("data-relay-tooltip", "Last known value — Temporarily unavailable");
  await expect(source.locator(".source-stats-panel")).not.toHaveAttribute("aria-busy");
  await page.evaluate(() => {
    const state = (window as ControlledWindow).__statsControl;
    state.fail = false;
    state.hold = true;
  });
  await source.getByRole("button", { name: "Refresh balance", exact: true }).click();
  await expect(source.locator(".source-stats-panel")).toHaveAttribute("aria-busy", "true");
  await expect(balance).toHaveAttribute("data-relay-tooltip", "Last known value — Temporarily unavailable");
  await page.evaluate(() => (window as ControlledWindow).__statsControl.release!());
  await expect(balance).toContainText("$22.00");
  await expect(balance).not.toHaveAttribute("data-relay-tooltip");
});

test("changing the source itself still discards its previous pending response", async ({ page }) => {
  const source = await installControlledSources(page);
  await page.evaluate(() => { (window as ControlledWindow).__statsControl.hold = true; });
  await source.getByRole("button", { name: "Refresh balance", exact: true }).click();
  await expect(source.locator(".source-stats-panel")).toHaveAttribute("aria-busy", "true");
  await page.evaluate(() => {
    const state = (window as ControlledWindow).__statsControl;
    state.endpoints.source_synthetic = "https://replacement.example.test/v1";
    state.revisions.source_synthetic = 2;
    state.observations.source_synthetic = { ...state.observations.source_synthetic!, balanceMicroUsd: 5_000_000, asOfMs: 50 };
  });
  await refreshSnapshot(page);
  await expect(source.locator('[data-metric="balance"]')).toContainText("$5.00");
  await page.evaluate(() => (window as ControlledWindow).__statsControl.release!());
  await expect.poll(() => page.evaluate(() => (window as ControlledWindow).__statsControl.completed)).toBe(1);
  await refreshSnapshot(page);
  await expect(source.locator('[data-metric="balance"]')).toContainText("$5.00");
});
