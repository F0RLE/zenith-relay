import { describe, expect, test } from "bun:test";
import type { CandidateRuntimeSnapshot, RuntimeSnapshot } from "../src/features/relay/api/types";
import type { PoolMember } from "../src/features/relay/poolHelpers";
import { applyRuntimeActivities, compareRuntimeActivity, preferNewerRuntimeOrder, reconcileRuntimeActivityOverlay } from "../src/features/relay/routingOrder";
import {
  orderedPoolMembers,
  poolActivityState,
  poolMemberRuntimeStates,
  poolMembersFromRuntime,
  poolMemberSourceIds,
  poolMemberStatusCounts,
  poolProviderCreditsSummary,
  poolRoutingAvailability,
} from "../src/features/relay/pages/pool/poolMembersModel";

const member = (kind: "account" | "source", id: string, overrides: Record<string, unknown> = {}) => ({
  kind,
  id,
  inPool: true,
  enabled: true,
  operationalStatus: "rotation",
  secretAvailable: true,
  proxyAvailable: true,
  priority: kind === "source" ? 1 : undefined,
  models: [],
  lastErrorCode: null,
  ...(kind === "account" ? { authState: { state: "ready" }, quota: {}, quotaRefreshStatus: "updated" } : {}),
  ...overrides,
} as unknown as PoolMember);

const candidate = (candidateId: string, overrides: Record<string, unknown> = {}) => ({
  candidateId,
  kind: candidateId.startsWith("source") ? "api_source" : "oauth_account",
  available: true,
  inFlight: 0,
  activeRequestCount: 0,
  dispatches: 0,
  ...overrides,
} as CandidateRuntimeSnapshot);

