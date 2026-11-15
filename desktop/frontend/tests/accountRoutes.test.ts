import { describe, expect, test } from "bun:test";
import { accountMetadataIsStale, accountMetadataObservation, accountMetadataSources, accountResetCreditsSource, accountRouteConnections, accountSubscriptionSource, expandAccountRouteIds, linkedAccountQuotaIndicator, linkedAccountQuotaState, preferredAccountRouteConnection, visibleAccounts } from "../src/features/relay/accountRoutes";
import type { AccountSummary } from "../src/features/relay/api/types";

const accounts = [
  { id: "codex", routeGroupId: "codex", routeInactive: true },
  { id: "siwc", routeGroupId: "codex", routeInactive: false },
  { id: "separate" },
] as AccountSummary[];

describe("account route inventory", () => {
  test("one card per group without hiding independent legacy accounts", () => {
    expect(visibleAccounts(accounts).map((a) => a.id)).toEqual(["siwc", "separate"]);
    expect(accountRouteConnections(accounts[1]!, accounts).map((a) => a.id)).toEqual(["codex", "siwc"]);
  });
  test("explicit export and removal include the whole group exactly once", () => {
    expect(expandAccountRouteIds(["siwc", "codex", "separate"], accounts)).toEqual(["codex", "siwc", "separate"]);
    expect(expandAccountRouteIds(["removed"], accounts)).toEqual(["removed"]);
  });
  test("an older card resolves its saved methods from the current inventory", () => {
    const outdated = { id: "siwc" } as AccountSummary;
    expect(accountRouteConnections(outdated, accounts).map((a) => a.id)).toEqual(["codex", "siwc"]);
  });
  test("a detached connection cannot regain an old group through a stale card", () => {
    const detached = { ...accounts[1]!, routeGroupId: null, routeInactive: false };
    expect(accountRouteConnections(accounts[1]!, [accounts[0]!, detached])).toEqual([detached]);
  });
});

const emptyQuota: AccountSummary["quota"] = {
  primary: null, secondary: null, updatedAtMs: null, limitReached: false,
  resetCreditsAvailable: null, directBalanceMicroUsd: null, error: null,
};

function metadataAccount(id: string, kind: AccountSummary["oauthClientKind"], overrides: Partial<AccountSummary> = {}) {
  return {
    id, oauthClientKind: kind, routeGroupId: "shared", routeInactive: kind !== "siwc",
    authState: { state: "active" }, secretAvailable: true, quotaRefreshStatus: "updated",
    subscription: { planType: null, activeUntilMs: null, status: "unknown", updatedAtMs: null },
    quota: { ...emptyQuota },
    ...overrides,
  } as AccountSummary;
}

test("a saved method prefers a usable grant over an expired duplicate without hiding either record", () => {
  const expired = metadataAccount("expired", "siwc", { authState: { state: "requires_reauth", reason: "token_rotation_recovery_required" } });
  const active = metadataAccount("active", "siwc");
  const excel = metadataAccount("excel", "excel_bps");
  const connections = [expired, excel, active];
  expect(preferredAccountRouteConnection(connections, "siwc")).toBe(active);
  expect(preferredAccountRouteConnection(connections, "excel_bps")).toBe(excel);
  expect(preferredAccountRouteConnection([expired], "siwc")).toBe(expired);
  expect(connections.map(({ id }) => id)).toEqual(["expired", "excel", "active"]);
});

const primary = { kind: "primary", availableBasisPoints: 7500, explicitlyFull: false, resetAtMs: 5000, windowMinutes: 300, observedAtMs: 1000 } as const;

