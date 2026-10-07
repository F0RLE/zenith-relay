import type { CandidateRuntimeSnapshot, RuntimeActivitySnapshot } from "./api/types";
import { modelIdKey } from "./modelGroups";

export function sameRuntimeOrder(
  left: readonly CandidateRuntimeSnapshot[],
  right: readonly CandidateRuntimeSnapshot[],
) {
  return left.length === right.length && left.every((candidate, index) => sameRuntimeCandidate(candidate, right[index]));
}

function sameRuntimeCandidate(left: CandidateRuntimeSnapshot | undefined, right: CandidateRuntimeSnapshot | undefined) {
  if (!left || !right) return false;
  return left.candidateId === right.candidateId
    && left.kind === right.kind
    && left.available === right.available
    && left.nextForNewRequest === right.nextForNewRequest
    && left.activityRevision === right.activityRevision
    && left.runtimeId === right.runtimeId
    && left.inFlight === right.inFlight
    && left.activeRequestCount === right.activeRequestCount
    && left.lastUsedAtMs === right.lastUsedAtMs
    && left.nextRetryAtMs === right.nextRetryAtMs
    && left.halfOpen === right.halfOpen
    && left.dispatches === right.dispatches
    && sameCounts(left.activeModels, right.activeModels)
    && sameRetries(left.modelRetries, right.modelRetries);
}

function sameCounts(
  left: CandidateRuntimeSnapshot["activeModels"],
  right: CandidateRuntimeSnapshot["activeModels"],
) {
  const leftModels = left ?? [];
  const rightModels = right ?? [];
  return leftModels.length === rightModels.length
    && leftModels.every((leftModel, index) => leftModel.model === rightModels[index]?.model
      && leftModel.requestCount === rightModels[index]?.requestCount);
}

function sameRetries(
  left: CandidateRuntimeSnapshot["modelRetries"],
  right: CandidateRuntimeSnapshot["modelRetries"],
) {
  const leftRetries = left ?? [];
  const rightRetries = right ?? [];
  return leftRetries.length === rightRetries.length
    && leftRetries.every((leftRetry, index) => leftRetry.model === rightRetries[index]?.model
      && leftRetry.retryAtMs === rightRetries[index]?.retryAtMs);
}

export function routingOrderPositions(order: CandidateRuntimeSnapshot[]) {
  const positions = new Map<string, number>();
  const sourcePositions = new Map<string, { index: number; active: boolean }>();
  for (const [index, candidate] of order.entries()) {
    positions.set(candidate.candidateId, index);
    if (candidate.kind !== "api_source") continue;
    const separator = candidate.candidateId.indexOf("::");
    if (separator > 0) {
      const sourceId = candidate.candidateId.slice(0, separator);
      // A source card represents all of its protocol candidates. Prefer the
      // protocol route that is actually carrying traffic, then fall back to
      // the first route supplied by the scheduler. This keeps a multi-
      // protocol source card aligned with the active route instead of pinning
      // it to whichever binding happened to be serialized first.
      const active = activeRequestCount(candidate) > 0;
      const previous = sourcePositions.get(sourceId);
      if (!previous || (active && !previous.active) || (active === previous.active && index < previous.index)) {
        sourcePositions.set(sourceId, { index, active });
      }
    }
  }
  for (const [sourceId, { index }] of sourcePositions) positions.set(sourceId, index);
  return positions;
}

/** Resolve the runtime state shown for a pool member. Sources may have one
 * runtime candidate per protocol binding (`sourceId::responses`), while the
 * UI card is keyed only by the source id. */
