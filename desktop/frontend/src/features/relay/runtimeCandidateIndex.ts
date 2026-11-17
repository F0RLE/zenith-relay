import type { CandidateRuntimeSnapshot } from "./api/types";
import { activeModelCounts, activeRequestCount } from "./routingOrder";
import { modelIdKey } from "./modelGroups";

type CandidateKind = CandidateRuntimeSnapshot["kind"];
type CandidateIdentity = { candidateId: string; kind?: CandidateKind };

/** Index one snapshot, preserving candidate order and source/protocol grouping. */
export function indexCandidatesByMember<T extends CandidateIdentity>(candidates: Iterable<T>) {
  const accounts = new Map<string, T[]>();
  const sources = new Map<string, T[]>();
  const empty: T[] = [];
  const append = (index: Map<string, T[]>, id: string, candidate: T) => {
    const group = index.get(id);
    if (group) group.push(candidate);
    else index.set(id, [candidate]);
  };
  for (const candidate of candidates) {
    const kind = candidate.kind;
    // Activity events lack kind; exact account IDs and source prefixes still
    // follow the same membership rules as their eventual runtime snapshot.
    if (kind !== "api_source") append(accounts, candidate.candidateId, candidate);
    if (kind === "oauth_account") continue;
    let id = candidate.candidateId;
    while (true) {
      append(sources, id, candidate);
      const separator = id.lastIndexOf("::");
      if (separator < 0) break;
      id = id.slice(0, separator);
    }
  }
  return (memberId: string, kind: CandidateKind) => (kind === "oauth_account" ? accounts : sources).get(memberId) ?? empty;
}

/** Aggregate only the indexed candidates belonging to this member. */
export function createRuntimeCandidateLookup(order: readonly CandidateRuntimeSnapshot[]) {
  const candidatesFor = indexCandidatesByMember(order);
  return (
    memberId: string,
    kind: CandidateKind,
    protocolFilter: Parameters<typeof runtimeCandidateForMember>[3] = "all",
    sourceWireApi?: Parameters<typeof runtimeCandidateForMember>[4],
  ) => runtimeCandidateForMember(memberId, kind, candidatesFor(memberId, kind), protocolFilter, sourceWireApi);
}

/** Resolve account or source/protocol state for the lazy member-card pages. */
export function runtimeCandidateForMember(
  memberId: string,
  memberKind: "api_source" | "oauth_account",
  runtimeCandidates: CandidateRuntimeSnapshot[],
  protocolFilter: "responses" | "all" = "all",
  sourceWireApi?: "responses" | "chat_completions" | "messages" | "gemini" | "gemini_interactions",
): CandidateRuntimeSnapshot | undefined {
  const memberCandidates = runtimeCandidates.filter((candidate) => candidate.kind === memberKind && (
    memberKind === "oauth_account"
      ? candidate.candidateId === memberId
      : (candidate.candidateId === memberId || candidate.candidateId.startsWith(`${memberId}::`))
        && (protocolFilter === "all" || isResponsesCandidate(candidate.candidateId, sourceWireApi))
  ));
  if (!memberCandidates.length) return undefined;
  const firstCandidate = memberCandidates[0];
  if (memberCandidates.length === 1 && firstCandidate?.candidateId === memberId) return firstCandidate;
  const memberActiveRequestCount = memberCandidates.reduce((total, candidate) => total + activeRequestCount(candidate), 0);
  const selectionRanks = memberCandidates.flatMap((candidate) => candidate.selectionRank == null ? [] : [candidate.selectionRank]);
  return {
    candidateId: memberId,
    kind: memberKind,
    available: memberCandidates.some((candidate) => candidate.available),
    quotaWait: memberCandidates.every((candidate) => !candidate.available) && memberCandidates.some((candidate) => candidate.quotaWait),
    capacityWait: memberCandidates.every((candidate) => !candidate.available) && memberCandidates.some((candidate) => candidate.capacityWait),
    nextForNewRequest: memberCandidates.some((candidate) => candidate.nextForNewRequest),
    selectionRank: selectionRanks.length ? Math.min(...selectionRanks) : null,
    ...(firstCandidate?.activityRevision == null ? {} : { activityRevision: firstCandidate.activityRevision }),
    ...(firstCandidate?.runtimeId == null ? {} : { runtimeId: firstCandidate.runtimeId }),
    inFlight: memberActiveRequestCount,
    activeRequestCount: memberActiveRequestCount,
    activeModels: activeModelCounts(memberCandidates),
    modelRetries: aggregateModelRetries(memberCandidates),
    lastUsedAtMs: memberCandidates.reduce<number | null>((latestUsageAtMs, candidate) =>
      candidate.lastUsedAtMs != null && (latestUsageAtMs == null || candidate.lastUsedAtMs > latestUsageAtMs) ? candidate.lastUsedAtMs : latestUsageAtMs, null),
    nextRetryAtMs: memberCandidates.reduce<number | null>((earliestRetryAtMs, candidate) =>
      candidate.nextRetryAtMs != null && (earliestRetryAtMs == null || candidate.nextRetryAtMs < earliestRetryAtMs) ? candidate.nextRetryAtMs : earliestRetryAtMs, null),
    halfOpen: memberCandidates.some((candidate) => candidate.halfOpen),
    dispatches: memberCandidates.reduce((total, candidate) => total + candidate.dispatches, 0),
  };
}

function isResponsesCandidate(candidateId: string, sourceWireApi?: "responses" | "chat_completions" | "messages" | "gemini" | "gemini_interactions") {
  const separator = candidateId.indexOf("::");
  if (separator < 0) return sourceWireApi == null || sourceWireApi === "responses";
  const suffix = candidateId.slice(separator + 2);
  return suffix === "responses" || suffix.startsWith("responses_");
}

function aggregateModelRetries(candidates: CandidateRuntimeSnapshot[]) {
  const earliestRetryByModel = new Map<string, { model: string; retryAtMs: number }>();
  for (const candidate of candidates) {
    for (const retry of candidate.modelRetries ?? []) {
      if (!retry.model || !Number.isFinite(retry.retryAtMs)) continue;
      const key = modelIdKey(retry.model);
      const earliestRetry = earliestRetryByModel.get(key);
      if (!earliestRetry || retry.retryAtMs < earliestRetry.retryAtMs) {
        earliestRetryByModel.set(key, { model: retry.model, retryAtMs: retry.retryAtMs });
      }
    }
  }
  return [...earliestRetryByModel.values()].sort((left, right) => left.retryAtMs - right.retryAtMs || left.model.localeCompare(right.model));
}
