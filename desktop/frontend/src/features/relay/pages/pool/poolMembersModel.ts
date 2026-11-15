import type { CandidateRuntimeSnapshot, OperationalStatus, RuntimeActivitySnapshot, RuntimeActivityState, RuntimeSnapshot } from "../../api/types";
import { accountDisplayStatus, currentAccountErrorCode } from "../../accountStatus";
import { providerCreditsSummary, type ProviderCreditsSummary } from "../../providerCredits";
import {
  activeModelCounts,
  activeRequestCount,
  applyRuntimeActivities,
  currentRuntimeActivities,
} from "../../routingOrder";
import { createRuntimeCandidateLookup, indexCandidatesByMember } from "../../runtimeCandidateIndex";
import type { PoolMember } from "../../poolHelpers";
import { modelIdKey } from "../../modelGroups";

export type PoolMemberStatusCounts = {
  rotation: number;
  quotaWait: number;
  waiting: number;
  errors: number;
  disabled: number;
};

export type PoolProviderCreditsSummary = ProviderCreditsSummary;
type PoolMemberDisplayStatus = OperationalStatus | "cooldown" | "busy" | "draining";

export type PoolActivityState = {
  activeMembers: PoolMember[];
  nextMember: PoolMember | null;
  activeRuntime: CandidateRuntimeSnapshot[];
  activeRequestTotal: number;
  activeModels: ReturnType<typeof activeModelCounts>;
  lastUsedRuntime: CandidateRuntimeSnapshot | null;
  lastUsedMember: PoolMember | null;
  lastActivityMember: PoolMember | null;
};

export function poolMembersFromRuntime(runtime: RuntimeSnapshot | null): PoolMember[] {
  if (!runtime) return [];
  return [
    ...runtime.accounts
      .filter((account) => account.inPool && !account.routeInactive)
      .map((account) => ({ ...account, kind: "account" as const })),
    ...runtime.sources
      .filter((source) => source.inPool)
      .map((source) => ({ ...source, kind: "source" as const })),
  ];
}

export function poolMemberRuntimeStates(
  members: readonly PoolMember[],
  runtimeOrder: CandidateRuntimeSnapshot[],
  activity?: RuntimeActivityState,
) {
  const activities = Object.values(activity?.candidates ?? {});
  const activityOrder = activity
    ? applyRuntimeActivities(runtimeOrder, activities)
    : runtimeOrder;
  const runtimeFor = createRuntimeCandidateLookup(activityOrder);
  const activityFor = indexCandidatesByMember(currentRuntimeActivities(activityOrder, activities));
  return new Map(members.map((member) => [
    member.id,
    runtimeStateForMember(
      member,
      runtimeFor,
      activityFor,
    ),
  ]));
}

function runtimeStateForMember(
  member: PoolMember,
  runtimeFor: ReturnType<typeof createRuntimeCandidateLookup>,
  activityFor: (id: string, kind: CandidateRuntimeSnapshot["kind"]) => RuntimeActivitySnapshot[],
) {
  const kind = member.kind === "source" ? "api_source" : "oauth_account";
  const runtimeState = runtimeFor(
      member.id,
      kind,
      "all",
      member.kind === "source" ? member.wireApi : undefined,
  );
  if (runtimeState) return runtimeState;

  // The reserve event can be delivered before an old runtime snapshot lists a
  // newly active candidate. Do not hide real in-flight work merely because
  // that snapshot is behind; create the smallest display state from the event.
  const activities = activityFor(member.id, kind);
  if (!activities.some((candidate) => candidate.activeRequestCount > 0)) return undefined;

  return activityRuntimeState(member, activities);
}

function activityRuntimeState(
  member: PoolMember,
  activities: readonly RuntimeActivitySnapshot[],
): CandidateRuntimeSnapshot {
  const activeModels = activeModelCounts(activities);
  const activeRequestCount = activities.reduce((total, activity) => total + activity.activeRequestCount, 0);
  return {
    candidateId: member.id,
    kind: member.kind === "source" ? "api_source" : "oauth_account",
    available: true,
    inFlight: activities.reduce((total, activity) => total + activity.inFlight, 0),
    activeRequestCount,
    activeModels,
    lastUsedAtMs: null,
    nextRetryAtMs: null,
    halfOpen: false,
    dispatches: 0,
  };
}