describe("SIWC linked reset credits", () => {
  const siwc = metadataAccount("siwc", "siwc");
  const bps = metadataAccount("bps", "excel_bps", {
    quota: { ...emptyQuota, resetCreditsAvailable: 2, updatedAtMs: 1000 },
  });

  test("selects a native credit source without quota windows or changing SIWC state", () => {
    const inventory = [siwc, bps];
    const before = structuredClone(inventory);
    expect(accountResetCreditsSource(siwc, inventory)).toBe(bps);
    expect(inventory).toEqual(before);
    expect(siwc.quota.resetCreditsAvailable).toBeNull();
    expect(siwc.routeInactive).toBe(false);
  });

  test("the newest known zero supersedes older positive availability", () => {
    const codex = metadataAccount("codex", "codex", {
      quota: { ...emptyQuota, resetCreditsAvailable: 0, updatedAtMs: 2000 },
    });
    expect(accountResetCreditsSource(siwc, [siwc, bps, codex])).toBe(codex);
    expect(accountResetCreditsSource(codex, [siwc, bps, codex])).toBe(codex);
  });

  test("unknown native availability and SIWC's own copied counts do not grant reset support", () => {
    const copied = { ...siwc, quota: { ...emptyQuota, resetCreditsAvailable: 3 } };
    const unknown = { ...bps, id: "unknown", quota: { ...emptyQuota, updatedAtMs: 3000 } };
    expect(accountResetCreditsSource(copied, [copied, unknown])).toBeNull();
    expect(accountResetCreditsSource(siwc, [siwc, bps, unknown])).toBe(bps);
  });

  test("only a usable explicitly linked native connection can redeem a credit", () => {
    const unavailable: Partial<AccountSummary>[] = [
      { routeGroupId: "different" }, { oauthClientKind: "siwc" },
      { secretAvailable: false }, { recoveryRequired: true }, { enabled: false },
      { draining: true }, { health: "blocked" }, { health: "unhealthy" },
      { authState: { state: "requires_reauth" } }, { authState: { state: "refreshing" } },
      { authState: { state: "error" } },
    ];
    for (const override of unavailable) {
      expect(accountResetCreditsSource(siwc, [siwc, { ...bps, ...override }])).toBeNull();
    }
    expect(accountResetCreditsSource(siwc, [bps, { ...siwc, routeGroupId: null }])).toBeNull();
  });
});

