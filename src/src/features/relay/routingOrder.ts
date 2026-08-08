import type { CandidateRuntimeSnapshot } from "./api/types";

const subscriptionPlanPriority = ["enterprise", "business", "pro-20x", "pro-5x", "pro", "plus", "go", "edu", "free", "unknown"];
const accountPlanOrder = ["plus", "pro", "pro-5x", "pro-20x", "business", "enterprise", "free", "go", "edu", "unknown"];

export type ApiSourceRole = "primary" | "stabilizer" | "reserve";

const API_SOURCE_PRIMARY_PRIORITY = 1_000_000;
const API_SOURCE_RESERVE_PRIORITY = -1_000_000;

export function routingOrderPositions(order: CandidateRuntimeSnapshot[]) {
  return new Map(order.map((candidate, index) => [candidate.candidateId, index]));
}

export function compareRoutingOrder(leftId: string, rightId: string, order: Map<string, number>, fallback?: Map<string, number>) {
  const left = order.get(leftId);
  const right = order.get(rightId);
  if (left != null || right != null) return (left ?? Number.MAX_SAFE_INTEGER) - (right ?? Number.MAX_SAFE_INTEGER);
  return (fallback?.get(leftId) ?? Number.MAX_SAFE_INTEGER) - (fallback?.get(rightId) ?? Number.MAX_SAFE_INTEGER);
}

export function activeRequestCount(candidate: CandidateRuntimeSnapshot | undefined) {
  return candidate?.activeRequestCount ?? candidate?.inFlight ?? 0;
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