export function orderedPoolMembers<T extends { kind: "account" | "source"; id: string }>(
  members: readonly T[],
  memberOrder: readonly string[] = [],
): T[] {
  const remaining = new Map(members.map((member) => [`${member.kind}:${member.id}`, member]));
  const ordered: T[] = [];
  for (const key of memberOrder) {
    const member = remaining.get(key);
    if (member) {
      ordered.push(member);
      remaining.delete(key);
    }
  }
  // Retain newly observed inventory while its backend projection catches up.
  return [...ordered, ...remaining.values()];
}

/** Explicit live waits narrow readiness; a bare unavailable preview can be stale. */
export function poolMemberDisplayStatus(member: PoolMember, candidate?: CandidateRuntimeSnapshot): PoolMemberDisplayStatus {
  if (member.kind === "account") return accountDisplayStatus(member, candidate);
  if (!member.enabled) return "disabled";
  if (member.draining) return "draining";
  if (member.operationalStatus === "disabled") return "disabled";
  if (!member.secretAvailable) return "unavailable";
  if (candidate?.available === false) {
    if (candidate.quotaWait) return "quotaWait";
    if (candidate.nextRetryAtMs != null) return "cooldown";
    if (candidate.capacityWait) return "busy";
  }
  return member.operationalStatus;
}

export function poolActivityState(
  members: readonly PoolMember[],
  runtimeByMember: ReadonlyMap<string, CandidateRuntimeSnapshot | undefined>,
  runtimeOrder: readonly CandidateRuntimeSnapshot[],
  activity?: RuntimeActivityState,
  visibleModelIds?: readonly string[],
): PoolActivityState {
  const effectiveOrder = activityRuntimeOrder(runtimeOrder, activity, members);
  const activeMembers = members.filter((member) => activeRequestCount(runtimeByMember.get(member.id)) > 0);
  const canRoute = createMemberRouteCheck(visibleModelIds);
  let nextMember: PoolMember | null = null;
  for (const candidate of effectiveOrder) {
    if (!candidate.available || candidate.nextForNewRequest !== true) continue;
    nextMember = members.find((member) =>
      candidateBelongsToMember(member, candidate)
      && canRoute(member)) ?? null;
    if (nextMember) break;
  }
  const activeRuntime = activeMembers.flatMap((member) => {
    const candidate = runtimeByMember.get(member.id);
    return candidate ? [candidate] : [];
  });
  const activeRequestTotal = activeRuntime.reduce((total, candidate) => total + activeRequestCount(candidate), 0);
  const activeModels = activeModelCounts(activeRuntime);
  const lastUsedRuntime = effectiveOrder.reduce<CandidateRuntimeSnapshot | null>((latest, candidate) => (
    candidate.lastUsedAtMs != null
      && (latest?.lastUsedAtMs == null || candidate.lastUsedAtMs > latest.lastUsedAtMs)
      ? candidate
      : latest
  ), null);
  const lastUsedMember = lastUsedRuntime
    ? members.find((member) => candidateBelongsToMember(member, lastUsedRuntime)) ?? null
    : null;
  const lastActivityMember = activity?.lastCandidateId && (activity.runtimeId ?? 0) >= (runtimeOrder[0]?.runtimeId ?? 0)
    ? members.find((member) => memberBelongsToCandidateId(member, activity.lastCandidateId!)) ?? null
    : null;
  return {
    activeMembers,
    nextMember,
    activeRuntime,
    activeRequestTotal,
    activeModels,
    lastUsedRuntime,
    lastUsedMember,
    lastActivityMember,
  };
}

/**
 * Merge live activity into the scheduler order used for route presentation.
 * A reserve event can arrive before the next full runtime snapshot, so active
 * candidates absent from that snapshot are appended before the normal activity
 * sort. Release-only tombstones are intentionally not added.
 */