export function runtimeCandidateForMember(
  memberId: string,
  kind: "api_source" | "oauth_account",
  order: CandidateRuntimeSnapshot[],
  protocol: "responses" | "all" = "all",
  legacyWireApi?: "responses" | "chat_completions" | "messages" | "gemini",
): CandidateRuntimeSnapshot | undefined {
  const candidates = order.filter((candidate) => candidate.kind === kind && (
    kind === "oauth_account"
      ? candidate.candidateId === memberId
      : (candidate.candidateId === memberId || candidate.candidateId.startsWith(`${memberId}::`))
        && (protocol === "all" || isResponsesCandidate(candidate.candidateId, legacyWireApi))
  ));
  if (!candidates.length) return undefined;
  const firstCandidate = candidates[0];
  if (candidates.length === 1 && firstCandidate?.candidateId === memberId) return firstCandidate;
  return {
    candidateId: memberId,
    kind,
    available: candidates.some((candidate) => candidate.available),
    nextForNewRequest: candidates.some((candidate) => candidate.nextForNewRequest),
    ...(firstCandidate?.activityRevision == null ? {} : { activityRevision: firstCandidate.activityRevision }),
    ...(firstCandidate?.runtimeId == null ? {} : { runtimeId: firstCandidate.runtimeId }),
    inFlight: candidates.reduce((total, candidate) => total + activeRequestCount(candidate), 0),
    activeRequestCount: candidates.reduce((total, candidate) => total + activeRequestCount(candidate), 0),
    activeModels: activeModelCounts(candidates),
    modelRetries: aggregateModelRetries(candidates),
    lastUsedAtMs: candidates.reduce<number | null>((latest, candidate) =>
      candidate.lastUsedAtMs != null && (latest == null || candidate.lastUsedAtMs > latest) ? candidate.lastUsedAtMs : latest, null),
    nextRetryAtMs: candidates.reduce<number | null>((earliest, candidate) =>
      candidate.nextRetryAtMs != null && (earliest == null || candidate.nextRetryAtMs < earliest) ? candidate.nextRetryAtMs : earliest, null),
    halfOpen: candidates.some((candidate) => candidate.halfOpen),
    dispatches: candidates.reduce((total, candidate) => total + candidate.dispatches, 0),
  };
}