describe("SIWC display metadata", () => {
  test("subscription date is a same-plan reference while token expiry stays with its route", () => {
    const siwc = metadataAccount("siwc", "siwc", {
      subscription: { planType: "plus", activeUntilMs: null, status: "active", updatedAtMs: 2000 },
      credentialDiagnostics: { status: "not_expired", expiresAtMs: 3000, hasRefreshToken: true, recovery: "none" },
    });
    const bps = metadataAccount("bps", "excel_bps", {
      subscription: { planType: "chatgptplusplan", activeUntilMs: 9000, status: "active", updatedAtMs: 1000 },
      credentialDiagnostics: { status: "not_expired", expiresAtMs: 6000, hasRefreshToken: true, recovery: "none" },
    });
    expect(accountSubscriptionSource(siwc, [siwc, bps])).toBe(bps);
    const codex = { ...siwc, oauthClientKind: "codex" as const };
    expect(accountSubscriptionSource(codex, [codex, bps])).toBe(bps);
    const excel = { ...siwc, oauthClientKind: "excel_bps" as const };
    const codexSource = { ...bps, oauthClientKind: "codex" as const };
    expect(accountSubscriptionSource(excel, [excel, codexSource])).toBe(codexSource);
    expect(siwc.subscription.activeUntilMs).toBeNull();
    expect(siwc.credentialDiagnostics?.expiresAtMs).toBe(3000);
    for (const donor of [
      { ...bps, routeGroupId: "other" },
      { ...bps, recoveryRequired: true },
      { ...bps, subscription: { ...bps.subscription, planType: "pro" } },
    ]) expect(accountSubscriptionSource(siwc, [siwc, donor])).toBe(siwc);
    siwc.subscription.activeUntilMs = 5000;
    expect(accountSubscriptionSource(siwc, [siwc, bps])).toBe(siwc);
  });

  test("prefers fresh Codex or BPS quota over a newer failed observation", () => {
    const siwc = metadataAccount("siwc", "siwc");
    const codex = metadataAccount("codex", "codex", {
      quota: { ...emptyQuota, primary, updatedAtMs: 1000 },
    });
    const bps = metadataAccount("bps", "excel_bps", {
      quota: { ...emptyQuota, primary: { ...primary, observedAtMs: 1500 }, updatedAtMs: 1500 },
      quotaRefreshStatus: "failed",
    });
    expect(accountMetadataSources(siwc, [siwc, codex, bps], 2000).quota).toBe(codex);
    bps.quotaRefreshStatus = "updated";
    expect(accountMetadataSources(siwc, [siwc, codex, bps], 2000).quota).toBe(bps);
    bps.subscription.activeUntilMs = 1800;
    expect(accountMetadataSources(siwc, [siwc, codex, bps], 2000).quota).toBe(codex);
  });

  test("uses only an explicitly grouped connection without mutating dispatch or quota evidence", () => {
    const siwc = metadataAccount("siwc", "siwc");
    const codex = metadataAccount("codex", "codex", {
      subscription: { planType: "plus", activeUntilMs: 9000, status: "active", updatedAtMs: 1000 },
      quota: { ...siwc.quota, primary, updatedAtMs: 1000 },
    });
    const before = structuredClone([siwc, codex]);
    expect(accountMetadataSources(siwc, [siwc, codex])).toEqual({ plan: codex, quota: codex });
    expect([siwc, codex]).toEqual(before);
    expect(siwc.subscription.planType).toBeNull();
    expect(siwc.quota.primary).toBeNull();
    expect(siwc.routeInactive).toBe(false);
  });

  test("keeps SIWC's own plan and exhausted quota authoritative", () => {
    const siwc = metadataAccount("siwc", "siwc", {
      subscription: { planType: "free", activeUntilMs: null, status: "active", updatedAtMs: 1 },
      quota: { ...emptyQuota, primary: { ...primary, availableBasisPoints: 0 }, updatedAtMs: 1 },
    });
    const codex = metadataAccount("codex", "codex", {
      subscription: { ...siwc.subscription, planType: "plus", updatedAtMs: 2000 },
      quota: { ...siwc.quota, primary: { ...primary, observedAtMs: 2000 }, updatedAtMs: 2000 },
    });
    expect(accountMetadataSources(siwc, [siwc, codex])).toEqual({ plan: siwc, quota: siwc });
  });

  test("chooses the newest plan and complete quota snapshot independently without adding windows", () => {
    const siwc = metadataAccount("siwc", "siwc");
    const codex = metadataAccount("codex", "codex", {
      subscription: { ...siwc.subscription, planType: "plus", updatedAtMs: 1000 },
      quota: { ...siwc.quota, primary: { ...primary, observedAtMs: 2000 }, updatedAtMs: 2000 },
    });
    const excel = metadataAccount("excel", "excel_bps", {
      subscription: { ...siwc.subscription, planType: "pro", updatedAtMs: 3000 },
      quota: { ...siwc.quota, secondary: { ...primary, kind: "secondary" }, updatedAtMs: 1000 },
    });
    const display = accountMetadataSources(siwc, [excel, siwc, codex]);
    expect(display.plan).toBe(excel);
    expect(display.quota).toBe(codex);
    expect(display.quota.quota.secondary).toBeNull();
    expect(accountMetadataSources(siwc, [siwc, excel])).toEqual({ plan: excel, quota: excel });
  });

  test("never borrows from unrelated, detached, or recovery-owned accounts", () => {
    const siwc = metadataAccount("siwc", "siwc");
    const source = metadataAccount("codex", "codex", {
      subscription: { ...siwc.subscription, planType: "plus" }, quota: { ...siwc.quota, primary },
    });
    for (const donor of [
      { ...source, routeGroupId: "other" },
      { ...source, routeGroupId: null },
      { ...source, recoveryRequired: true },
      { ...source, secretAvailable: false },
      { ...source, enabled: false },
      { ...source, health: "blocked" as const },
      { ...source, authState: { state: "requires_reauth" } },
      { ...source, authState: { state: "error" } },
      { ...source, routingBlockReason: "reauth_required" as const },
    ]) expect(accountMetadataSources(siwc, [siwc, donor])).toEqual({ plan: siwc, quota: siwc });
    const ungrouped = { ...siwc, routeGroupId: null };
    expect(accountMetadataSources(ungrouped, [ungrouped, source])).toEqual({ plan: ungrouped, quota: ungrouped });
  });

  test("fills only missing metadata and leaves other issuing clients unchanged", () => {
    const codex = metadataAccount("codex", "codex", { quota: { ...emptyQuota, primary, updatedAtMs: 1000 } });
    const siwc = metadataAccount("siwc", "siwc", { subscription: { ...codex.subscription, planType: "pro" } });
    expect(accountMetadataSources(siwc, [siwc, codex])).toEqual({ plan: siwc, quota: codex });
    expect(accountMetadataSources(codex, [siwc, codex])).toEqual({ plan: codex, quota: codex });
    expect(accountMetadataSources(siwc, [siwc])).toEqual({ plan: siwc, quota: siwc });
  });

  test("a reported SIWC limit cannot be replaced by another client's available quota", () => {
    const siwc = metadataAccount("siwc", "siwc", { quota: { ...emptyQuota, limitReached: true } });
    const source = metadataAccount("codex", "codex", { quota: { ...emptyQuota, primary } });
    expect(accountMetadataSources(siwc, [siwc, source]).quota).toBe(siwc);
  });

  test("does not turn a Fast-only signal or provider balance into shared quota windows", () => {
    const siwc = metadataAccount("siwc", "siwc");
    const source = metadataAccount("codex", "codex", {
      quota: { ...emptyQuota, updatedAtMs: 9000, availableCreditsMicroUnits: 1_000_000,
        supplemental: [{ id: "fast", label: "Fast", serviceTier: "fast", window: primary }] },
    });
    expect(accountMetadataSources(siwc, [siwc, source]).quota).toBe(siwc);
    source.quota.supplemental = [{ id: "review", label: "Code review", window: primary }];
    expect(accountMetadataSources(siwc, [siwc, source]).quota).toBe(source);
  });

  test("selected-route recovery and reauthentication cannot use donor metadata or refresh", () => {
    const donor = metadataAccount("codex", "codex", { quota: { ...emptyQuota, primary } });
    for (const overrides of [{ recoveryRequired: true }, { authState: { state: "requires_reauth" } }]) {
      const siwc = metadataAccount("siwc", "siwc", overrides);
      expect(accountMetadataSources(siwc, [siwc, donor])).toEqual({ plan: siwc, quota: siwc });
    }
  });

  test("quota source selection follows window observations, not unrelated snapshot updates", () => {
    const siwc = metadataAccount("siwc", "siwc");
    const old = metadataAccount("codex", "codex", { quota: { ...emptyQuota, primary, updatedAtMs: 9000 } });
    const newer = metadataAccount("excel", "excel_bps", { quota: { ...emptyQuota, primary: { ...primary, observedAtMs: 2000 }, updatedAtMs: 2000 } });
    expect(accountMetadataSources(siwc, [siwc, old, newer]).quota).toBe(newer);
    expect(accountMetadataObservation(old, "quota").observedAtMs).toBe(1000);
  });
});

