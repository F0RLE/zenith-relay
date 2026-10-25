import type { AccountSummary, CandidateRuntimeSnapshot, OperationalStatus, PoolRoutingMode, RuntimeActivitySnapshot, RuntimeActivityState, RuntimeSnapshot } from "../../api/types";
import { accountDisplayStatus, currentAccountErrorCode } from "../../accountStatus";
import { providerCreditsSummary, type ProviderCreditsSummary } from "../../providerCredits";
import {
  activeModelCounts,
  activeRequestCount,
  applyRuntimeActivities,
  currentRuntimeActivities,
  routingOrderPositions,
  compareRoutingOrder,
} from "../../routingOrder";
import { createRuntimeCandidateLookup, indexCandidatesByMember } from "../../runtimeCandidateIndex";
import { compareStableText, memberName, type PoolMember } from "../../poolHelpers";
import { modelIdKey } from "../../modelGroups";

export type PoolMemberStatusCounts = {
  rotation: number;
  quotaWait: number;
  waiting: number;
  errors: number;
  disabled: number;
};

export type PoolProviderCreditsSummary = ProviderCreditsSummary;
type ResourceHeadroom = CandidateRuntimeSnapshot["resourceHeadroom"];
type PoolMemberDisplayStatus = OperationalStatus | "cooldown" | "busy" | "draining";
const DISPLAY_STATUS_ORDER: Record<PoolMemberDisplayStatus, number> = {
  rotation: 0, quotaWait: 1, cooldown: 1, busy: 1, unavailable: 2, disabled: 3, draining: 3,
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

export function orderedPoolMembers(
  members: readonly PoolMember[],
  runtimeOrder: CandidateRuntimeSnapshot[],
  savedMembers?: readonly { id: string }[],
  mode?: PoolRoutingMode | null,
  runtimeByMember: ReadonlyMap<string, CandidateRuntimeSnapshot | undefined> = poolMemberRuntimeStates(members, runtimeOrder),
) {
  const savedOrder = savedMemberPositions(savedMembers);
  const orderByMember = savedOrder.size > 0 ? savedOrder : routingOrderPositions(runtimeOrder);
  return [...members].sort((left, right) => compareDisplayedPoolMembers(left, right, orderByMember, mode, runtimeByMember));
}

function compareDisplayedPoolMembers(
  left: PoolMember,
  right: PoolMember,
  order: Map<string, number>,
  mode?: PoolRoutingMode | null,
  runtimeByMember?: ReadonlyMap<string, CandidateRuntimeSnapshot | undefined>,
) {
  const leftRuntime = runtimeByMember?.get(left.id);
  const rightRuntime = runtimeByMember?.get(right.id);
  // Match the scheduler's live order before ranking candidates for new work.
  const active = Number(activeRequestCount(rightRuntime) > 0) - Number(activeRequestCount(leftRuntime) > 0);
  if (active) return active;
  const leftStatus = poolMemberDisplayStatus(left, leftRuntime);
  const rightStatus = poolMemberDisplayStatus(right, rightRuntime);
  const status = DISPLAY_STATUS_ORDER[leftStatus] - DISPLAY_STATUS_ORDER[rightStatus];
  if (status) return status;
  if (leftStatus === "rotation") {
    const rank = (candidate: CandidateRuntimeSnapshot | undefined) =>
      candidate?.selectionRank ?? (candidate?.nextForNewRequest ? 0 : Number.MAX_SAFE_INTEGER);
    const preference = rank(leftRuntime) - rank(rightRuntime);
    if (preference) return preference;
    if (mode === "automatic") {
      const resources = compareRoutingRemainder(left, right, leftRuntime?.resourceHeadroom, rightRuntime?.resourceHeadroom)
        || compareCreditsWhenQuotaIsEmpty(left, right, leftRuntime?.resourceHeadroom, rightRuntime?.resourceHeadroom);
      if (resources) return resources;
    }
  } else if (DISPLAY_STATUS_ORDER[leftStatus] < DISPLAY_STATUS_ORDER.disabled) {
    const retry = (leftRuntime?.nextRetryAtMs ?? Number.MAX_SAFE_INTEGER)
      - (rightRuntime?.nextRetryAtMs ?? Number.MAX_SAFE_INTEGER);
    if (retry) return retry;
  }
  return compareRoutingOrder(left.id, right.id, order)
    || compareStableText(memberName(left), memberName(right));
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

/**
 * Positive known window remainder used only to place automatic cards.
 * Provider credits are a separate display key when no positive window exists.
 * Source balances are excluded from both display keys.
 */
export function memberRoutingRemainder(member: PoolMember, headroom?: ResourceHeadroom): number | null {
  if (headroom) {
    const remaining = headroom.quotaRemainingBasisPoints;
    return remaining != null && Number.isInteger(remaining) && remaining > 0 && remaining <= 10_000 ? remaining : null;
  }
  if (member.kind !== "account" || member.refreshState?.quota !== "fresh" || member.quota.limitReached) return null;
  const windows = [member.quota?.primary, member.quota?.secondary]
    .map((window) => window?.availableBasisPoints)
    .filter((basisPointRemainder): basisPointRemainder is number => basisPointRemainder != null
      && Number.isInteger(basisPointRemainder) && basisPointRemainder >= 0 && basisPointRemainder <= 10_000);
  const remaining = windows.length ? Math.min(...windows) : null;
  return remaining != null && remaining > 0 ? remaining : null;
}

function compareRoutingRemainder(left: PoolMember, right: PoolMember, leftHeadroom?: ResourceHeadroom, rightHeadroom?: ResourceHeadroom) {
  const leftRemainder = memberRoutingRemainder(left, leftHeadroom);
  const rightRemainder = memberRoutingRemainder(right, rightHeadroom);
  if (leftRemainder == null && rightRemainder == null) return 0;
  if (leftRemainder == null) return 1;
  if (rightRemainder == null) return -1;
  return rightRemainder - leftRemainder;
}

function compareCreditsWhenQuotaIsEmpty(left: PoolMember, right: PoolMember, leftHeadroom?: ResourceHeadroom, rightHeadroom?: ResourceHeadroom) {
  if (
    memberRoutingRemainder(left, leftHeadroom) != null
    || memberRoutingRemainder(right, rightHeadroom) != null
  ) return 0;
  const leftCredits = memberProviderCredits(left, leftHeadroom);
  const rightCredits = memberProviderCredits(right, rightHeadroom);
  if (leftCredits === "unlimited" && rightCredits === "unlimited") return 0;
  if (leftCredits === "unlimited") return -1;
  if (rightCredits === "unlimited") return 1;
  if (leftCredits == null && rightCredits == null) return 0;
  if (leftCredits == null) return 1;
  if (rightCredits == null) return -1;
  return rightCredits - leftCredits;
}

function memberProviderCredits(member: PoolMember, headroom?: ResourceHeadroom): number | "unlimited" | null {
  let credits: number | null | undefined;
  if (headroom) {
    if (headroom.providerCreditsUnlimited) return "unlimited";
    credits = headroom.providerCreditsMicroUnits;
  } else {
    if (member.kind !== "account" || member.refreshState?.quota !== "fresh" || member.quota.providerCreditsAvailable !== true) return null;
    if (member.quota.providerCreditsUnlimited === true) return "unlimited";
    credits = member.quota.availableCreditsMicroUnits;
  }
  return credits != null && Number.isSafeInteger(credits) && credits > 0
    ? credits
    : null;
}

function savedMemberPositions(members: readonly { id: string }[] | undefined) {
  const positions = new Map<string, number>();
  for (const member of members ?? []) {
    const memberId = member.id;
    if (memberId && !positions.has(memberId)) positions.set(memberId, positions.size);
  }
  return positions;
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
  let nextMember: PoolMember | null = null;
  for (const candidate of effectiveOrder) {
    if (!candidate.available || candidate.nextForNewRequest !== true) continue;
    nextMember = members.find((member) =>
      candidateBelongsToMember(member, candidate)
      && memberCanRoute(member, visibleModelIds)) ?? null;
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

export function memberCanRoute(member: PoolMember, visibleModelIds?: readonly string[]) {
  if (!member.inPool || !member.enabled || member.draining || member.operationalStatus !== "rotation") return false;
  if (member.kind === "source" && !member.secretAvailable) return false;
  if (member.kind === "account" && (!member.secretAvailable || !member.proxyAvailable)) return false;
  if (visibleModelIds == null) return true;
  const visible = new Set(visibleModelIds.map((model) => modelIdKey(model)));
  return member.models.some((model) => visible.has(modelIdKey(model)));
}

/** Missing route telemetry preserves configured-member availability.
 * The exact next candidate remains a separate scheduler-owned preview. */
export function poolRoutingAvailability(
  members: readonly PoolMember[],
  visibleModelIds: readonly string[],
  activeRequestTotal: number,
): "active" | "ready" | "noModels" | "unavailable" {
  if (activeRequestTotal > 0) return "active";
  if (members.some((member) => memberCanRoute(member, visibleModelIds))) return "ready";
  if (members.some((member) => memberCanRoute(member))) return "noModels";
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
    waiting: statuses.filter((status) => DISPLAY_STATUS_ORDER[status] === 1).length,
    errors: members.filter((member, index) => member.kind === "account"
      ? Boolean(currentAccountErrorCode(member as AccountSummary))
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