function activityRuntimeOrder(
  runtimeOrder: readonly CandidateRuntimeSnapshot[],
  activity?: RuntimeActivityState,
  members: readonly PoolMember[] = [],
) {
  if (!activity) return runtimeOrder;
  const activities = currentRuntimeActivities(runtimeOrder, Object.values(activity.candidates));
  const known = new Set(runtimeOrder.map((candidate) => candidate.candidateId));
  const missingActive = activities
    .filter((candidate) => candidate.activeRequestCount > 0 && !known.has(candidate.candidateId))
    .map((candidate) => ({
      candidateId: candidate.candidateId,
      kind: members.some((member) => member.kind === "source" && memberBelongsToCandidateId(member, candidate.candidateId))
        ? "api_source" as const
        : "oauth_account" as const,
      available: true,
      inFlight: candidate.inFlight,
      activeRequestCount: candidate.activeRequestCount,
      activeModels: candidate.activeModels,
      lastUsedAtMs: null,
      nextRetryAtMs: null,
      halfOpen: false,
      dispatches: 0,
    } satisfies CandidateRuntimeSnapshot));
  return applyRuntimeActivities([...runtimeOrder, ...missingActive], activities);
}

function memberIsRouteReady(member: PoolMember) {
  if (!member.inPool || !member.enabled || member.draining || member.operationalStatus !== "rotation") return false;
  if (member.kind === "source" && !member.secretAvailable) return false;
  if (member.kind === "account" && (!member.secretAvailable || !member.proxyAvailable)) return false;
  return true;
}

/** Share catalog normalization within this projection only. No eligible
 * candidate or next-route preview means no catalog work is needed. */
function createMemberRouteCheck(visibleModelIds?: readonly string[]) {
  let visible: Set<string> | undefined;
  return (member: PoolMember) => {
    if (!memberIsRouteReady(member)) return false;
    if (visibleModelIds == null) return true;
    const modelKeys = visible ??= new Set(visibleModelIds.map((model) => modelIdKey(model)));
    return member.models.some((model) => modelKeys.has(modelIdKey(model)));
  };
}

/** Missing route telemetry preserves configured-member availability.
 * The exact next candidate remains a separate scheduler-owned preview. */
export function poolRoutingAvailability(
  members: readonly PoolMember[],
  visibleModelIds: readonly string[],
  activeRequestTotal: number,
): "active" | "ready" | "noModels" | "unavailable" {
  if (activeRequestTotal > 0) return "active";
  if (members.some(createMemberRouteCheck(visibleModelIds))) return "ready";
  if (members.some(memberIsRouteReady)) return "noModels";
  return "unavailable";
}

function candidateBelongsToMember(member: PoolMember, candidate: CandidateRuntimeSnapshot) {
  return candidateKindMatchesMember(member, candidate.kind) && memberBelongsToCandidateId(member, candidate.candidateId);
}

function memberBelongsToCandidateId(member: PoolMember, candidateId: string) {
  if (member.kind === "account") return candidateId === member.id;
  return candidateId === member.id || candidateId.startsWith(`${member.id}::`);
}

function candidateKindMatchesMember(member: PoolMember, kind: CandidateRuntimeSnapshot["kind"]) {
  return member.kind === "account" ? kind === "oauth_account" : kind === "api_source";
}

export function poolMemberStatusCounts(
  members: readonly PoolMember[],
  runtimeByMember?: ReadonlyMap<string, CandidateRuntimeSnapshot | undefined>,
): PoolMemberStatusCounts {
  const statuses = members.map((member) => poolMemberDisplayStatus(member, runtimeByMember?.get(member.id)));
  return {
    rotation: statuses.filter((status) => status === "rotation").length,
    quotaWait: statuses.filter((status) => status === "quotaWait").length,
    waiting: statuses.filter((status) => status === "quotaWait" || status === "busy" || status === "cooldown").length,
    errors: members.filter((member, index) => member.kind === "account"
      ? Boolean(currentAccountErrorCode(member))
      : statuses[index] === "unavailable" || Boolean(member.lastErrorCode?.trim())).length,
    disabled: statuses.filter((status) => status === "disabled" || status === "draining").length,
  };
}

/**
 * Sums only provider-reported credits from accounts currently in the pool.
 * Reset credits, API-equivalent estimates, and source balances are separate
 * ledgers and must not appear in this total.
 */
export function poolProviderCreditsSummary(
  members: readonly PoolMember[],
): PoolProviderCreditsSummary | null {
  return providerCreditsSummary(
    members
      .filter((member): member is Extract<PoolMember, { kind: "account" }> => member.kind === "account" && member.inPool),
  );
}