describe("linked metadata observation freshness", () => {
  const observed = 1_000_000;
  const source = metadataAccount("codex", "codex", {
    subscription: { planType: "plus", activeUntilMs: null, status: "active", updatedAtMs: observed },
    quota: { ...emptyQuota, primary: { ...primary, resetAtMs: null, observedAtMs: observed }, updatedAtMs: observed },
    refreshState: { quota: "fresh", models: "fresh" },
  });
  test("expires locally at twenty minutes without polling", () => {
    for (const resource of ["plan", "quota"] as const) {
      const deadline = accountMetadataObservation(source, resource).staleAtMs!;
      expect(deadline).toBe(observed + 20 * 60_000);
      expect(accountMetadataIsStale(source, resource, deadline - 1)).toBe(false);
      expect(accountMetadataIsStale(source, resource, deadline)).toBe(true);
    }
  });
  test("reset and subscription expiry take precedence over the age bound", () => {
    const expiring = { ...source,
      subscription: { ...source.subscription, activeUntilMs: observed + 500 },
      quota: { ...source.quota, primary: { ...source.quota.primary!, resetAtMs: observed + 300 } },
    };
    expect(accountMetadataObservation(expiring, "quota").staleAtMs).toBe(observed + 300);
    expect(accountMetadataIsStale(expiring, "quota", observed + 300)).toBe(true);
    expect(accountMetadataIsStale(expiring, "plan", observed + 500)).toBe(true);
  });
  test("missing, future, and invalid observation times never claim freshness", () => {
    for (const time of [null, 0, NaN, Infinity, 9e15, observed + 1]) {
      const invalid = { ...source, subscription: { ...source.subscription, updatedAtMs: time } };
      expect(accountMetadataIsStale(invalid, "plan", observed)).toBe(true);
    }
    const unknown = { ...source, quota: { ...source.quota, primary: { ...source.quota.primary!, observedAtMs: 0 } } };
    expect(accountMetadataObservation(unknown, "quota").observedAtMs).toBeNull();
    expect(accountMetadataIsStale(unknown, "quota", observed)).toBe(true);
    const futureWindow = { ...source, quota: { ...source.quota,
      secondary: { ...source.quota.primary!, kind: "secondary" as const, observedAtMs: observed + 1 },
    } };
    expect(accountMetadataObservation(futureWindow, "quota").observedAtMs).toBe(observed);
    expect(accountMetadataIsStale(futureWindow, "quota", observed)).toBe(true);
  });
  test("the oldest displayed window controls freshness, not a newer snapshot or Fast window", () => {
    const mixed = { ...source, quota: { ...source.quota, updatedAtMs: observed + 5_000,
      secondary: { ...source.quota.primary!, kind: "secondary" as const, observedAtMs: 1 },
      supplemental: [{ id: "fast", label: "Fast", serviceTier: "fast" as const, window: { ...primary, observedAtMs: 0 } }],
    } };
    expect(accountMetadataObservation(mixed, "quota").observedAtMs).toBe(1);
    expect(accountMetadataIsStale(mixed, "quota", observed + 20 * 60_000)).toBe(true);
    expect(accountMetadataIsStale({ ...mixed, quota: { ...mixed.quota, secondary: null } }, "quota", observed)).toBe(false);
  });
  test("failed or stale backend evidence stays stale even with recent timestamps", () => {
    expect(accountMetadataIsStale({ ...source, quotaRefreshStatus: "failed" }, "quota", observed)).toBe(true);
    expect(accountMetadataIsStale({ ...source, refreshState: { models: "fresh", quota: "stale" } }, "quota", observed)).toBe(true);
    expect(accountMetadataIsStale({ ...source, refreshState: { models: "fresh", quota: "unknown" } }, "quota", observed)).toBe(true);
  });
});

