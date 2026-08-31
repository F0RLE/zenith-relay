import type { AccountSummary, CandidateRuntimeSnapshot, RuntimeActivityState, RuntimeSnapshot } from "../../api/types";
import { currentAccountErrorCode } from "../../accountStatus";
import {
  activeModelCounts,
  activeRequestCount,
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
) {
  return new Map(members.map((member) => [
    member.id,
    runtimeCandidateForMember(
      member.id,
      member.kind === "source" ? "api_source" : "oauth_account",
      runtimeOrder,
      "all",
      member.kind === "source" ? member.wireApi : undefined,
    ),
  ]));
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
  const activeMembers = members.filter((member) => activeRequestCount(runtimeByMember.get(member.id)) > 0);
  const activeMemberIds = new Set(activeMembers.map((member) => `${member.kind}:${member.id}`));
  const nextMember = runtimeOrder
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
  const lastUsedRuntime = runtimeOrder.reduce<CandidateRuntimeSnapshot | null>((latest, candidate) => (
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
