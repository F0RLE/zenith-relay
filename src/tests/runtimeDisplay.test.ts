import { describe, expect, test } from "bun:test";
import type { AccountSummary, RuntimeSnapshot } from "../src/features/relay/api/types";
import { createRuntimeAccountLabelProjector, projectRuntimeAccountLabels } from "../src/features/relay/state/runtimeDisplay";

function account(overrides: Partial<AccountSummary> = {}): AccountSummary {
  return {
    id: "account",
    label: "Imported account name",
    identityHint: "a1b2c3d4e5f6",
    enabled: true,
    inPool: true,
    draining: false,
    authState: { state: "ready" },
    health: "ready",
    operationalStatus: "rotation",
    models: [],
    allowedModels: [],
    excludedModels: [],
    priority: 1,
    weight: 1,
    apiEquivalent: { microUsd: 0, unpricedTokens: 0 },
    subscription: { planType: null, activeUntilMs: null, status: "active", updatedAtMs: null },
    quota: {},
    quotaRefreshStatus: "updated",
    secretAvailable: true,
    lastErrorCode: null,
    ...overrides,
  };
}

function runtime(accounts: AccountSummary[]): RuntimeSnapshot {
  return {
    schemaVersion: 1,
    gateway: {
      running: true,
      baseUrl: "http://127.0.0.1:0",
      candidateCount: accounts.length,
      visibleModelIds: [],
      maxRetryCandidates: 3,

      defaultServiceTier: "standard",
    },
    platform: "test",
    capabilities: { features: [] },
    sources: [],
    accounts,
    automations: [],
    wakeHistory: [],
    warnings: [],
  };
}

describe("runtime account display projection", () => {
  test("uses the resolved display name without mutating the runtime snapshot", () => {
    const snapshot = runtime([account({ id: "one" }), account({ id: "two", label: "Fallback" })]);

    const projected = projectRuntimeAccountLabels(
      snapshot,
      (accountId, fallbackLabel) => accountId === "one" ? "Visible account" : fallbackLabel ?? null,
    );

    expect(projected).not.toBe(snapshot);
    expect(snapshot.accounts[0]).toMatchObject({ label: "Imported account name", identityHint: "a1b2c3d4e5f6" });
    expect(projected?.accounts).toMatchObject([
      { id: "one", label: "Visible account", identityHint: "Visible account" },
      { id: "two", label: "Fallback", identityHint: "Fallback" },
    ]);
  });

  test("preserves the null runtime state", () => {
    expect(projectRuntimeAccountLabels(null, () => "unused")).toBeNull();
  });

  test("keeps account references stable when display labels do not change", () => {
    const first = account({ id: "one", label: "Visible", identityHint: "Visible" });
    const second = account({ id: "two", label: "Fallback", identityHint: "Fallback" });
    const snapshot = runtime([first, second]);

    const projected = projectRuntimeAccountLabels(snapshot, (_accountId, fallbackLabel) => fallbackLabel ?? null);

    expect(projected).not.toBe(snapshot);
    expect(projected?.accounts).toBe(snapshot.accounts);
    expect(projected?.accounts[0]).toBe(first);
    expect(projected?.accounts[1]).toBe(second);
  });

  test("activity-only snapshots reuse one account projection instead of cloning every account again", () => {
    const snapshot = runtime(Array.from({ length: 500 }, (_, index) => account({ id: `account-${index}` })));
    let baselineCalls = 0;
    let memoizedCalls = 0;
    const baselineName = () => { baselineCalls += 1; return "Hidden account"; };
    const memoizedName = () => { memoizedCalls += 1; return "Hidden account"; };
    const project = createRuntimeAccountLabelProjector();
    const initial = project(snapshot, memoizedName);
    for (let revision = 1; revision <= 100; revision += 1) {
      const updated = { ...snapshot, gateway: { ...snapshot.gateway, candidateCount: revision } };
      projectRuntimeAccountLabels(updated, baselineName);
      const displayed = project(updated, memoizedName);
      expect(displayed?.accounts).toBe(initial?.accounts);
      expect(displayed?.gateway).toBe(updated.gateway);
    }
    expect(baselineCalls).toBe(50_000);
    expect(memoizedCalls).toBe(500);
    expect(snapshot.accounts[0].label).toBe("Imported account name");
  });

  test("new account facts invalidate the projection even if labels stay unchanged", () => {
    const project = createRuntimeAccountLabelProjector();
    const snapshot = runtime([account()]);
    const name = () => "Hidden account";
    const first = project(snapshot, name);
    const changed = runtime([{ ...snapshot.accounts[0], inPool: false, health: "requires_reauth" }]);
    const second = project(changed, name);
    expect(second?.accounts).not.toBe(first?.accounts);
    expect(second?.accounts[0]).toMatchObject({ inPool: false, health: "requires_reauth" });
  });

  test("privacy changes and mode clearing cannot reuse old visible identities", () => {
    const project = createRuntimeAccountLabelProjector();
    const snapshot = runtime([account()]);
    const visibleName = () => "Synthetic visible name";
    let hiddenCalls = 0;
    const hiddenName = () => { hiddenCalls += 1; return "Hidden account"; };
    expect(project(snapshot, visibleName)?.accounts[0].label).toBe("Synthetic visible name");
    expect(project(snapshot, hiddenName)?.accounts[0].label).toBe("Hidden account");
    expect(project(null, hiddenName)).toBeNull();
    expect(project(snapshot, hiddenName)?.accounts[0].label).toBe("Hidden account");
    expect(hiddenCalls).toBe(2);
  });
});
