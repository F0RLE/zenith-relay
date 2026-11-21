import { describe, expect, test } from "bun:test";
import type { AccountSummary, CandidateRuntimeSnapshot, RuntimeSnapshot } from "../src/features/relay/api/types";
import type { PoolMember } from "../src/features/relay/poolHelpers";
import { applyPoolRotation, applyRuntimeActivities, compareRuntimeActivity, preferNewerPoolRotation, preferNewerRuntimeOrder, reconcileRuntimeActivityOverlay, sameRuntimeOrder } from "../src/features/relay/routingOrder";
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
  test("member projection revisions preserve newer statuses across full and lightweight reads", () => {
    const current = { inventoryRevision: 3, preset: null, memberOrder: [], memberStatuses: { "account:one": "rotation" } } as const;
    const projection = { ...current, memberOrder: [] };
    const stale = { ...projection, inventoryRevision: 2 };
    const newer = { ...projection, inventoryRevision: 4 };
    expect(preferNewerPoolRotation(projection, stale, true)).toBe(projection);
    expect(preferNewerPoolRotation(projection, { ...projection }, false)).toBe(projection);
    expect(preferNewerPoolRotation(projection, newer, false)).toBe(newer);
    expect(preferNewerPoolRotation(projection, null, true)).toBe(projection);
    expect(preferNewerPoolRotation(undefined, newer, true)).toBe(newer);
  });

  test("applies backend status recovery without inferring readiness from activity", () => {
    const account = member("account", "shared", { operationalStatus: "unavailable" });
    const source = member("source", "shared", { operationalStatus: "unavailable" });
    const snapshot = { accounts: [account], sources: [source], gateway: {} } as unknown as RuntimeSnapshot;
    const rotation = { inventoryRevision: 2, preset: null, memberOrder: ["account:shared", "source:shared"],
      memberStatuses: { "account:shared": "rotation", "source:shared": "unavailable" } } as const;
    const updated = applyPoolRotation(snapshot, { ...rotation, memberOrder: [...rotation.memberOrder] });
    expect(updated.accounts[0]?.operationalStatus).toBe("rotation");
    expect(updated.sources).toBe(snapshot.sources);
    expect(updated.accounts[0]?.authState).toBe(snapshot.accounts[0]?.authState);
    expect(snapshot.accounts[0]?.operationalStatus).toBe("unavailable");
    expect(applyPoolRotation(updated, updated.gateway.poolRotation!).accounts).toBe(updated.accounts);
    const legacy = applyPoolRotation(snapshot, { preset: null, memberOrder: [] });
    expect(legacy.accounts).toBe(snapshot.accounts);
    expect(legacy.sources).toBe(snapshot.sources);
  });

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

  test("image activity survives delayed polls without replacing backend readiness", () => {
    const model = "gpt-image-test";
    for (const [overrides, status] of [
      [{}, "rotation"],
      [{ operationalStatus: "unavailable", secretAvailable: false }, "unavailable"],
      [{ draining: true }, "draining"],
      [{ enabled: false, operationalStatus: "disabled" }, "disabled"],
    ] as const) {
      const account = member("account", "image-account", {
        models: [model], authState: { state: "active" }, ...overrides,
      });
      const base = [candidate(account.id, {
        runtimeId: 1, activityRevision: 0, available: status === "rotation",
        nextForNewRequest: false,
      })];
      const admitted = {
        runtimeId: 1, revision: 1, candidateId: account.id, inFlight: 0,
        activeRequestCount: 1, activeModels: [{ model, requestCount: 1 }],
      };
      const overlay = new Map([[account.id, admitted]]);
      const project = (order: CandidateRuntimeSnapshot[]) => {
        reconcileRuntimeActivityOverlay(order, overlay);
        const activity = { runtimeId: 1, revision: 1, candidates: Object.fromEntries(overlay) };
        const byMember = poolMemberRuntimeStates([account], order, activity);
        expect(poolMemberDisplayStatus(account, byMember.get(account.id))).toBe(status);
        return poolActivityState([account], byMember, order, activity);
      };
      expect(project(base).activeModels).toEqual([{ model, requestCount: 1 }]);
      expect(overlay.size).toBe(1);
      const active = base.map((item) => ({ ...item, activityRevision: 1,
        activeRequestCount: 1, activeModels: admitted.activeModels }));
      expect(project(active).activeRequestTotal).toBe(1);
      expect(overlay.size).toBe(0);

      overlay.set(account.id, { ...admitted, revision: 2, activeRequestCount: 0, activeModels: [] });
      expect(project(active).activeRequestTotal).toBe(0);
      expect(overlay.size).toBe(1);
      const idle = base.map((item) => ({ ...item, activityRevision: 2 }));
      expect(project(idle).activeRequestTotal).toBe(0);
      expect(overlay.size).toBe(0);
    }
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

  test("renders backend order without sorting by status or name", () => {
    const members = [
      member("account", "error", { label: "A", operationalStatus: "unavailable" }),
      member("account", "disabled", { label: "B", operationalStatus: "disabled" }),
      member("account", "wait", { label: "C", operationalStatus: "quotaWait" }),
      member("source", "source-ready", { name: "Z" }),
      member("account", "ready", { label: "Y" }),
    ];
    const order = ["source:source-ready", "account:ready", "account:wait", "account:error", "account:disabled"];
    expect(orderedPoolMembers(members, order).map((item) => item.id)).toEqual(["source-ready", "ready", "wait", "error", "disabled"]);
    expect(orderedPoolMembers(members, [])).toEqual(members);
    expect(orderedPoolMembers(members, ["account:disabled", ...order]).map((item) => item.id)).toEqual(["disabled", "source-ready", "ready", "wait", "error"]);
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
    expect(orderedPoolMembers(members, ["account:account-1", "source:source-1"]).map((item) => item.id)).toEqual(["account-1", "source-1"]);
  });

  test("keeps backend identities distinct and retains members missing from a projection", () => {
    const members = [
      member("account", "first", { label: "First" }),
      member("account", "second", { label: "Second" }),
    ];
    members.push(member("source", "first"));
    expect(orderedPoolMembers(members, ["source:first", "account:removed", "source:first", "account:second"]).map((item) => `${item.kind}:${item.id}`))
      .toEqual(["source:first", "account:second", "account:first"]);
  });

  test("quota and credit changes cannot override the backend projection", () => {
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
    const order = ["source:api", "account:empty", "account:lower", "account:leader", "account:empty-lower"];
    expect(orderedPoolMembers(members, order).map((item) => item.id)).toEqual(["api", "empty", "lower", "leader", "empty-lower"]);
    expect(orderedPoolMembers(members)).toEqual(members);
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
    expect(poolMemberDisplayStatus(members[0]!, states.get("disabled"))).toBe("disabled");
    expect(poolMemberStatusCounts(members, states)).toMatchObject({ rotation: 1, quotaWait: 2, disabled: 2 });
  });

  test("source aliases preserve preference and only wait when no binding is ready", () => {
    const members = [member("source", "source"), member("account", "account")];
    const order = [candidate("source::messages", { available: false, quotaWait: true }),
      candidate("source::responses", { selectionRank: 0 }), candidate("account", { selectionRank: 1 })];
    const states = poolMemberRuntimeStates(members, order);
    expect(states.get("source")).toMatchObject({ available: true, quotaWait: false, selectionRank: 0 });
    expect(orderedPoolMembers(members, ["source:source", "account:account"]).map((item) => item.id)).toEqual(["source", "account"]);
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
    expect(orderedPoolMembers(members, ["account:a", "account:b"]).map((item) => item.id)).toEqual(["a", "b"]);
  });

  test("linked account metadata never substitutes a frontend ordering policy", () => {
    for (const kind of ["codex", "excel_bps"] as const) {
      const { accounts } = linkedQuotaPool(kind);
      const before = JSON.stringify(accounts);
      const visible = poolMembersFromRuntime({ accounts, sources: [] } as unknown as RuntimeSnapshot);
      const order = ["account:quota-900", "account:siwc", "account:quota-3400", "account:quota-2800"];
      expect(orderedPoolMembers(visible, order).map((item) => item.id))
        .toEqual(["quota-900", "siwc", "quota-3400", "quota-2800"]);
      expect(orderedPoolMembers(visible)).toEqual(visible);
      expect(JSON.stringify(accounts)).toBe(before);
      expect(visible).toHaveLength(4);
    }
  });

  test("linked observation changes await a new backend order", () => {
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
      const { donor, members, now } = linkedQuotaPool();
      invalidate(donor, now);
      expect(orderedPoolMembers(members, ["account:quota-900", "account:siwc"])[0]?.id, reason).toBe("quota-900");
    }
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

  test("unavailable accounts without an error code are counted, but busy accounts are not", () => {
    const accounts = [
      member("account", "unavailable", { operationalStatus: "unavailable" }),
      member("account", "busy", { operationalStatus: "unavailable" }),
    ];
    const states = new Map([
      ["unavailable", candidate("unavailable", { available: false })],
      ["busy", candidate("busy", { available: false, capacityWait: true, activeRequestCount: 1 })],
    ]);
    expect(poolMemberStatusCounts(accounts, states)).toEqual({ rotation: 0, quotaWait: 0, waiting: 1, errors: 1, disabled: 0 });
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
