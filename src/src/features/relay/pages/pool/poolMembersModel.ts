import type { AccountSummary, CandidateRuntimeSnapshot, RuntimeSnapshot } from "../../api/types";
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
  activeRuntime: CandidateRuntimeSnapshot[];
  activeRequestTotal: number;
  activeModels: ReturnType<typeof activeModelCounts>;
  lastUsedRuntime: CandidateRuntimeSnapshot | null;
  lastUsedMember: PoolMember | null;
  nextMember: PoolMember | null;
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
): PoolActivityState {
  const activeMembers = members.filter((member) => activeRequestCount(runtimeByMember.get(member.id)) > 0);
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
  const nextMember = members.find((member) => runtimeByMember.get(member.id)?.available) ?? null;
  return {
    activeMembers,
    activeRuntime,
    activeRequestTotal,
    activeModels,
    lastUsedRuntime,
    lastUsedMember,
    nextMember,
  };
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
