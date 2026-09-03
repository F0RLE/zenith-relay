import { describe, expect, test } from "bun:test";
import type { AccountSummary, RuntimeSnapshot } from "../src/features/relay/api/types";
import { projectRuntimeAccountLabels } from "../src/features/relay/state/runtimeDisplay";

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
    runtimeTarget: { kind: "local", connected: true, origin: null, serverId: null, version: null },
    gateway: {
      running: true,
      baseUrl: "http://127.0.0.1:0",
      candidateCount: accounts.length,
      visibleModelIds: [],
      maxRetryCandidates: 3,
      routingStrategy: "adaptive",
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
});
