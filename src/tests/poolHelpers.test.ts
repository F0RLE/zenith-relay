import { describe, expect, test } from "bun:test";
import type { AccountSummary, RuntimeSnapshot, SourceSummary } from "../src/features/relay/api/types";
import {
  clampRoutingCount,
  comparePoolMembers,
  currentPoolModelSummaries,
  groupModelSummaries,
  mergeSubscriptionPlanOrder,
  modelSummaries,
  operationalModelSummaries,
  sourceOrderForRole,
  sourceRoutingStages,
  subscriptionPlanGroups,
  toggle,
} from "../src/features/relay/poolHelpers";
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
        routingStrategy: "adaptive",
        defaultServiceTier: "standard",
      },
      accounts: [account({ models: ["GPT-TEST"] })],
    }));
    expect(fallback[0]).toMatchObject({ id: "gpt-test", memberCount: 1, enabled: true });
    expect(groupModelSummaries(fallback, [account({ models: ["GPT-TEST"] })]).map((group) => group.provider)).toEqual(["openai"]);
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

  test("keeps Model Rules limited to models with an active pool route", () => {
    const nowMs = Date.now();
    const snapshot = runtime({
      gateway: {
        running: true,
        baseUrl: "http://127.0.0.1:0",
        candidateCount: 1,
        visibleModelIds: [],
        maxRetryCandidates: 3,
        routingStrategy: "adaptive",
        defaultServiceTier: "standard",
        models: [
          { id: "gpt-live", enabled: true, memberCount: 1, codexVisible: true, codexDisplayName: "gpt-live", catalogProvider: null, catalogFamily: null, inputMicroUsdPerMillion: null, outputMicroUsdPerMillion: null, customPrice: false },
          { id: "gpt-cooled", enabled: true, memberCount: 1, codexVisible: true, codexDisplayName: "gpt-cooled", catalogProvider: null, catalogFamily: null, inputMicroUsdPerMillion: null, outputMicroUsdPerMillion: null, customPrice: false },
          { id: "gpt-unavailable", enabled: true, memberCount: 1, codexVisible: true, codexDisplayName: "gpt-unavailable", catalogProvider: null, catalogFamily: null, inputMicroUsdPerMillion: null, outputMicroUsdPerMillion: null, customPrice: false },
          { id: "gpt-excluded", enabled: true, memberCount: 1, codexVisible: true, codexDisplayName: "gpt-excluded", catalogProvider: null, catalogFamily: null, inputMicroUsdPerMillion: null, outputMicroUsdPerMillion: null, customPrice: false },
          { id: "gpt-disabled-rule", enabled: false, memberCount: 1, codexVisible: false, codexDisplayName: "gpt-disabled-rule", catalogProvider: null, catalogFamily: null, inputMicroUsdPerMillion: null, outputMicroUsdPerMillion: null, customPrice: false },
        ],
        routingOrder: [
          { candidateId: "live-source", kind: "api_source", available: true, inFlight: 0, lastUsedAtMs: null, nextRetryAtMs: null, halfOpen: false, dispatches: 0, modelRetries: [] },
          { candidateId: "cooled-account", kind: "oauth_account", available: true, inFlight: 0, lastUsedAtMs: null, nextRetryAtMs: nowMs + 60_000, halfOpen: false, dispatches: 0, modelRetries: [{ model: "gpt-cooled", retryAtMs: nowMs + 60_000 }] },
          { candidateId: "unavailable-account", kind: "oauth_account", available: false, inFlight: 0, lastUsedAtMs: null, nextRetryAtMs: nowMs + 60_000, halfOpen: false, dispatches: 0, modelRetries: [] },
          { candidateId: "excluded-account", kind: "oauth_account", available: true, inFlight: 0, lastUsedAtMs: null, nextRetryAtMs: null, halfOpen: false, dispatches: 0, modelRetries: [] },
        ],
      },
      sources: [source({
        id: "live-source",
        models: ["gpt-live", "gpt-unbound", "gpt-disabled-rule"],
        protocolBindings: [{ wireApi: "responses", modelIds: ["gpt-live", "gpt-disabled-rule"] }],
      })],
      accounts: [
        account({ id: "cooled-account", models: ["gpt-cooled"] }),
        account({ id: "unavailable-account", models: ["gpt-unavailable"], operationalStatus: "unavailable" }),
        account({ id: "excluded-account", models: ["gpt-excluded"], excludedModels: ["gpt-*"] }),
      ],
    });

    expect(operationalModelSummaries(snapshot).map((model) => model.id)).toEqual([
      "gpt-live",
      "gpt-disabled-rule",
    ]);
  });

  test("keeps a pooled API source model visible when the runtime order is not reported", () => {
    const snapshot = runtime({
      gateway: {
        running: true,
        baseUrl: "http://127.0.0.1:0",
        candidateCount: 1,
        visibleModelIds: ["provider-model"],
        maxRetryCandidates: 3,
        routingStrategy: "adaptive",
        defaultServiceTier: "standard",
        models: [{ id: "provider-model", enabled: true, memberCount: 1, codexVisible: false, codexDisplayName: "Provider model", catalogProvider: null, catalogFamily: null, inputMicroUsdPerMillion: null, outputMicroUsdPerMillion: null, customPrice: false }],
        routingOrder: [],
      },
      sources: [source({ id: "provider", models: ["provider-model"], protocolBindings: [{ wireApi: "responses", modelIds: ["provider-model"] }] })],
    });

    expect(operationalModelSummaries(snapshot).map((model) => model.id)).toEqual(["provider-model"]);
  });

  test("merges a lagging gateway catalog and keeps rotation members visible", () => {
    const snapshot = runtime({
      gateway: {
        running: true,
        baseUrl: "http://127.0.0.1:0",
        candidateCount: 2,
        visibleModelIds: ["shared-model"],
        maxRetryCandidates: 3,
        routingStrategy: "adaptive",
        defaultServiceTier: "standard",
        // The derived catalog has not caught up with the account/source
        // refresh yet, so both member-only models are intentionally absent.
        models: [{ id: "shared-model", enabled: true, memberCount: 1, codexVisible: true, codexDisplayName: "Shared", catalogProvider: null, catalogFamily: null, inputMicroUsdPerMillion: null, outputMicroUsdPerMillion: null, customPrice: false }],
        // A transient snapshot can mark both candidates unavailable while
        // their management status is already back in rotation.
        routingOrder: [
          { candidateId: "provider", kind: "api_source", available: false, inFlight: 0, lastUsedAtMs: null, nextRetryAtMs: null, halfOpen: false, dispatches: 0 },
          { candidateId: "account-a", kind: "oauth_account", available: false, inFlight: 0, lastUsedAtMs: null, nextRetryAtMs: null, halfOpen: false, dispatches: 0 },
        ],
      },
      sources: [source({
        id: "provider",
        models: ["shared-model", "source-only"],
        protocolBindings: [{ wireApi: "responses", adapter: "native", modelIds: [] }],
      })],
      accounts: [account({ id: "account-a", models: ["shared-model", "account-only"] })],
    });

    expect(operationalModelSummaries(snapshot).map((model) => model.id)).toEqual([
      "shared-model",
      "source-only",
      "account-only",
    ]);
  });

  test("hides an unavailable account while retaining an exact model cooldown boundary", () => {
    const nowMs = Date.now();
    const snapshot = runtime({
      gateway: {
        running: true,
        baseUrl: "http://127.0.0.1:0",
        candidateCount: 1,
        visibleModelIds: ["cooling", "healthy"],
        maxRetryCandidates: 3,
        routingStrategy: "adaptive",
        defaultServiceTier: "standard",
        models: [
          { id: "cooling", enabled: true, memberCount: 1, codexVisible: true, codexDisplayName: "Cooling", catalogProvider: null, catalogFamily: null, inputMicroUsdPerMillion: null, outputMicroUsdPerMillion: null, customPrice: false },
          { id: "healthy", enabled: true, memberCount: 1, codexVisible: true, codexDisplayName: "Healthy", catalogProvider: null, catalogFamily: null, inputMicroUsdPerMillion: null, outputMicroUsdPerMillion: null, customPrice: false },
          { id: "unavailable", enabled: true, memberCount: 1, codexVisible: true, codexDisplayName: "Unavailable", catalogProvider: null, catalogFamily: null, inputMicroUsdPerMillion: null, outputMicroUsdPerMillion: null, customPrice: false },
        ],
        routingOrder: [
          { candidateId: "account-a", kind: "oauth_account", available: false, inFlight: 0, lastUsedAtMs: null, nextRetryAtMs: null, halfOpen: false, dispatches: 0, modelRetries: [{ model: "cooling", retryAtMs: nowMs + 60_000 }] },
          { candidateId: "account-unavailable", kind: "oauth_account", available: false, inFlight: 0, lastUsedAtMs: null, nextRetryAtMs: null, halfOpen: false, dispatches: 0, modelRetries: [] },
        ],
      },
      accounts: [
        account({ id: "account-a", models: ["cooling", "healthy"] }),
        account({ id: "account-unavailable", models: ["unavailable"], operationalStatus: "unavailable" }),
      ],
    });

    expect(operationalModelSummaries(snapshot).map((model) => model.id)).toEqual(["healthy"]);
  });

  test("hides every model during a future whole-candidate cooldown", () => {
    const snapshot = runtime({
      gateway: {
        running: true,
        baseUrl: "http://127.0.0.1:0",
        candidateCount: 1,
        visibleModelIds: ["temporarily-unavailable"],
        maxRetryCandidates: 3,
        routingStrategy: "adaptive",
        defaultServiceTier: "standard",
        models: [{ id: "temporarily-unavailable", enabled: true, memberCount: 1, codexVisible: true, codexDisplayName: "Temporary", catalogProvider: null, catalogFamily: null, inputMicroUsdPerMillion: null, outputMicroUsdPerMillion: null, customPrice: false }],
        routingOrder: [{ candidateId: "account-a", kind: "oauth_account", available: false, inFlight: 0, lastUsedAtMs: null, nextRetryAtMs: Date.now() + 60_000, halfOpen: false, dispatches: 0, modelRetries: [] }],
      },
      accounts: [account({ id: "account-a", models: ["temporarily-unavailable"] })],
    });

    expect(operationalModelSummaries(snapshot)).toEqual([]);
  });

  test("falls back to the API source catalog while the derived gateway catalog is empty", () => {
    const snapshot = runtime({
      gateway: {
        running: true,
        baseUrl: "http://127.0.0.1:0",
        candidateCount: 1,
        visibleModelIds: [],
        maxRetryCandidates: 3,
        routingStrategy: "adaptive",
        defaultServiceTier: "standard",
        models: [],
        routingOrder: [],
      },
      sources: [source({ id: "provider", models: ["provider-model"], protocolBindings: [{ wireApi: "responses", modelIds: ["provider-model"] }] })],
    });

    expect(operationalModelSummaries(snapshot).map((model) => model.id)).toEqual(["provider-model"]);
  });

  test("expands a sole empty native source binding and tolerates a partial route snapshot", () => {
    const snapshot = runtime({
      gateway: {
        running: true,
        baseUrl: "http://127.0.0.1:0",
        candidateCount: 2,
        visibleModelIds: ["provider-model", "account-model"],
        maxRetryCandidates: 3,
        routingStrategy: "adaptive",
        defaultServiceTier: "standard",
        models: [
          { id: "provider-model", enabled: true, memberCount: 1, codexVisible: false, codexDisplayName: "Provider model", catalogProvider: null, catalogFamily: null, inputMicroUsdPerMillion: null, outputMicroUsdPerMillion: null, customPrice: false },
          { id: "account-model", enabled: true, memberCount: 1, codexVisible: true, codexDisplayName: "Account model", catalogProvider: null, catalogFamily: null, inputMicroUsdPerMillion: null, outputMicroUsdPerMillion: null, customPrice: false },
        ],
        // The account route is omitted from this older/partial snapshot. The
        // account status is still rotation, so its model must remain visible.
        routingOrder: [{ candidateId: "provider", kind: "api_source", available: true, inFlight: 0, lastUsedAtMs: null, nextRetryAtMs: null, halfOpen: false, dispatches: 0 }],
      },
      sources: [source({
        id: "provider",
        models: ["provider-model"],
        protocolBindings: [{ wireApi: "responses", adapter: "native", modelIds: [] }],
      })],
      accounts: [account({ id: "account-a", models: ["account-model"] })],
    });

    expect(operationalModelSummaries(snapshot).map((model) => model.id)).toEqual([
      "provider-model",
      "account-model",
    ]);
  });

  test("does not expand an empty native binding when another route is configured", () => {
    const snapshot = runtime({
      gateway: {
        running: true,
        baseUrl: "http://127.0.0.1:0",
        candidateCount: 1,
        visibleModelIds: ["unconfirmed-model", "confirmed-model"],
        maxRetryCandidates: 3,
        routingStrategy: "adaptive",
        defaultServiceTier: "standard",
        models: [
          { id: "unconfirmed-model", enabled: true, memberCount: 1, codexVisible: false, codexDisplayName: "Unconfirmed", catalogProvider: null, catalogFamily: null, inputMicroUsdPerMillion: null, outputMicroUsdPerMillion: null, customPrice: false },
          { id: "confirmed-model", enabled: true, memberCount: 1, codexVisible: false, codexDisplayName: "Confirmed", catalogProvider: null, catalogFamily: null, inputMicroUsdPerMillion: null, outputMicroUsdPerMillion: null, customPrice: false },
        ],
        routingOrder: [
          { candidateId: "provider::messages", kind: "api_source", available: true, inFlight: 0, lastUsedAtMs: null, nextRetryAtMs: null, halfOpen: false, dispatches: 0 },
          { candidateId: "provider::responses", kind: "api_source", available: true, inFlight: 0, lastUsedAtMs: null, nextRetryAtMs: null, halfOpen: false, dispatches: 0 },
        ],
      },
      sources: [source({
        id: "provider",
        models: ["unconfirmed-model", "confirmed-model"],
        protocolBindings: [
          { wireApi: "responses", adapter: "native", modelIds: [] },
          { wireApi: "messages", adapter: "native", modelIds: ["confirmed-model"] },
        ],
      })],
    });

    expect(operationalModelSummaries(snapshot).map((model) => model.id)).toEqual(["confirmed-model"]);
  });

  test("keeps selection and numeric policy inputs bounded", () => {
    expect(toggle(["a"], "a")).toEqual([]);
    expect(toggle(["a"], "b")).toEqual(["a", "b"]);
    expect(clampRoutingCount("0")).toBe(1);
    expect(clampRoutingCount("99")).toBe(8);
    expect(clampRoutingCount("bad")).toBe(1);
  });

  test("keeps backend routing order even when a member is unavailable", () => {
    const healthy = { ...account({ id: "healthy", label: "Z" }), kind: "account" as const };
    const unavailable = { ...account({ id: "unavailable", label: "A", operationalStatus: "unavailable" }), kind: "account" as const };
    const order = new Map([["unavailable", 0], ["healthy", 1]]);
    expect(comparePoolMembers(unavailable, healthy, order)).toBeLessThan(0);
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
