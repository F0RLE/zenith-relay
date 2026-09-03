import type { CandidateRuntimeSnapshot, RuntimeActivitySnapshot } from "./api/types";

const subscriptionPlanPriority = ["enterprise", "business", "pro-20x", "pro-5x", "pro", "plus", "go", "edu", "free", "unknown"];
const accountPlanOrder = ["plus", "pro", "pro-5x", "pro-20x", "business", "enterprise", "free", "go", "edu", "unknown"];

export type ApiSourceRole = "primary" | "stabilizer" | "reserve";

const API_SOURCE_PRIMARY_PRIORITY = 1_000_000;
const API_SOURCE_RESERVE_PRIORITY = -1_000_000;

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
  const retries = new Map<string, { model: string; retryAtMs: number }>();
  for (const candidate of candidates) {
    for (const retry of candidate.modelRetries ?? []) {
      if (!retry.model || !Number.isFinite(retry.retryAtMs)) continue;
      const key = retry.model.toLowerCase();
      const current = retries.get(key);
      if (!current || retry.retryAtMs < current.retryAtMs) {
        retries.set(key, { model: retry.model, retryAtMs: retry.retryAtMs });
      }
    }
  }
  return [...retries.values()].sort((left, right) => left.retryAtMs - right.retryAtMs || left.model.localeCompare(right.model));
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

export function applyRuntimeActivities(
  order: CandidateRuntimeSnapshot[],
  activities: Iterable<RuntimeActivitySnapshot>,
) {
  const updates = new Map<string, RuntimeActivitySnapshot>();
  for (const activity of activities) {
    const previous = updates.get(activity.candidateId);
    if (!previous || activity.revision > previous.revision) {
      updates.set(activity.candidateId, activity);
    }
  }
  if (!updates.size) return order;

  let changed = false;
  const next = order.map((candidate) => {
    const activity = updates.get(candidate.candidateId);
    if (!activity) return candidate;
    changed = true;
    return {
      ...candidate,
      inFlight: activity.inFlight,
      activeRequestCount: activity.activeRequestCount,
      activeModels: activity.activeModels,
    };
  });
  if (!changed) return order;

  // `PoolScheduler::runtime_order` puts leased candidates first. Apply the
  // whole burst before sorting once; this keeps activity updates linear in the
  // number of candidates instead of sorting the complete order per event.
  return next
    .map((candidate, index) => ({ candidate, index }))
    .sort((left, right) => {
      const leftActive = activeRequestCount(left.candidate) > 0;
      const rightActive = activeRequestCount(right.candidate) > 0;
      return Number(rightActive) - Number(leftActive) || left.index - right.index;
    })
    .map(({ candidate }) => candidate);
}

export function activeModelCounts(candidates: Iterable<CandidateRuntimeSnapshot>) {
  const counts = new Map<string, { model: string; requestCount: number }>();
  for (const candidate of candidates) {
    for (const activeModel of candidate.activeModels ?? []) {
      if (!activeModel.model || activeModel.requestCount <= 0) continue;
      const key = activeModel.model.toLowerCase();
      const current = counts.get(key);
      if (current) current.requestCount += activeModel.requestCount;
      else counts.set(key, { model: activeModel.model, requestCount: activeModel.requestCount });
    }
  }
  return [...counts.values()].sort((left, right) =>
    right.requestCount - left.requestCount || left.model.localeCompare(right.model),
  );
}

export function compareSubscriptionPlanPriority(left: { id: string; label: string }, right: { id: string; label: string }) {
  const leftRank = subscriptionPlanPriority.indexOf(left.id);
  const rightRank = subscriptionPlanPriority.indexOf(right.id);
  return (leftRank < 0 ? subscriptionPlanPriority.length : leftRank) - (rightRank < 0 ? subscriptionPlanPriority.length : rightRank) || left.label.localeCompare(right.label);
}

export function formatAccountPlan(planType: string | null, unknown: string) {
  const value = planType?.trim();
  if (!value) return unknown;
  const key = value.toLocaleLowerCase().replace(/[\s_-]/g, "");
  if (key.includes("team") || key.includes("business")) return "Business";
  if (key.includes("enterprise")) return "Enterprise";
  if (key === "prolite") return "Pro 5x";
  if (key === "promax") return "Pro 20x";
  if (key === "pro") return "Pro";
  if (key.includes("plus")) return "Plus";
  if (key === "free") return "Free";
  if (key === "go") return "Go";
  if (key === "edu" || key.includes("education")) return "Edu";
  return value;
}

export function accountPlanOption(planType: string | null, unknown: string) {
  const label = formatAccountPlan(planType, unknown);
  return {
    id: label.toLocaleLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-|-$/g, "") || "unknown",
    label,
  };
}

export function compareAccountPlans(left: { id: string; label: string }, right: { id: string; label: string }) {
  const leftRank = accountPlanOrder.indexOf(left.id);
  const rightRank = accountPlanOrder.indexOf(right.id);
  return (leftRank < 0 ? accountPlanOrder.length : leftRank) - (rightRank < 0 ? accountPlanOrder.length : rightRank) || left.label.localeCompare(right.label);
}

export function apiSourceRole(priority: number): ApiSourceRole {
  if (priority >= API_SOURCE_PRIMARY_PRIORITY) return "primary";
  if (priority <= API_SOURCE_RESERVE_PRIORITY) return "reserve";
  return "stabilizer";
}

export function apiSourcePriority(role: ApiSourceRole, position = 0, total = 1) {
  const index = Math.max(0, Math.trunc(position));
  const rank = Math.max(1, Math.trunc(total) - index);
  if (role === "primary") return API_SOURCE_PRIMARY_PRIORITY + rank;
  if (role === "reserve") return API_SOURCE_RESERVE_PRIORITY - index;
  return rank;
}