function isResponsesCandidate(candidateId: string, legacyWireApi?: "responses" | "chat_completions" | "messages" | "gemini") {
  const separator = candidateId.indexOf("::");
  if (separator < 0) return legacyWireApi == null || legacyWireApi === "responses";
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

export function compareRoutingOrder(leftId: string, rightId: string, order: ReadonlyMap<string, number>, fallback?: ReadonlyMap<string, number>) {
  const left = order.get(leftId);
  const right = order.get(rightId);
  if (left != null || right != null) return (left ?? Number.MAX_SAFE_INTEGER) - (right ?? Number.MAX_SAFE_INTEGER);
  return (fallback?.get(leftId) ?? Number.MAX_SAFE_INTEGER) - (fallback?.get(rightId) ?? Number.MAX_SAFE_INTEGER);
}

export function activeRequestCount(candidate: CandidateRuntimeSnapshot | undefined) {
  return candidate?.activeRequestCount ?? candidate?.inFlight ?? 0;
}

/** Returns only currently active per-model cooldowns in their display order. */
export function upcomingModelRetries(candidate: CandidateRuntimeSnapshot | undefined, nowMs: number) {
  return [...(candidate?.modelRetries ?? [])]
    .filter((retry) => retry.retryAtMs > nowMs)
    .sort((left, right) => left.retryAtMs - right.retryAtMs);
}

/** Apply a host activity event without waiting for the next full snapshot. */
export function applyRuntimeActivity(
  order: CandidateRuntimeSnapshot[],
  activity: RuntimeActivitySnapshot,
) {
  return applyRuntimeActivities(order, [activity]);
}

export function compareRuntimeActivity(
  left: { runtimeId?: number | undefined; revision: number },
  right: { runtimeId?: number | undefined; revision: number },
) {
  return (left.runtimeId ?? 0) - (right.runtimeId ?? 0) || left.revision - right.revision;
}

export function currentRuntimeActivities(
  order: readonly CandidateRuntimeSnapshot[],
  activities: Iterable<RuntimeActivitySnapshot>,
) {
  const runtimeId = order[0]?.runtimeId;
  return [...activities].filter((activity) => runtimeId == null || activity.runtimeId == null || activity.runtimeId >= runtimeId);
}

/** A late poll must not resurrect activity already retired by a newer snapshot. */
export function preferNewerRuntimeOrder(currentOrder: CandidateRuntimeSnapshot[], incomingOrder: CandidateRuntimeSnapshot[]) {
  const currentSnapshot = currentOrder[0];
  const incomingSnapshot = incomingOrder[0];
  if (currentSnapshot?.runtimeId == null || currentSnapshot.activityRevision == null
    || incomingSnapshot?.runtimeId == null || incomingSnapshot.activityRevision == null) return incomingOrder;
  return compareRuntimeActivity(
    { runtimeId: currentSnapshot.runtimeId, revision: currentSnapshot.activityRevision },
    { runtimeId: incomingSnapshot.runtimeId, revision: incomingSnapshot.activityRevision },
  ) > 0 ? currentOrder : incomingOrder;
}

export function applyRuntimeActivities(
  order: CandidateRuntimeSnapshot[],
  activities: Iterable<RuntimeActivitySnapshot>,
) {
  const latestActivityByCandidate = new Map<string, RuntimeActivitySnapshot>();
  for (const activity of currentRuntimeActivities(order, activities)) {
    const previousActivity = latestActivityByCandidate.get(activity.candidateId);
    if (!previousActivity || compareRuntimeActivity(activity, previousActivity) > 0) {
      latestActivityByCandidate.set(activity.candidateId, activity);
    }
  }
  if (!latestActivityByCandidate.size) return order;

  const snapshotRevision = order.reduce((revision, candidate) => Math.min(revision, candidate.activityRevision ?? 0), Infinity);
  const activityAheadOfSnapshot = [...latestActivityByCandidate.values()].some((activity) =>
    (activity.runtimeId ?? 0) > (order[0]?.runtimeId ?? 0) || activity.revision > snapshotRevision);
  let orderChanged = false;
  const updatedOrder = order.map((candidate) => {
    const activity = latestActivityByCandidate.get(candidate.candidateId);
    const candidateWithReset = activityAheadOfSnapshot && candidate.nextForNewRequest
      ? { ...candidate, nextForNewRequest: false }
      : candidate;
    orderChanged ||= candidateWithReset !== candidate;
    if (!activity || compareRuntimeActivity(activity, { runtimeId: candidate.runtimeId, revision: candidate.activityRevision ?? -1 }) <= 0) return candidateWithReset;
    orderChanged = true;
    return {
      ...candidateWithReset,
      inFlight: activity.inFlight,
      activeRequestCount: activity.activeRequestCount,
      activeModels: activity.activeModels,
    };
  });
  if (!orderChanged) return order;

  // `PoolScheduler::runtime_order` puts leased candidates first. Apply the
  // whole burst before sorting once; this keeps activity updates linear in the
  // number of candidates instead of sorting the complete order per event.
  return updatedOrder
    .map((candidate, index) => ({ candidate, originalIndex: index }))
    .sort((left, right) => {
      const leftActive = activeRequestCount(left.candidate) > 0;
      const rightActive = activeRequestCount(right.candidate) > 0;
      return Number(rightActive) - Number(leftActive) || left.originalIndex - right.originalIndex;
    })
    .map(({ candidate }) => candidate);
}

/**
 * Reconcile activity tombstones with a fresh runtime snapshot.
 *
 * A release event is kept as a tombstone so a stale lightweight poll cannot
 * resurrect a completed request. Once a newer full snapshot reports that the
 * same candidate is active again, that old tombstone must be discarded or it
 * would hide the new request until another activity event arrives.
 */
export function reconcileRuntimeActivityOverlay(
  order: readonly CandidateRuntimeSnapshot[],
  overlay: Map<string, RuntimeActivitySnapshot>,
) {
  const candidates = new Map(order.map((candidate) => [candidate.candidateId, candidate]));
  for (const [candidateId, activity] of overlay) {
    const candidate = candidates.get(candidateId);
    const runtimeId = candidate?.runtimeId ?? order[0]?.runtimeId;
    if (runtimeId != null && activity.runtimeId != null && runtimeId !== activity.runtimeId) {
      if (runtimeId > activity.runtimeId) overlay.delete(candidateId);
      continue;
    }
    const revision = candidate?.activityRevision ?? order[0]?.activityRevision;
    if (revision != null) {
      if (revision >= activity.revision) overlay.delete(candidateId);
    } else if (!candidate || (activeRequestCount(candidate) > 0 && activity.activeRequestCount === 0)) {
      overlay.delete(candidateId);
    }
  }
}

export function activeModelCounts(candidates: Iterable<CandidateRuntimeSnapshot>) {
  const requestCountsByModel = new Map<string, { model: string; requestCount: number }>();
  for (const candidate of candidates) {
    for (const activeModel of candidate.activeModels ?? []) {
      if (!activeModel.model || activeModel.requestCount <= 0) continue;
      const key = modelIdKey(activeModel.model);
      const existingModel = requestCountsByModel.get(key);
      if (existingModel) existingModel.requestCount += activeModel.requestCount;
      else requestCountsByModel.set(key, { model: activeModel.model, requestCount: activeModel.requestCount });
    }
  }
  return [...requestCountsByModel.values()].sort((left, right) =>
    right.requestCount - left.requestCount || left.model.localeCompare(right.model),
  );
}
