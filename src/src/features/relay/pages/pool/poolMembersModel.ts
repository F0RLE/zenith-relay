import type { AccountSummary, CandidateRuntimeSnapshot, RuntimeActivitySnapshot, RuntimeActivityState, RuntimeSnapshot } from "../../api/types";
import { currentAccountErrorCode } from "../../accountStatus";
import { providerCreditsSummary, type ProviderCreditsSummary } from "../../providerCredits";
import {
  activeModelCounts,
  activeRequestCount,
  applyRuntimeActivities,
  routingOrderPositions,
  runtimeCandidateForMember,
} from "../../routingOrder";
import { comparePoolMembers, type PoolMember } from "../../poolHelpers";

export type PoolMemberStatusCounts = {
  rotation: number;
  quotaWait: number;
  errors: number;
  disabled: number;
};

export type PoolProviderCreditsSummary = ProviderCreditsSummary;

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
      .filter((item) => item.inPool)
      .map((item) => ({ ...item, kind: "account" as const })),
    ...runtime.sources
      .filter((item) => item.inPool)
      .map((item) => ({ ...item, kind: "source" as const })),
  ];
}

export function poolMemberRuntimeStates(
  members: readonly PoolMember[],
  runtimeOrder: CandidateRuntimeSnapshot[],
  activity?: RuntimeActivityState,
) {
  const activityOrder = activity
    ? applyRuntimeActivities(runtimeOrder, Object.values(activity.candidates))
    : runtimeOrder;
  return new Map(members.map((member) => [
    member.id,
    runtimeStateForMember(
      member,
      activityOrder,
      activity,
    ),
  ]));
}

function runtimeStateForMember(
  member: PoolMember,
  runtimeOrder: CandidateRuntimeSnapshot[],
  activity?: RuntimeActivityState,
) {
  const runtimeState = runtimeCandidateForMember(
      member.id,
      member.kind === "source" ? "api_source" : "oauth_account",
      runtimeOrder,
      "all",
      member.kind === "source" ? member.wireApi : undefined,
  );
  if (runtimeState) return runtimeState;

  // The reserve event can be delivered before an old runtime snapshot lists a
  // newly active candidate. Do not hide real in-flight work merely because
  // that snapshot is behind; create the smallest display state from the event.
  const activities = Object.values(activity?.candidates ?? {})
    .filter((candidate) => memberBelongsToCandidateId(member, candidate.candidateId));
  if (!activities.some((candidate) => candidate.activeRequestCount > 0)) return undefined;

  return activityRuntimeState(member, activities);
}

function activityRuntimeState(
  member: PoolMember,
  activities: readonly RuntimeActivitySnapshot[],
): CandidateRuntimeSnapshot {
  const activeModels = activeModelCounts(activities.map((activity) => ({
    ...activity,
    kind: member.kind === "source" ? "api_source" as const : "oauth_account" as const,
    available: true,
    lastUsedAtMs: null,
    nextRetryAtMs: null,
    halfOpen: false,
    dispatches: 0,
  })));
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

export function orderedPoolMembers(
  members: readonly PoolMember[],
  runtimeOrder: CandidateRuntimeSnapshot[],
) {
  const orderByMember = routingOrderPositions(runtimeOrder);
  return [...members].sort((left, right) => comparePoolMembers(left, right, orderByMember));
}

export function poolMemberSourceIds(members: readonly PoolMember[]) {
  return members
    .filter((member) => member.kind === "source")
    .map((member) => member.id)
    .sort()
    .join("\n");
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
  const activeMemberIds = new Set(activeMembers.map((member) => `${member.kind}:${member.id}`));
  const nextMember = effectiveOrder
    .filter((candidate) => candidate.available && activeRequestCount(candidate) === 0)
    .map((candidate) => members.find((member) =>
      candidateBelongsToMember(member, candidate)
      && memberCanRoute(member, visibleModelIds),
    ))
    .find((member): member is PoolMember => member != null && !activeMemberIds.has(`${member.kind}:${member.id}`)) ?? null;
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
    ? members.find((member) => runtimeByMember.get(member.id)?.lastUsedAtMs === lastUsedRuntime.lastUsedAtMs) ?? null
    : null;
  const lastActivityMember = activity?.lastCandidateId
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
  const activities = Object.values(activity.candidates);
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

export function memberCanRoute(member: PoolMember, visibleModelIds?: readonly string[]) {
  if (!member.inPool || !member.enabled || member.draining || member.operationalStatus !== "rotation") return false;
  if (member.kind === "source" && !member.secretAvailable) return false;
  if (member.kind === "account" && (!member.secretAvailable || !member.proxyAvailable)) return false;
  if (visibleModelIds == null) return true;
  const visible = new Set(visibleModelIds.map((model) => model.toLowerCase()));
  return member.models.some((model) => visible.has(model.toLowerCase()));
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

export function poolMemberStatusCounts(members: readonly PoolMember[]): PoolMemberStatusCounts {
  const statuses = members.map((member) => member.operationalStatus);
  return {
    rotation: statuses.filter((status) => status === "rotation").length,
    quotaWait: statuses.filter((status) => status === "quotaWait").length,
    errors: members.filter((member) => member.kind === "account"
      ? Boolean(currentAccountErrorCode(member as AccountSummary))
      : member.operationalStatus === "unavailable" || Boolean(member.lastErrorCode?.trim())).length,
    disabled: statuses.filter((status) => status === "disabled").length,
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