describe("pool members model", () => {
  test("uses the backend preview even when the selected member is busy or not first", () => {
    const members = [member("account", "account"), member("source", "source-a")];
    const order = [
      candidate("account"),
      candidate("source-a", { nextForNewRequest: true, inFlight: 1, activeRequestCount: 1 }),
    ];
    const state = poolActivityState(members, poolMemberRuntimeStates(members, order), order);
    expect(state.nextMember?.id).toBe("source-a");
    expect(state.activeMembers.map((item) => item.id)).toEqual(["source-a"]);
  });

  test("does not invent a next member from legacy or ambiguous routing telemetry", () => {
    const members = [member("account", "account"), member("source", "source-a")];
    for (const preview of [undefined, false]) {
      const order = [candidate("account", { nextForNewRequest: preview }), candidate("source-a")];
      expect(poolActivityState(members, poolMemberRuntimeStates(members, order), order).nextMember).toBeNull();
    }
  });

  test("invalidates an old preview after activity and accepts a newer backend snapshot", () => {
    const members = [member("account", "account"), member("source", "source-a")];
    const order = [candidate("account", { nextForNewRequest: true, activityRevision: 3 }), candidate("source-a", { activityRevision: 3 })];
    const activity = { revision: 4, lastCandidateId: "source-a", candidates: {
      "source-a": { candidateId: "source-a", revision: 4, inFlight: 0, activeRequestCount: 0, activeModels: [] },
    } };
    expect(poolActivityState(members, poolMemberRuntimeStates(members, order), order, activity).nextMember).toBeNull();
    const fresh = order.map((item) => ({ ...item, activityRevision: 4 }));
    expect(poolActivityState(members, poolMemberRuntimeStates(members, fresh), fresh, activity).nextMember?.id).toBe("account");
  });

  test("an absent next-candidate preview does not block ready members in a mixed pool", () => {
    const members = [
      member("account", "healthy", { models: ["gpt-5.4"] }),
      member("account", "quota", { operationalStatus: "quotaWait" }),
      member("account", "reauth", { operationalStatus: "unavailable", authState: { state: "requires_reauth", reason: "invalidated_refresh_token" } }),
    ];
    for (const order of [[], [candidate("reauth", { available: false })], [candidate("healthy", { available: false })]]) {
      const state = poolActivityState(members, poolMemberRuntimeStates(members, order), order, undefined, ["gpt-5.4"]);
      expect(state.nextMember).toBeNull();
      expect(poolRoutingAvailability(members, ["gpt-5.4"], state.activeRequestTotal)).toBe("ready");
    }
    expect(poolRoutingAvailability(members.slice(1), ["gpt-5.4"], 0)).toBe("unavailable");
    expect(poolRoutingAvailability(members, [], 0)).toBe("noModels");
    expect(poolRoutingAvailability(members.slice(1), [], 1)).toBe("active");
  });

  test("fresh snapshots retire only activity events they have observed", () => {
    const order = [candidate("account", { runtimeId: 2, activityRevision: 4, nextForNewRequest: true })];
    const completed = { runtimeId: 2, revision: 5, candidateId: "account", inFlight: 0, activeRequestCount: 0, activeModels: [] };
    const overlay = new Map([["account", completed]]);
    reconcileRuntimeActivityOverlay(order, overlay);
    expect(overlay.size).toBe(1);
    expect(applyRuntimeActivities(order, overlay.values())[0]?.nextForNewRequest).toBe(false);
    const fresh = order.map((item) => ({ ...item, activityRevision: 6, activeRequestCount: 1 }));
    reconcileRuntimeActivityOverlay(fresh, overlay);
    expect(overlay.size).toBe(0);
    expect(applyRuntimeActivities(fresh, [completed])[0]?.activeRequestCount).toBe(1);
  });

  test("runtime replacement discards old activity without hiding its first new request", () => {
    const members = [member("account", "account"), member("account", "removed")];
    const order = [candidate("account", { runtimeId: 2, activityRevision: 0, nextForNewRequest: true })];
    const old = { runtimeId: 1, revision: 99, candidateId: "account", inFlight: 2, activeRequestCount: 2, activeModels: [] };
    const retired = { ...old, candidateId: "removed" };
    const activity = { runtimeId: 1, revision: 99, lastCandidateId: "removed", candidates: { account: old, removed: retired } };
    const byMember = poolMemberRuntimeStates(members, order, activity);
    const state = poolActivityState(members, byMember, order, activity);
    expect(state.activeRequestTotal).toBe(0);
    expect(state.lastActivityMember).toBeNull();
    expect(state.nextMember?.id).toBe("account");
    const overlay = new Map([["account", old], ["removed", retired]]);
    reconcileRuntimeActivityOverlay(order, overlay);
    expect(overlay.size).toBe(0);
    const fresh = { ...old, runtimeId: 2, revision: 1, inFlight: 1, activeRequestCount: 1 };
    expect(compareRuntimeActivity(fresh, old)).toBeGreaterThan(0);
    const result = applyRuntimeActivities(order, [fresh, old]);
    expect(result[0]?.activeRequestCount).toBe(1);
    expect(result[0]?.nextForNewRequest).toBe(false);
  });

  test("a late poll cannot resurrect a released request after its overlay was retired", () => {
    const stale = [candidate("account", { runtimeId: 1, activityRevision: 8, activeRequestCount: 1 })];
    const fresh = [candidate("account", { runtimeId: 1, activityRevision: 9, activeRequestCount: 0 })];
    expect(preferNewerRuntimeOrder(stale, fresh)).toBe(fresh);
    expect(preferNewerRuntimeOrder(fresh, stale)).toBe(fresh);
    const restarted = [candidate("account", { runtimeId: 2, activityRevision: 0 })];
    expect(preferNewerRuntimeOrder(fresh, restarted)).toBe(restarted);
    expect(preferNewerRuntimeOrder(restarted, fresh)).toBe(restarted);
    expect(preferNewerRuntimeOrder(restarted, [])).toEqual([]);
  });

  test("groups statuses before runtime preference, with name ordering only as a fallback", () => {
    const members = [
      member("account", "error", { label: "A", operationalStatus: "unavailable" }),
      member("account", "disabled", { label: "B", operationalStatus: "disabled" }),
      member("account", "wait", { label: "C", operationalStatus: "quotaWait" }),
      member("source", "source-ready", { name: "Z" }),
      member("account", "ready", { label: "Y" }),
    ];
    const order = members.map((item) => candidate(item.id));
    expect(orderedPoolMembers(members, order).map((item) => item.id)).toEqual(["source-ready", "ready", "wait", "error", "disabled"]);
    expect(orderedPoolMembers(members, []).map((item) => item.id)).toEqual(["ready", "source-ready", "wait", "error", "disabled"]);
  });

  test("projects only pooled accounts and sources and keeps source ids stable", () => {
    const runtime = {
      accounts: [{ id: "account-1", inPool: true }, { id: "account-2", inPool: false }],
      sources: [{ id: "source-2", inPool: true }, { id: "source-1", inPool: true }],
    } as unknown as RuntimeSnapshot;
    const members = poolMembersFromRuntime(runtime);
    expect(members.map((item) => `${item.kind}:${item.id}`)).toEqual([
      "account:account-1",
      "source:source-2",
      "source:source-1",
    ]);
    expect(poolMemberSourceIds(members)).toBe("source-1\nsource-2");
  });

  test("maps protocol candidates back to source cards and preserves backend order", () => {
    const members = [member("source", "source-1"), member("account", "account-1")];
    const order = [candidate("account-1"), candidate("source-1::responses")];
    const runtimeByMember = poolMemberRuntimeStates(members, order);
    expect(runtimeByMember.get("source-1")?.candidateId).toBe("source-1");
    expect(orderedPoolMembers(members, order).map((item) => item.id)).toEqual(["account-1", "source-1"]);
  });

  test("derives active and last-used route state from runtime snapshots", () => {
    const members = [member("account", "active"), member("source", "source-last"), member("source", "source-next")];
    const order = [
      candidate("active", { available: false, activeRequestCount: 2, inFlight: 2, activeModels: [{ model: "gpt-5.4", requestCount: 2 }] }),
      candidate("source-last::responses", { available: false, lastUsedAtMs: 20 }),
      candidate("source-next::responses", { available: true, nextForNewRequest: true }),
    ];
    const state = poolActivityState(members, poolMemberRuntimeStates(members, order), order);
    expect(state.activeMembers.map((item) => item.id)).toEqual(["active"]);
    expect(state.activeRequestTotal).toBe(2);
    expect(state.activeModels).toEqual([{ model: "gpt-5.4", requestCount: 2 }]);
    expect(state.lastUsedMember?.id).toBe("source-last");
    expect(state.lastActivityMember).toBeNull();
    expect(state.nextMember?.id).toBe("source-next");
  });

  test("exposes the next available route from the live scheduler order", () => {
    const members = [member("account", "account-top"), member("source", "stabilizer")];
    const order = [
      candidate("account-top", { available: true, nextForNewRequest: true }),
      candidate("stabilizer::responses", { available: true }),
    ];
    const runtimeByMember = poolMemberRuntimeStates(members, order);
    const state = poolActivityState(members, runtimeByMember, order, {
      revision: 7,
      lastCandidateId: "stabilizer::responses",
      candidates: {},
    });

    expect(state.lastActivityMember?.id).toBe("stabilizer");
    expect(state.lastUsedMember).toBeNull();
    expect(state.nextMember?.id).toBe("account-top");
  });

  test("matches the last-used member by identity when timestamps are equal", () => {
    const members = [member("account", "account-first"), member("source", "source-last")];
    const order = [
      candidate("source-last::responses", { lastUsedAtMs: 20 }),
      candidate("account-first", { lastUsedAtMs: 20 }),
    ];
    const state = poolActivityState(members, poolMemberRuntimeStates(members, order), order);

    expect(state.lastUsedRuntime?.candidateId).toBe("source-last::responses");
    expect(state.lastUsedMember?.id).toBe("source-last");
  });

  test("does not assign a removed candidate's last use to another member", () => {
    const members = [member("account", "account-current")];
    const order = [
      candidate("account-removed", { lastUsedAtMs: 20 }),
      candidate("account-current", { lastUsedAtMs: 20 }),
    ];
    const state = poolActivityState(members, poolMemberRuntimeStates(members, order), order);

    expect(state.lastUsedMember).toBeNull();
  });

  test("shows an active account from the activity overlay before a stale order catches up", () => {
    const members = [member("account", "account-active"), member("source", "source-api-next", { models: ["gpt-5.4"] })];
    const order = [candidate("source-api-next::responses", { available: true })];
    const activity = {
      revision: 11,
      lastCandidateId: "account-active",
      candidates: {
        "account-active": {
          revision: 11,
          candidateId: "account-active",
          inFlight: 1,
          activeRequestCount: 1,
          activeModels: [{ model: "gpt-5.4", requestCount: 1 }],
        },
      },
    };
    const runtimeByMember = poolMemberRuntimeStates(members, order, activity);
    const state = poolActivityState(members, runtimeByMember, order, activity);

    expect(state.activeMembers.map((item) => item.id)).toEqual(["account-active"]);
    expect(state.activeRequestTotal).toBe(1);
    expect(state.activeModels).toEqual([{ model: "gpt-5.4", requestCount: 1 }]);
    expect(state.nextMember).toBeNull();
  });

  test("shows an active API source from the activity overlay before a stale order catches up", () => {
    const members = [member("source", "source-active", { models: ["gpt-5.4"] }), member("account", "account-next", { models: ["gpt-5.4"] })];
    const order = [candidate("account-next", { available: true })];
    const activity = {
      revision: 12,
      lastCandidateId: "source-active::responses",
      candidates: {
        "source-active::responses": {
          revision: 12,
          candidateId: "source-active::responses",
          inFlight: 2,
          activeRequestCount: 2,
          activeModels: [{ model: "gpt-5.4", requestCount: 2 }],
        },
      },
    };
    const runtimeByMember = poolMemberRuntimeStates(members, order, activity);
    const state = poolActivityState(members, runtimeByMember, order, activity, ["gpt-5.4"]);

    expect(state.activeMembers.map((item) => item.id)).toEqual(["source-active"]);
    expect(state.activeRequestTotal).toBe(2);
    expect(state.nextMember).toBeNull();
  });

  test("does not expose a stale healthy candidate for an unavailable member", () => {
    const members = [member("source", "zenith", { operationalStatus: "unavailable", models: ["gpt-5.4"] })];
    const order = [candidate("zenith::responses", { available: true })];
    const state = poolActivityState(
      members,
      poolMemberRuntimeStates(members, order),
      order,
      undefined,
      ["gpt-5.4"],
    );
    expect(state.nextMember).toBeNull();
  });

  test("does not expose a route when the visible model catalog is empty", () => {
    const members = [member("source", "source-1", { models: ["gpt-5.4"] })];
    const order = [candidate("source-1::responses", { available: true })];
    const state = poolActivityState(
      members,
      poolMemberRuntimeStates(members, order),
      order,
      undefined,
      [],
    );
    expect(state.nextMember).toBeNull();
  });

  test("counts account and source errors without changing status semantics", () => {
    const members = [
      member("account", "ready"),
      member("account", "error", { quotaRefreshStatus: "failed", quota: { error: { code: "quota_transport" } } }),
      member("source", "source-error", { operationalStatus: "unavailable", lastErrorCode: "upstream_404" }),
      member("source", "disabled", { operationalStatus: "disabled" }),
      member("account", "quota", { operationalStatus: "quotaWait" }),
    ];
    expect(poolMemberStatusCounts(members)).toEqual({ rotation: 2, quotaWait: 1, errors: 2, disabled: 1 });
  });

  test("sums provider credits only for accounts currently in the pool", () => {
    const members = [
      member("account", "account-one", { quota: { availableCreditsMicroUnits: 1_250_000 } }),
      member("account", "account-two", { quota: { availableCreditsMicroUnits: 2_750_000 } }),
      member("account", "account-outside", { inPool: false, quota: { availableCreditsMicroUnits: 99_000_000 } }),
      member("source", "source-without-account-credits"),
    ];

    expect(poolProviderCreditsSummary(members)).toEqual({ kind: "finite", availableCredits: 4 });
  });

  test("shows unlimited provider credits and omits the summary without a ledger", () => {
    expect(poolProviderCreditsSummary([
      member("account", "unlimited", { quota: { providerCreditsUnlimited: true } }),
      member("account", "finite", { quota: { availableCreditsMicroUnits: 2_000_000 } }),
    ])).toEqual({ kind: "unlimited" });
    expect(poolProviderCreditsSummary([member("account", "missing"), member("source", "source-only")])).toBeNull();
  });

  test("omits a zero credit balance from the summary", () => {
    expect(poolProviderCreditsSummary([
      member("account", "empty", { quota: { availableCreditsMicroUnits: 0 } }),
    ])).toBeNull();
  });
});
