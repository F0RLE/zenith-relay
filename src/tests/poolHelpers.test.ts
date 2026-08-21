import { describe, expect, test } from "bun:test";
import type { AccountSummary, RuntimeSnapshot, SourceSummary } from "../src/features/relay/api/types";
import {
  clampRoutingCount,
  comparePoolMembers,
  groupModelSummariesForLauncher,
  mergeSubscriptionPlanOrder,
  modelSummaries,
  sourceOrderForRole,
  sourceRoutingStages,
  subscriptionPlanGroups,
  toggle,
} from "../src/features/relay/poolHelpers";
import { routingOrderPositions, runtimeCandidateForMember } from "../src/features/relay/routingOrder";

function source(overrides: Partial<SourceSummary>): SourceSummary {
  return {
    id: "source",
    name: "Source",
    enabled: true,
    inPool: true,
    draining: false,
    operationalStatus: "rotation",
    baseUrl: "https://example.test/v1",
    wireApi: "responses",
    models: [],
    allowedModels: [],
    excludedModels: [],
    priority: 1,
    weight: 1,
    recoveryDelaySeconds: 0,
    apiEquivalent: { microUsd: 0, unpricedTokens: 0 },
    secretAvailable: true,
    lastErrorCode: null,
    ...overrides,
  };
}

