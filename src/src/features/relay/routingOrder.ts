import type { CandidateRuntimeSnapshot } from "./api/types";

const subscriptionPlanPriority = ["enterprise", "business", "pro-20x", "pro-5x", "pro", "plus", "go", "edu", "free", "unknown"];

export function routingOrderPositions(order: CandidateRuntimeSnapshot[]) {
  return new Map(order.map((candidate, index) => [candidate.candidateId, index]));
}

export function compareRoutingOrder(leftId: string, rightId: string, order: Map<string, number>, fallback?: Map<string, number>) {
  const left = order.get(leftId);
  const right = order.get(rightId);
  if (left != null || right != null) return (left ?? Number.MAX_SAFE_INTEGER) - (right ?? Number.MAX_SAFE_INTEGER);
  return (fallback?.get(leftId) ?? Number.MAX_SAFE_INTEGER) - (fallback?.get(rightId) ?? Number.MAX_SAFE_INTEGER);
}

export function compareSubscriptionPlanPriority(left: { id: string; label: string }, right: { id: string; label: string }) {
  const leftRank = subscriptionPlanPriority.indexOf(left.id);
  const rightRank = subscriptionPlanPriority.indexOf(right.id);
  return (leftRank < 0 ? subscriptionPlanPriority.length : leftRank) - (rightRank < 0 ? subscriptionPlanPriority.length : rightRank) || left.label.localeCompare(right.label);
}
