import { describe, expect, test } from "bun:test";
import type { AccountSummary, RuntimeSnapshot, SourceSummary } from "../src/features/relay/api/types";
import {
  comparePoolMembers,
  toggle,
} from "../src/features/relay/poolHelpers";
import {
  currentPoolModelSummaries,
  groupModelSummaries,
  modelSummaries,
} from "../src/features/relay/modelSummaries";
import { applyRuntimeActivity, applyRuntimeActivities, reconcileRuntimeActivityOverlay, routingOrderPositions, runtimeCandidateForMember, upcomingModelRetries } from "../src/features/relay/routingOrder";

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
  test("normalizes model metadata and preserves fallback catalog counts", () => {
    const explicit = runtime({
      gateway: {
        running: true,
        baseUrl: "http://127.0.0.1:0",
        candidateCount: 1,
        visibleModelIds: ["unused"],
        maxRetryCandidates: 3,

        defaultServiceTier: "standard",
        models: [{ id: "gpt-test", enabled: false, memberCount: 2, codexVisible: true, codexDisplayName: "", catalogProvider: "openai", catalogFamily: "gpt", inputMicroUsdPerMillion: null, outputMicroUsdPerMillion: null, customPrice: false }],
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

        defaultServiceTier: "standard",
      },
      accounts: [account({ models: ["GPT-TEST"] })],
    }));
    expect(fallback[0]).toMatchObject({ id: "gpt-test", memberCount: 1, enabled: true });
    expect(groupModelSummaries(fallback, [account({ models: ["GPT-TEST"] })]).map((group) => group.provider)).toEqual(["openai"]);
  });

  test("enriches a sparse gateway row with later reasoning and pricing data", () => {
    const snapshot = runtime({
      gateway: {
        running: true,
        baseUrl: "http://127.0.0.1:0",
        candidateCount: 1,
        visibleModelIds: [],
        maxRetryCandidates: 3,

        defaultServiceTier: "standard",
        models: [
          { id: "gpt-test", enabled: true, memberCount: 1, codexVisible: true, codexDisplayName: "gpt-test", catalogProvider: null, catalogFamily: null, inputMicroUsdPerMillion: null, outputMicroUsdPerMillion: null, customPrice: false },
          { id: "GPT-TEST", enabled: true, memberCount: 2, codexVisible: true, codexDisplayName: "GPT Test", catalogProvider: "openai", catalogFamily: "gpt", inputMicroUsdPerMillion: 10, outputMicroUsdPerMillion: 20, customPrice: true, reasoningLevels: ["low", "high"], reasoningSupportedLevels: ["low", "high"], reasoningConfigurable: true },
        ],
      },
    });

    expect(modelSummaries(snapshot)[0]).toMatchObject({
      id: "gpt-test",
      memberCount: 2,
      catalogProvider: "openai",
      inputMicroUsdPerMillion: 10,
      outputMicroUsdPerMillion: 20,
      reasoningLevels: ["low", "high"],
      reasoningConfigurable: true,
    });
  });

  test("merges protocol routes, reasoning levels, and both Claude cache write prices", () => {
    const snapshot = runtime({
      gateway: {
        running: true,
        baseUrl: "http://127.0.0.1:0",
        candidateCount: 1,
        visibleModelIds: [],
        maxRetryCandidates: 3,

        defaultServiceTier: "standard",
        models: [
          {
            id: "claude-test",
            enabled: true,
            memberCount: 1,
            codexVisible: true,
            codexDisplayName: "claude-test",
            inputMicroUsdPerMillion: 100,
            outputMicroUsdPerMillion: 300,
            customPrice: false,
            reasoningLevels: ["low"],
            reasoningSupportedLevels: ["low"],
            reasoningAllowedLevels: ["low"],
            protocolRoutes: [{
              clientWireApi: "responses",
              upstreamWireApi: "messages",
              features: { text: "confirmed" },
              reasoningEfforts: ["low"],
            }],
          },
          {
            id: "CLAUDE-TEST",
            enabled: true,
            memberCount: 1,
            codexVisible: true,
            codexDisplayName: "Claude Test",
            inputMicroUsdPerMillion: null,
            outputMicroUsdPerMillion: null,
            customPrice: false,
            cacheWrite5mMicroUsdPerMillion: 125,
            cacheWrite1hMicroUsdPerMillion: 110,
            reasoningLevels: ["high", "medium"],
            reasoningSupportedLevels: ["high", "medium"],
            reasoningAllowedLevels: ["high", "medium"],
            protocolRoutes: [{
              clientWireApi: "messages",
              upstreamWireApi: "messages",
              features: { function_tools: "confirmed" },
              reasoningEfforts: ["high", "medium"],
            }],
          },
        ],
      },
    });

    expect(modelSummaries(snapshot)[0]).toMatchObject({
      reasoningLevels: ["low", "medium", "high"],
      reasoningSupportedLevels: ["low", "medium", "high"],
      reasoningAllowedLevels: ["low", "medium", "high"],
      cacheWrite5mMicroUsdPerMillion: 125,
      cacheWrite1hMicroUsdPerMillion: 110,
      protocolRoutes: [
        { clientWireApi: "responses", upstreamWireApi: "messages" },
        { clientWireApi: "messages", upstreamWireApi: "messages" },
      ],
    });
  });

  test("counts physical pool members once and preserves complete backend metadata", () => {
    const snapshot = runtime({
      sources: [
        source({ id: "offline", enabled: false, secretAvailable: false, models: ["future"],
          protocolBindings: [{ wireApi: "messages", modelIds: ["FUTURE"] }] }),
        source({ id: "outside", inPool: false, models: ["future"] }),
      ],
      accounts: [account({ models: ["FUTURE"], secretAvailable: false })],
    });
    snapshot.gateway.models = [{ id: "future", enabled: false, memberCount: 2,
      catalogName: "Future Model", catalogProvider: "synthetic", codexDisplayName: "Future Model",
      inputMicroUsdPerMillion: 12, outputMicroUsdPerMillion: 34, customPrice: false,
      reasoningSupportedLevels: ["low", "high"], reasoningAllowedLevels: ["high"],
      reasoningLevels: ["high"], reasoningConfigurable: true, protocolRoutes: [],
    }];
    expect(currentPoolModelSummaries(snapshot)).toHaveLength(1);
    expect(currentPoolModelSummaries(snapshot)[0]).toMatchObject({
      id: "future", enabled: false, memberCount: 2, catalogName: "Future Model",
      inputMicroUsdPerMillion: 12, outputMicroUsdPerMillion: 34,
      reasoningSupportedLevels: ["low", "high"], reasoningLevels: ["high"], reasoningConfigurable: true,
    });
  });

  test("keeps a binding-only pooled source model in the saved order inventory", () => {
    const snapshot = runtime({
      sources: [source({
        models: [],
        protocolBindings: [{ wireApi: "responses", adapter: "native", modelIds: ["binding-only"] }],
      })],
    });

    expect(currentPoolModelSummaries(snapshot).map((model) => model.id)).toEqual(["binding-only"]);
  });

  test("keeps every pooled provider model and its catalog group when the gateway rows lag", () => {
    const snapshot = runtime({
      gateway: {
        running: true,
        baseUrl: "http://127.0.0.1:0",
        candidateCount: 1,
        visibleModelIds: ["gpt-live"],
        maxRetryCandidates: 3,

        defaultServiceTier: "standard",
        // The derived rows currently contain only the native account model.
        models: [{ id: "gpt-live", enabled: true, memberCount: 1, codexVisible: true, codexDisplayName: "GPT Live", catalogProvider: "openai", catalogFamily: "gpt", inputMicroUsdPerMillion: null, outputMicroUsdPerMillion: null, customPrice: false }],
        modelCatalog: {
          "gpt-live": { catalogProvider: "openai", catalogFamily: "gpt" },
          "claude-live": { catalogProvider: "anthropic", catalogFamily: "claude" },
          "grok-live": { catalogProvider: "xai", catalogFamily: "grok" },
        },
      },
      sources: [source({
        id: "provider",
        models: ["claude-live", "grok-live"],
        protocolBindings: [{ wireApi: "messages", adapter: "native", modelIds: ["claude-live"] }],
      })],
      accounts: [account({ id: "account-a", models: ["gpt-live"] })],
    });

    const models = currentPoolModelSummaries(snapshot);
    expect(models.map((model) => model.id)).toEqual(["gpt-live", "claude-live", "grok-live"]);
    expect(groupModelSummaries(models, snapshot.accounts).map((group) => [group.provider, group.items.map((model) => model.id)])).toEqual([
      ["openai", ["gpt-live"]],
      ["anthropic", ["claude-live"]],
      ["xai", ["grok-live"]],
    ]);
  });

  test("keeps selection and numeric policy inputs bounded", () => {
    expect(toggle(["a"], "a")).toEqual([]);
    expect(toggle(["a"], "b")).toEqual(["a", "b"]);
  });

  test("shows ready members before unavailable ones even with an older runtime order", () => {
    const healthy = { ...account({ id: "healthy", label: "Z" }), kind: "account" as const };
    const unavailable = { ...account({ id: "unavailable", label: "A", operationalStatus: "unavailable" }), kind: "account" as const };
    const order = new Map([["unavailable", 0], ["healthy", 1]]);
    expect(comparePoolMembers(unavailable, healthy, order)).toBeGreaterThan(0);
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

  test("keeps only future model retries in runtime display order", () => {
    const candidate = {
      candidateId: "account-a",
      kind: "oauth_account" as const,
      available: true,
      inFlight: 0,
      modelRetries: [
        { model: "later", retryAtMs: 900 },
        { model: "expired", retryAtMs: 100 },
        { model: "first", retryAtMs: 500 },
        { model: "second", retryAtMs: 500 },
      ],
      lastUsedAtMs: null,
      nextRetryAtMs: 500,
      halfOpen: false,
      dispatches: 0,
    };

    expect(upcomingModelRetries(candidate, 499)).toEqual([
      { model: "first", retryAtMs: 500 },
      { model: "second", retryAtMs: 500 },
      { model: "later", retryAtMs: 900 },
    ]);
  });

  test("applies an activity snapshot without mutating the previous order", () => {
    const order = [
      { candidateId: "account-a", kind: "oauth_account" as const, available: true, inFlight: 0, activeRequestCount: 0, activeModels: [], lastUsedAtMs: null, nextRetryAtMs: null, halfOpen: false, dispatches: 0 },
      { candidateId: "source-b::messages", kind: "api_source" as const, available: true, inFlight: 0, activeRequestCount: 0, activeModels: [], lastUsedAtMs: null, nextRetryAtMs: null, halfOpen: false, dispatches: 0 },
    ];
    const next = applyRuntimeActivity(order, {
      revision: 1,
      candidateId: "source-b::messages",
      inFlight: 2,
      activeRequestCount: 2,
      activeModels: [{ model: "claude-opus", requestCount: 2 }],
    });

    expect(next).not.toBe(order);
    expect(next[0]).toMatchObject({ candidateId: "source-b::messages", inFlight: 2, activeRequestCount: 2, activeModels: [{ model: "claude-opus", requestCount: 2 }] });
    expect(next[1]).toMatchObject({ candidateId: "account-a", inFlight: 0, activeRequestCount: 0, activeModels: [] });
    expect(order[1]).toMatchObject({ inFlight: 0, activeRequestCount: 0, activeModels: [] });
    expect(applyRuntimeActivity(order, { revision: 2, candidateId: "missing", inFlight: 1, activeRequestCount: 1, activeModels: [] })).toBe(order);
  });

  test("applies a burst once and keeps the last update per candidate", () => {
    const order = [
      { candidateId: "account-a", kind: "oauth_account" as const, available: true, inFlight: 0, activeRequestCount: 0, activeModels: [], lastUsedAtMs: null, nextRetryAtMs: null, halfOpen: false, dispatches: 0 },
      { candidateId: "source-b", kind: "api_source" as const, available: true, inFlight: 0, activeRequestCount: 0, activeModels: [], lastUsedAtMs: null, nextRetryAtMs: null, halfOpen: false, dispatches: 0 },
    ];
    const next = applyRuntimeActivities(order, [
      { revision: 1, candidateId: "source-b", inFlight: 1, activeRequestCount: 1, activeModels: [] },
      { revision: 2, candidateId: "source-b", inFlight: 0, activeRequestCount: 0, activeModels: [] },
      { revision: 3, candidateId: "account-a", inFlight: 2, activeRequestCount: 2, activeModels: [] },
    ]);

    expect(next.map((candidate) => candidate.candidateId)).toEqual(["account-a", "source-b"]);
    expect(next[0]).toMatchObject({ inFlight: 2, activeRequestCount: 2 });
    expect(next[1]).toMatchObject({ inFlight: 0, activeRequestCount: 0 });
    expect(order[0]).toMatchObject({ inFlight: 0, activeRequestCount: 0 });
  });

  test("drops a stale release tombstone when a fresh snapshot reports activity", () => {
    const overlay = new Map([
      ["account-a", { revision: 2, candidateId: "account-a", inFlight: 0, activeRequestCount: 0, activeModels: [] }],
    ]);
    reconcileRuntimeActivityOverlay([
      { candidateId: "account-a", kind: "oauth_account", available: true, inFlight: 1, activeRequestCount: 1, activeModels: [{ model: "gpt-5.4", requestCount: 1 }], lastUsedAtMs: null, nextRetryAtMs: null, halfOpen: false, dispatches: 1 },
    ], overlay);
    expect(overlay.size).toBe(0);
  });

  test("positions a multi-protocol source by its active binding", () => {
    const order = routingOrderPositions([
      { candidateId: "source-a::responses", kind: "api_source", available: true, inFlight: 0, activeRequestCount: 0, activeModels: [], lastUsedAtMs: null, nextRetryAtMs: null, halfOpen: false, dispatches: 0 },
      { candidateId: "source-b", kind: "api_source", available: true, inFlight: 0, activeRequestCount: 0, activeModels: [], lastUsedAtMs: null, nextRetryAtMs: null, halfOpen: false, dispatches: 0 },
      { candidateId: "source-a::messages", kind: "api_source", available: true, inFlight: 1, activeRequestCount: 1, activeModels: [{ model: "claude-opus", requestCount: 1 }], lastUsedAtMs: null, nextRetryAtMs: null, halfOpen: false, dispatches: 1 },
    ]);

    expect(order.get("source-a")).toBe(2);
  });

  test("applies a release event to a stale base order", () => {
    const base = [
      { candidateId: "account-a", kind: "oauth_account" as const, available: true, inFlight: 0, activeRequestCount: 0, activeModels: [], lastUsedAtMs: null, nextRetryAtMs: null, halfOpen: false, dispatches: 0 },
      { candidateId: "source-b", kind: "api_source" as const, available: true, inFlight: 0, activeRequestCount: 0, activeModels: [], lastUsedAtMs: null, nextRetryAtMs: null, halfOpen: false, dispatches: 0 },
    ];
    const active = applyRuntimeActivity(base, {
      revision: 1,
      candidateId: "source-b",
      inFlight: 1,
      activeRequestCount: 1,
      activeModels: [{ model: "claude-opus", requestCount: 1 }],
    });
    expect(active[0].candidateId).toBe("source-b");

    // The base order can still contain the in-flight state when the release
    // event wins the race with the lightweight runtime poll. Replaying the
    // tombstone against the raw base must clear both the count and the move.
    const released = applyRuntimeActivity(base, {
      revision: 2,
      candidateId: "source-b",
      inFlight: 0,
      activeRequestCount: 0,
      activeModels: [],
    });
    expect(released.map((candidate) => candidate.candidateId)).toEqual(["account-a", "source-b"]);
    expect(released[1]).toMatchObject({ inFlight: 0, activeRequestCount: 0, activeModels: [] });
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
