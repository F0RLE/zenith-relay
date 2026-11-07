import { describe, expect, test } from "bun:test";
import type { AccountSummary, CandidateRuntimeSnapshot, RuntimeSnapshot } from "../src/features/relay/api/types";
import type { PoolMember } from "../src/features/relay/poolHelpers";
import { applyRuntimeActivities, compareRuntimeActivity, preferNewerRuntimeOrder, reconcileRuntimeActivityOverlay, sameRuntimeOrder } from "../src/features/relay/routingOrder";
import {
  orderedPoolMembers,
  poolActivityState,
  poolMemberRuntimeStates,
  poolMembersFromRuntime,
  poolMemberStatusCounts,
  poolMemberDisplayStatus,
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

function linkedQuotaPool(donorKind: "codex" | "excel_bps" = "excel_bps") {
  const now = Date.now();
  const window = (remaining: number) => ({ kind: "primary", availableBasisPoints: remaining,
    observedAtMs: now - 1_000, resetAtMs: now + 60_000, windowMinutes: 300 });
  const account = (id: string, overrides: Record<string, unknown> = {}) => member("account", id, {
    oauthClientKind: "excel_bps", subscription: {}, refreshState: { quota: "fresh" }, ...overrides,
  }) as Extract<PoolMember, { kind: "account" }>;
  const selected = account("siwc", { oauthClientKind: "siwc", routeGroupId: "linked",
    quota: { primary: null, secondary: null, supplemental: [], limitReached: false } });
  const donor = account("reference", { oauthClientKind: donorKind, routeGroupId: "linked", routeInactive: true,
    inPool: false, quota: { primary: window(5_600), secondary: { ...window(6_500), kind: "secondary" } } });
  const members = [3_400, 2_800, 900].map((remaining) => account(`quota-${remaining}`, { quota: { primary: window(remaining) } }));
  members.push(selected);
  return { selected, donor, members, accounts: [...members, donor], now };
}

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

  test("next-member projection stops at the first eligible preview without reading later routes", () => {
    const members = [
      member("account", "blocked", { enabled: false, models: ["gpt"] }),
      member("account", "first", { models: ["gpt"] }),
      member("account", "later", { models: ["gpt"] }),
    ];
    const order = ["removed", "blocked", "first", "later"].map((id) => candidate(id, { nextForNewRequest: true }));
    const byMember = poolMemberRuntimeStates(members, order);
    let laterReads = 0;
    Object.defineProperty(order[3]!, "available", {
      get() { laterReads += 1; return true; },
    });
    const state = poolActivityState(members, byMember, order, undefined, ["gpt"]);
    expect(state.nextMember).toBe(members[1]!);
    expect(laterReads).toBe(0);
    expect(order.map((item) => item.candidateId)).toEqual(["removed", "blocked", "first", "later"]);
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

  test("projects only pooled accounts and sources", () => {
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
  });

  test("maps protocol candidates back to source cards and preserves backend order", () => {
    const members = [member("source", "source-1"), member("account", "account-1")];
    const order = [candidate("account-1"), candidate("source-1::responses")];
    const runtimeByMember = poolMemberRuntimeStates(members, order);
    expect(runtimeByMember.get("source-1")?.candidateId).toBe("source-1");
    expect(orderedPoolMembers(members, order).map((item) => item.id)).toEqual(["account-1", "source-1"]);
  });

  test("keeps active work ahead of the scheduler's next member and saved order", () => {
    const members = [
      member("account", "first", { label: "First" }),
      member("account", "second", { label: "Second" }),
    ];
    const live = [
      candidate("second", { activeRequestCount: 1, nextForNewRequest: true }),
      candidate("first"),
    ];
    expect(orderedPoolMembers(members, live).map((item) => item.id)).toEqual(["second", "first"]);
    expect(orderedPoolMembers(members, live, [
      { id: "first" },
      { id: "second" },
    ]).map((item) => item.id)).toEqual(["second", "first"]);
    const busyOnly = live.map((item) => ({ ...item, nextForNewRequest: false }));
    expect(orderedPoolMembers(members, busyOnly, members, "round_robin").map((item) => item.id)).toEqual(["second", "first"]);
  });

  test("automatic cards keep active work first, then rank fresh quota ahead of manual order", () => {
    const window = (availableBasisPoints: number) => ({ availableBasisPoints });
    const members = [
      member("source", "api", { name: "Zenith API" }),
      member("account", "empty", {
        label: "Empty",
        quota: { primary: window(0), providerCreditsAvailable: true, availableCreditsMicroUnits: 40_000_000 },
        refreshState: { models: "stale", quota: "fresh" },
      }),
      member("account", "empty-lower", {
        label: "Empty lower",
        quota: { primary: window(0), providerCreditsAvailable: true, availableCreditsMicroUnits: 20_000_000 },
        refreshState: { models: "stale", quota: "fresh" },
      }),
      member("account", "lower", { label: "Lower", quota: { primary: window(7_800), secondary: window(9_000) }, refreshState: { quota: "fresh" } }),
      member("account", "leader", { label: "Leader", quota: { primary: window(8_300) }, refreshState: { quota: "fresh" } }),
    ];
    const live = [
      candidate("lower", { activeRequestCount: 1 }),
      candidate("leader"),
      candidate("empty"),
      candidate("api"),
    ];
    const saved = [{ id: "api" }, { id: "empty" }, { id: "empty-lower" }, { id: "lower" }, { id: "leader" }];
    expect(orderedPoolMembers(members, live, saved, "automatic").map((item) => item.id)).toEqual([
      "lower",
      "leader",
      "empty",
      "empty-lower",
      "api",
    ]);
    expect(orderedPoolMembers(members, live, saved, "round_robin").map((item) => item.id)).toEqual([
      "lower",
      "api",
      "empty",
      "empty-lower",
      "leader",
    ]);
  });

  test("all modes show active work before the scheduler's fallback preference", () => {
    const members = ["a", "b", "c"].map((id) => member("account", id, {
      label: id, quota: { primary: { availableBasisPoints: id === "a" ? 10_000 : 1_000 } }, refreshState: { quota: "fresh" },
    }));
    const order = [candidate("a", { selectionRank: 2, activeRequestCount: 1 }),
      candidate("b", { selectionRank: 0, nextForNewRequest: true }), candidate("c", { selectionRank: 1 })];
    for (const mode of ["automatic", "round_robin", "weighted_round_robin", "failover"] as const) {
      expect(orderedPoolMembers(members, order, members, mode).map((item) => item.id)).toEqual(["a", "b", "c"]);
    }
    expect(members.map((item) => item.id)).toEqual(["a", "b", "c"]);
  });

  test("live unavailable members cannot outrank ready members, even with a stale preview", () => {
    const members = [
      member("account", "disabled", { enabled: false, lastErrorCode: "invalid_grant" }),
      member("account", "draining", { draining: true }),
      member("account", "error", { operationalStatus: "unavailable" }),
      member("account", "wait-late"), member("account", "wait-soon"),
      member("account", "ready"), member("account", "no-secret", { secretAvailable: false }),
    ];
    const order = members.map((item) => candidate(item.id, item.id.startsWith("wait")
      ? { available: false, quotaWait: true, nextRetryAtMs: item.id === "wait-soon" ? 100 : 200, selectionRank: 0 }
      : { available: true, selectionRank: item.id === "ready" ? 1 : 0 }));
    const states = poolMemberRuntimeStates(members, order);
    for (const mode of ["automatic", "round_robin", "weighted_round_robin", "failover"] as const) {
      expect(orderedPoolMembers(members, order, members, mode, states).map((item) => item.id))
        .toEqual(["ready", "wait-soon", "wait-late", "error", "no-secret", "disabled", "draining"]);
    }
    expect(poolMemberDisplayStatus(members[0]!, states.get("disabled"))).toBe("disabled");
    expect(poolMemberStatusCounts(members, states)).toMatchObject({ rotation: 1, quotaWait: 2, disabled: 2 });
  });

  test("weekly exhaustion follows temporary quota waits in every display mode", () => {
    const now = Date.now();
    const window = (availableBasisPoints: number) => ({ availableBasisPoints,
      observedAtMs: now, resetAtMs: now + 60_000 });
    const members = [
      member("account", "weekly", { subscription: {}, operationalStatus: "quotaWait", refreshState: { quota: "fresh" },
        quota: { primary: window(10_000), secondary: window(0), updatedAtMs: now } }),
      member("account", "temporary", { subscription: {}, operationalStatus: "quotaWait", refreshState: { quota: "fresh" },
        quota: { primary: window(0), secondary: window(6_900), updatedAtMs: now } }),
      member("account", "both", { subscription: {}, operationalStatus: "quotaWait", refreshState: { quota: "fresh" },
        quota: { primary: window(0), secondary: window(0), updatedAtMs: now } }),
      member("account", "ready"),
    ];
    const order = members.map((item) => candidate(item.id, { available: item.id === "ready",
      quotaWait: item.id !== "ready", nextRetryAtMs: item.id === "temporary" ? now + 5_000 : now + 1_000 }));
    for (const mode of ["automatic", "round_robin", "weighted_round_robin", "failover"] as const) {
      expect(orderedPoolMembers(members, order, members, mode, undefined, [], now).map((item) => item.id))
        .toEqual(["ready", "temporary", "weekly", "both"]);
      expect(orderedPoolMembers(members, [], members, mode, undefined, [], now).map((item) => item.id))
        .toEqual(["ready", "temporary", "weekly", "both"]);
    }
    expect(members.map((item) => item.id)).toEqual(["weekly", "temporary", "both", "ready"]);
    const activeOrder = order.map((item) => ({ ...item, activeRequestCount: item.candidateId === "weekly" ? 1 : 0 }));
    expect(orderedPoolMembers(members, activeOrder, members, "automatic", undefined, [], now).map((item) => item.id))
      .toEqual(["weekly", "ready", "temporary", "both"]);
    const staleMembers = members.map((item) => item.id === "weekly"
      ? { ...item, refreshState: { quota: "stale" } } as PoolMember : item);
    expect(orderedPoolMembers(staleMembers, order, members, "automatic", undefined, [], now).map((item) => item.id))
      .toEqual(["ready", "weekly", "temporary", "both"]);
  });

  test("source aliases preserve preference and only wait when no binding is ready", () => {
    const members = [member("source", "source"), member("account", "account")];
    const order = [candidate("source::messages", { available: false, quotaWait: true }),
      candidate("source::responses", { selectionRank: 0 }), candidate("account", { selectionRank: 1 })];
    const states = poolMemberRuntimeStates(members, order);
    expect(states.get("source")).toMatchObject({ available: true, quotaWait: false, selectionRank: 0 });
    expect(orderedPoolMembers(members, order, members, "automatic", states).map((item) => item.id)).toEqual(["source", "account"]);
    expect(sameRuntimeOrder(order, order.map((item) => ({ ...item, selectionRank: 2 })))).toBe(false);
    expect(sameRuntimeOrder(order, order.map((item) => ({ ...item, quotaWait: false })))).toBe(false);
  });

  test("distinguishes cooldown, capacity, recovery and draining from terminal errors", () => {
    const ready = member("account", "account");
    const cooling = candidate("account", { available: false, nextRetryAtMs: 200 });
    expect(poolMemberDisplayStatus(ready, cooling)).toBe("cooldown");
    expect(poolMemberDisplayStatus(ready, candidate("account", { available: false, capacityWait: true, activeRequestCount: 1 }))).toBe("busy");
    expect(poolMemberDisplayStatus(member("account", "account", { operationalStatus: "unavailable" }),
      candidate("account", { available: false, activeRequestCount: 1 }))).toBe("unavailable");
    expect(poolMemberDisplayStatus(ready, candidate("account", { halfOpen: true }))).toBe("rotation");
    expect(poolMemberDisplayStatus({ ...ready, draining: true }, cooling)).toBe("draining");
    expect(poolMemberDisplayStatus(member("account", "account", { authState: { state: "requires_reauth" } }), cooling)).toBe("unavailable");
    expect(poolMemberDisplayStatus(member("account", "account", { routingBlockReason: "subscription_expired" }), cooling)).toBe("unavailable");
    expect(poolMemberDisplayStatus(ready, candidate("account", { available: false }))).toBe("rotation");
    expect(poolMemberDisplayStatus(ready, { ...cooling, available: true })).toBe("rotation");
    expect(poolMemberStatusCounts([ready], new Map([[ready.id, cooling]])))
      .toEqual({ rotation: 0, quotaWait: 0, waiting: 1, errors: 0, disabled: 0 });
    const source = member("source", "source", { operationalStatus: "unavailable" });
    expect(poolMemberStatusCounts([source], new Map([[source.id, candidate("source", { available: false, capacityWait: true })]])))
      .toEqual({ rotation: 0, quotaWait: 0, waiting: 1, errors: 0, disabled: 0 });
  });

  test("new activity invalidates the whole preference, not just its first member", () => {
    const members = [member("account", "a"), member("account", "b")];
    const order = [candidate("a", { activityRevision: 2, selectionRank: 1 }), candidate("b", { activityRevision: 2, selectionRank: 0, nextForNewRequest: true })];
    const activity = { revision: 3, lastCandidateId: "b", candidates: {
      b: { candidateId: "b", revision: 3, inFlight: 1, activeRequestCount: 1, activeModels: [] },
    } };
    const states = poolMemberRuntimeStates(members, order, activity);
    expect(states.get("a")?.selectionRank).toBeNull();
    expect(states.get("b")?.selectionRank).toBeNull();
    expect(orderedPoolMembers(members, order, members, "round_robin", states).map((item) => item.id)).toEqual(["b", "a"]);
  });

  test("automatic cards do not rank stale, undated or explicitly exhausted quota as fresh capacity", () => {
    const fresh = member("account", "fresh", {
      quota: { primary: { availableBasisPoints: 1_000 } }, refreshState: { quota: "fresh" },
    });
    for (const overrides of [
      { refreshState: { quota: "stale" } },
      { refreshState: undefined },
      { quota: { primary: { availableBasisPoints: 10_000 }, limitReached: true } },
      { quota: { primary: { availableBasisPoints: 20_000 } } },
    ]) {
      const outdated = member("account", "outdated", {
        quota: { primary: { availableBasisPoints: 10_000 } }, refreshState: { quota: "fresh" }, ...overrides,
      });
      expect(orderedPoolMembers([outdated, fresh], [], [outdated, fresh], "automatic").map((item) => item.id))
        .toEqual(["fresh", "outdated"]);
    }
  });

  test("automatic fallback ranks a hidden linked quota source without changing account data", () => {
    for (const kind of ["codex", "excel_bps"] as const) {
      const { members, accounts } = linkedQuotaPool(kind);
      const before = JSON.stringify(accounts);
      const visible = poolMembersFromRuntime({ accounts, sources: [] } as unknown as RuntimeSnapshot);
      for (const order of [[], members.map((item) => candidate(item.id))]) {
        expect(orderedPoolMembers(visible, order, members, "automatic", undefined, accounts).map((item) => item.id))
          .toEqual(["siwc", "quota-3400", "quota-2800", "quota-900"]);
      }
      expect(JSON.stringify(accounts)).toBe(before);
      expect(visible).toHaveLength(4);
    }
  });

  test("quota ordering is independent of the selected account client", () => {
    for (const kind of ["codex", "excel_bps", "siwc"] as const) {
      const { selected, donor, members, accounts } = linkedQuotaPool();
      selected.oauthClientKind = kind;
      if (kind !== "siwc") selected.quota = donor.quota;
      expect(orderedPoolMembers(members, [], members, "automatic", undefined, accounts)[0]?.id).toBe("siwc");
    }
  });

  test("linked ranking expires on the display clock without replacing the inventory", () => {
    const { members, accounts, now } = linkedQuotaPool();
    expect(orderedPoolMembers(members, [], members, "automatic", undefined, accounts, now)[0]?.id).toBe("siwc");
    expect(orderedPoolMembers(members, [], members, "automatic", undefined, accounts, now + 60_000).at(-1)?.id).toBe("siwc");
  });

  test("a reference never transfers provider credits or hides own supplemental evidence", () => {
    const { selected, donor, members, accounts } = linkedQuotaPool();
    selected.quota.supplemental = [{ id: "fast", label: "Fast", serviceTier: "fast", window: donor.quota.primary! }];
    expect(orderedPoolMembers(members, [], members, "automatic", undefined, accounts).at(-1)?.id).toBe("siwc");
    selected.quota.supplemental = [];
    donor.quota = { ...donor.quota, primary: null, secondary: null, providerCreditsAvailable: true, availableCreditsMicroUnits: 1_000_000_000 };
    const ownCredits = member("account", "own-credits", { refreshState: { quota: "fresh" }, quota: { providerCreditsAvailable: true, availableCreditsMicroUnits: 1_000_000 } });
    expect(orderedPoolMembers([selected, ownCredits], [], [selected, ownCredits], "automatic", undefined, accounts)[0]?.id).toBe("own-credits");
  });

  test("linked fallback rejects stale, invalid and unavailable reference evidence", () => {
    const invalid: Record<string, (donor: AccountSummary, now: number) => void> = {
      stale: (a, now) => { a.quota.primary!.observedAtMs = now - 20 * 60_000; },
      future: (a, now) => { a.quota.primary!.observedAtMs = now + 60_000; },
      undated: (a) => { a.quota.primary!.observedAtMs = 0; },
      reset: (a, now) => { a.quota.primary!.resetAtMs = now - 1; },
      subscription: (a, now) => { a.subscription.activeUntilMs = now - 1; },
      missingCounter: (a) => { a.quota.primary!.availableBasisPoints = null; },
      invalidCounter: (a) => { a.quota.primary!.availableBasisPoints = 10_001; },
      exhausted: (a) => { a.quota.secondary!.availableBasisPoints = 0; },
      limit: (a) => { a.quota.limitReached = true; },
      failed: (a) => { a.quota.error = { code: "quota_failed", occurredAtMs: Date.now() }; },
      unavailable: (a) => { a.secretAvailable = false; },
      disabled: (a) => { a.enabled = false; },
      draining: (a) => { a.draining = true; },
      reauth: (a) => { a.authState = { state: "requires_reauth", reason: "invalidated_refresh_token" }; },
      recovery: (a) => { a.recoveryRequired = true; },
      unlinked: (a) => { a.routeGroupId = "unrelated"; },
    };
    for (const [reason, invalidate] of Object.entries(invalid)) {
      const { donor, members, accounts, now } = linkedQuotaPool();
      invalidate(donor, now);
      expect(orderedPoolMembers(members, [], members, "automatic", undefined, accounts).at(-1)?.id, reason).toBe("siwc");
    }
  });

  test("runtime resources and selection rank remain authoritative over linked fallback", () => {
    const { members, accounts } = linkedQuotaPool();
    const headroom = { quotaRemainingBasisPoints: null, providerCreditsMicroUnits: null, providerCreditsUnlimited: false };
    expect(orderedPoolMembers(members, [candidate("siwc", { resourceHeadroom: headroom })], members, "automatic", undefined, accounts).at(-1)?.id)
      .toBe("siwc");
    expect(orderedPoolMembers(members, [candidate("quota-900", { selectionRank: 0 })], members, "automatic", undefined, accounts)[0]?.id)
      .toBe("quota-900");
  });

  test("own quota and explicit routing modes are preserved instead of borrowing a larger reference", () => {
    for (const remaining of [0, 800]) {
      const { selected, donor, members, accounts } = linkedQuotaPool();
      selected.quota = { ...donor.quota, primary: { ...donor.quota.primary!, availableBasisPoints: remaining } };
      expect(orderedPoolMembers(members, [], members, "automatic", undefined, accounts).at(-1)?.id).toBe("siwc");
    }
    const { members, accounts } = linkedQuotaPool();
    for (const mode of ["round_robin", "weighted_round_robin", "failover"] as const) {
      expect(orderedPoolMembers(members, [], members, mode, undefined, accounts).map((item) => item.id))
        .toEqual(members.map((item) => item.id));
    }
  });

  test("scheduler resource evidence overrides raw quotas and monitoring freshness", () => {
    const members = [
      member("account", "stale", { quota: { primary: { availableBasisPoints: 10_000 } }, refreshState: { quota: "fresh" } }),
      member("account", "fresh", { quota: { primary: { availableBasisPoints: 1_000 } }, refreshState: { quota: "stale" } }),
    ];
    const order = [
      candidate("stale", { resourceHeadroom: { quotaRemainingBasisPoints: null, providerCreditsMicroUnits: null, providerCreditsUnlimited: false } }),
      candidate("fresh", { resourceHeadroom: { quotaRemainingBasisPoints: 1_000, providerCreditsMicroUnits: null, providerCreditsUnlimited: false } }),
    ];
    expect(orderedPoolMembers(members, order, members, "automatic").map((item) => item.id)).toEqual(["fresh", "stale"]);
  });

  test("credit ranking follows the independent scheduler ledger evidence", () => {
    const members = [
      member("account", "stale", { quota: { availableCreditsMicroUnits: 1_000_000_000, providerCreditsAvailable: true }, refreshState: { quota: "fresh" } }),
      member("account", "fresh", { quota: { availableCreditsMicroUnits: 1_000_000 }, refreshState: { quota: "stale" } }),
    ];
    const order = [
      candidate("stale", { resourceHeadroom: { quotaRemainingBasisPoints: null, providerCreditsMicroUnits: null, providerCreditsUnlimited: false } }),
      candidate("fresh", { resourceHeadroom: { quotaRemainingBasisPoints: null, providerCreditsMicroUnits: 1_000_000, providerCreditsUnlimited: false } }),
    ];
    expect(orderedPoolMembers(members, order, members, "automatic").map((item) => item.id)).toEqual(["fresh", "stale"]);
    const unlimited = { ...order[0], resourceHeadroom: { quotaRemainingBasisPoints: null, providerCreditsMicroUnits: null, providerCreditsUnlimited: true } };
    expect(orderedPoolMembers(members, [unlimited, order[1]], members, "automatic").map((item) => item.id)).toEqual(["stale", "fresh"]);
  });

  test("resource-only snapshot changes are not discarded as unchanged activity", () => {
    const before = candidate("account", { resourceHeadroom: { quotaRemainingBasisPoints: 1_000, providerCreditsMicroUnits: 2_000_000, providerCreditsUnlimited: false } });
    expect(sameRuntimeOrder([before], [structuredClone(before)])).toBe(true);
    for (const change of [{ quotaRemainingBasisPoints: null }, { providerCreditsMicroUnits: null }, { providerCreditsUnlimited: true }]) {
      const after = { ...before, resourceHeadroom: { ...before.resourceHeadroom!, ...change } };
      expect(sameRuntimeOrder([before], [after])).toBe(false);
    }
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
    expect(poolMemberStatusCounts(members)).toEqual({ rotation: 2, quotaWait: 1, waiting: 1, errors: 2, disabled: 1 });
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