describe("linked quota presentation", () => {
  const now = 1_000_000;
  const source = metadataAccount("codex", "codex", {
    quota: { ...emptyQuota, primary: { ...primary, resetAtMs: now + 60_000, observedAtMs: now },
      secondary: { ...primary, kind: "secondary", windowMinutes: 10_080,
        availableBasisPoints: 0, resetAtMs: now + 86_400_000, observedAtMs: now }, updatedAtMs: now },
    refreshState: { quota: "fresh", models: "fresh" },
  });

  test("fresh weekly exhaustion describes the reader without mutating SIWC", () => {
    const siwc = metadataAccount("siwc", "siwc", { enabled: true, operationalStatus: "rotation" });
    const inventory = [siwc, source];
    const before = structuredClone(inventory);
    expect(accountMetadataSources(siwc, inventory, now).quota).toBe(source);
    expect(linkedAccountQuotaState(source, now)).toBe("weekly_exhausted");
    expect(inventory).toEqual(before);
    expect(siwc.quota.limitReached).toBe(false);
    expect(siwc.operationalStatus).toBe("rotation");
    expect(linkedAccountQuotaIndicator(siwc, source, now, "ready")).toEqual({ tone: "warning", warning: true });
  });

  test("linked warnings preserve errors, disabled state and recovery priority", () => {
    const siwc = metadataAccount("siwc", "siwc");
    for (const tone of ["error", "disabled", "info"] as const) {
      expect(linkedAccountQuotaIndicator(siwc, source, now, tone)).toEqual({ tone, warning: false });
    }
    expect(linkedAccountQuotaIndicator(source, source, now, "ready")).toEqual({ tone: "ready", warning: false });
    expect(linkedAccountQuotaIndicator(siwc, source, now + 60_000, "ready")).toEqual({ tone: "warning", warning: true });
    const available = { ...source, quota: { ...source.quota, secondary: null } };
    expect(linkedAccountQuotaIndicator(siwc, available, now, "ready")).toEqual({ tone: "ready", warning: false });
  });

  test("stale, failed, future and expired zeroes are not current exhaustion", () => {
    expect(linkedAccountQuotaState(source, now + 20 * 60_000)).toBe("stale");
    expect(linkedAccountQuotaState(source, now + 60_000)).toBe("stale");
    expect(linkedAccountQuotaState(source, now - 1)).toBe("stale");
    expect(linkedAccountQuotaState({ ...source, quotaRefreshStatus: "failed" }, now)).toBe("stale");
    expect(linkedAccountQuotaState({ ...source, refreshState: { quota: "stale", models: "fresh" } }, now)).toBe("stale");
  });

  test("an exhausted short window or reported limit keeps a generic label", () => {
    const short = { ...source, quota: { ...source.quota, secondary: null,
      primary: { ...source.quota.primary!, availableBasisPoints: 0 } } };
    expect(linkedAccountQuotaState(short, now)).toBe("exhausted");
    expect(linkedAccountQuotaState({ ...source, quota: { ...source.quota, secondary: null, limitReached: true } }, now)).toBe("exhausted");
  });

  test("unknown, supplemental and rounded-down values do not exhaust ordinary quota", () => {
    for (const remaining of [null, 1, 49, 1000]) {
      const quota = { ...source.quota, secondary: { ...source.quota.secondary!, availableBasisPoints: remaining } };
      expect(linkedAccountQuotaState({ ...source, quota }, now)).toBe("available");
    }
    expect(linkedAccountQuotaState({ ...source, quota: { ...source.quota, secondary: null,
      supplemental: [{ id: "review", label: "Code review", window: { ...source.quota.primary!, availableBasisPoints: 0 } }] } }, now)).toBe("available");
  });
});