function account(overrides: Partial<AccountSummary>): AccountSummary {
  return {
    id: "account",
    label: "Account",
    identityHint: "Account",
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

function runtime(overrides: Partial<RuntimeSnapshot>): RuntimeSnapshot {
  return {
    schemaVersion: 1,
    runtimeTarget: { kind: "local", connected: true, origin: null, serverId: null, version: null },
    gateway: {
      running: true,
      baseUrl: "http://127.0.0.1:0",
      candidateCount: 0,
      visibleModelIds: [],
      maxRetryCandidates: 3,
      routingStrategy: "adaptive",
      defaultServiceTier: "standard",
    },
    platform: "test",
    capabilities: { features: [] },
    sources: [],
    accounts: [],
    automations: [],
    wakeHistory: [],
    warnings: [],
    ...overrides,
  };
}

describe("pool helpers", () => {
  test("orders sources within a role and keeps the edited source visible", () => {
    const sources = [
      source({ id: "primary-a", name: "A", priority: 1_000_002 }),
      source({ id: "primary-b", name: "B", priority: 1_000_001 }),
      source({ id: "reserve", name: "Reserve", priority: -1_000_000 }),
    ];

    expect(sourceOrderForRole(sources, "primary", "primary-b")).toEqual([
      "primary-a",
      "primary-b",
    ]);
    expect(sourceOrderForRole(sources, "primary", "reserve")).toEqual([
      "primary-a",
      "primary-b",
      "reserve",
    ]);
  });

  test("recalculates routing stages for an unsaved role selection", () => {
    const stages = sourceRoutingStages(
      [source({ id: "one", priority: 1 }), source({ id: "two", priority: -1_000_000 })],
      [account({ id: "account-a" }), account({ id: "account-b", enabled: false })],
      "one",
      "reserve",
    );

    expect(stages).toEqual([
      { role: "primary", count: 0 },
      { role: "accounts", count: 1 },
      { role: "stabilizer", count: 0 },
      { role: "reserve", count: 2 },
    ]);
  });

  test("merges saved plan order without dropping newly available plans", () => {
    const groups = subscriptionPlanGroups([
      account({ id: "a", subscription: { planType: "plus", activeUntilMs: null, status: "active", updatedAtMs: null } }),
      account({ id: "b", subscription: { planType: "enterprise", activeUntilMs: null, status: "active", updatedAtMs: null } }),
      account({ id: "c", inPool: false, subscription: { planType: "free", activeUntilMs: null, status: "active", updatedAtMs: null } }),
    ], "Unknown");

    expect(mergeSubscriptionPlanOrder(groups, ["enterprise", "removed"])).toEqual([
      "enterprise",
      "plus",
    ]);
  });

  test("normalizes model metadata and preserves fallback catalog counts", () => {
    const explicit = runtime({
      gateway: {
        running: true,
        baseUrl: "http://127.0.0.1:0",
        candidateCount: 1,
        visibleModelIds: ["unused"],
        maxRetryCandidates: 3,
        routingStrategy: "adaptive",
        defaultServiceTier: "standard",
        models: [{ id: "gpt-test", enabled: false, memberCount: 2, codexVisible: true, codexDisplayName: "", catalogRank: null, inputMicroUsdPerMillion: null, outputMicroUsdPerMillion: null, customPrice: false }],
      },
    });
    expect(modelSummaries(explicit)[0]).toMatchObject({ codexDisplayName: "gpt-test", reasoningLevels: [], reasoningSupportedLevels: [], reasoningAllowedLevels: [], reasoningConfigurable: false });

    const fallback = modelSummaries(runtime({
      gateway: {
        running: true,
        baseUrl: "http://127.0.0.1:0",
        candidateCount: 1,
        visibleModelIds: ["gpt-test"],
        maxRetryCandidates: 3,
        routingStrategy: "adaptive",
        defaultServiceTier: "standard",
      },
      accounts: [account({ models: ["GPT-TEST"] })],
    }));
    expect(fallback[0]).toMatchObject({ id: "gpt-test", memberCount: 1, enabled: true });
    expect(groupModelSummariesForLauncher(fallback, []).map((group) => group.id)).toEqual(["openai"]);
  });

  test("keeps selection and numeric policy inputs bounded", () => {
    expect(toggle(["a"], "a")).toEqual([]);
    expect(toggle(["a"], "b")).toEqual(["a", "b"]);
    expect(clampRoutingCount("0")).toBe(1);
    expect(clampRoutingCount("99")).toBe(8);
    expect(clampRoutingCount("bad")).toBe(1);
  });

  test("sorts unavailable members after healthy routing candidates", () => {
    const healthy = { ...account({ id: "healthy", label: "Z" }), kind: "account" as const };
    const unavailable = { ...account({ id: "unavailable", label: "A", operationalStatus: "unavailable" }), kind: "account" as const };
    const order = new Map([["unavailable", 0], ["healthy", 1]]);
    expect(comparePoolMembers(healthy, unavailable, order)).toBeLessThan(0);
  });

  test("sorts a multi-protocol source by its first protocol candidate", () => {
    const order = routingOrderPositions([
      { candidateId: "zenith-api::responses", kind: "api_source", available: true, inFlight: 0, lastUsedAtMs: null, nextRetryAtMs: null, halfOpen: false, dispatches: 0 },
      { candidateId: "zenith-api::messages", kind: "api_source", available: true, inFlight: 0, lastUsedAtMs: null, nextRetryAtMs: null, halfOpen: false, dispatches: 0 },
      { candidateId: "gpt-pro", kind: "api_source", available: true, inFlight: 0, lastUsedAtMs: null, nextRetryAtMs: null, halfOpen: false, dispatches: 0 },
    ]);

    expect(order.get("zenith-api")).toBe(0);
    expect(comparePoolMembers(
      { ...source({ id: "zenith-api", name: "Zenith API" }), kind: "source" },
      { ...source({ id: "gpt-pro", name: "GPT PRO" }), kind: "source" },
      order,
    )).toBeLessThan(0);
  });

  test("aggregates runtime state for a multi-protocol source card", () => {
    const state = runtimeCandidateForMember("zenith-api", "api_source", [
      { candidateId: "zenith-api::responses", kind: "api_source", available: false, inFlight: 1, activeRequestCount: 1, activeModels: [{ model: "gpt-test", requestCount: 1 }], modelRetries: [{ model: "gpt-test", retryAtMs: 500 }], lastUsedAtMs: 10, nextRetryAtMs: 500, halfOpen: false, dispatches: 2 },
      { candidateId: "zenith-api::messages", kind: "api_source", available: true, inFlight: 2, activeRequestCount: 2, activeModels: [{ model: "gpt-test", requestCount: 2 }], modelRetries: [{ model: "gpt-test", retryAtMs: 900 }, { model: "gpt-other", retryAtMs: 700 }], lastUsedAtMs: 20, nextRetryAtMs: 900, halfOpen: true, dispatches: 3 },
    ]);

    expect(state).toMatchObject({ candidateId: "zenith-api", available: true, inFlight: 3, activeRequestCount: 3, lastUsedAtMs: 20, nextRetryAtMs: 500, halfOpen: true, dispatches: 5 });
    expect(state?.activeModels).toEqual([{ model: "gpt-test", requestCount: 3 }]);
    expect(state?.modelRetries).toEqual([{ model: "gpt-test", retryAtMs: 500 }, { model: "gpt-other", retryAtMs: 700 }]);
  });

  test("uses only the Responses route for a pooled source", () => {
    const state = runtimeCandidateForMember("zenith-api", "api_source", [
      { candidateId: "zenith-api::messages", kind: "api_source", available: true, inFlight: 0, lastUsedAtMs: null, nextRetryAtMs: null, halfOpen: false, dispatches: 1 },
      { candidateId: "zenith-api::responses", kind: "api_source", available: false, inFlight: 0, lastUsedAtMs: null, nextRetryAtMs: 2_000, halfOpen: false, dispatches: 1 },
    ], "responses", "messages");

    expect(state).toMatchObject({ available: false, nextRetryAtMs: 2_000 });
  });

  test("does not treat a legacy Messages source candidate as a pooled Responses route", () => {
    const state = runtimeCandidateForMember("messages-source", "api_source", [
      { candidateId: "messages-source", kind: "api_source", available: true, inFlight: 0, lastUsedAtMs: null, nextRetryAtMs: null, halfOpen: false, dispatches: 1 },
    ], "responses", "messages");

    expect(state).toBeUndefined();
  });
});
